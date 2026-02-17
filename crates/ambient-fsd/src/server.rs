//! Daemon server that wires together store + watcher + socket server.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use rusqlite::Connection;

use ambient_fs_core::event::{FileEvent, Source};
use ambient_fs_core::filter::PathFilter;
use ambient_fs_store::{EventStore, EventPruner, PruneConfig};
use ambient_fs_watcher::FsWatcher;
use ambient_fs_server::SocketServer;

use anyhow::Result;
use ambient_fs_server::{ServerState, AnalysisPipeline, PipelineConfig, ProjectTree};
use chrono::{Duration, Utc};

/// Default paths
pub const DEFAULT_DB_PATH: &str = "/tmp/ambient-fs/events.db";
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/ambient-fs.sock";

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub db_path: PathBuf,
    pub socket_path: PathBuf,
    pub debounce_ms: u64,
    pub max_file_size_bytes: u64,
    pub machine_id: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from(DEFAULT_DB_PATH),
            socket_path: PathBuf::from(DEFAULT_SOCKET_PATH),
            debounce_ms: 100,
            max_file_size_bytes: 10 * 1024 * 1024, // 10MB
            machine_id: default_machine_id(),
        }
    }
}

/// Generate or load machine ID
fn default_machine_id() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "localhost".to_string())
}

/// Main daemon server
///
/// Wires together all components and runs the event loop.
pub struct DaemonServer {
    config: ServerConfig,
    store_path: PathBuf,
    /// Exposed for testing
    pub state: Arc<ServerState>,
    watcher: Arc<Mutex<FsWatcher>>,
    socket_server: Arc<Mutex<SocketServer>>,
    pipeline: AnalysisPipeline,
    running: Arc<Mutex<bool>>,
    /// External shutdown flag (e.g., from Daemon signal handler)
    shutdown_requested: Option<Arc<AtomicBool>>,
}

impl DaemonServer {
    /// Create a new daemon server
    pub async fn new(config: ServerConfig) -> Result<Self> {
        // Ensure db directory exists
        if let Some(parent) = config.db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Initialize event store (not async)
        let _store = EventStore::new(config.db_path.clone())?;

        // Create watcher with debounce
        let watcher = Arc::new(Mutex::new(FsWatcher::new(
            config.debounce_ms,
            "default",
            &config.machine_id,
        )));

        // Create socket server
        let socket_server = Arc::new(Mutex::new(SocketServer::new(config.socket_path.clone())));

        // Create shared server state
        let state = Arc::new(ServerState::new(config.db_path.clone()));

        // Create analysis pipeline
        let pipeline_config = PipelineConfig {
            max_concurrent: 2,
            max_file_size: config.max_file_size_bytes,
        };
        let pipeline = AnalysisPipeline::new(config.db_path.clone(), pipeline_config)
            .with_state(state.clone());

        let store_path = config.db_path.clone();
        Ok(Self {
            config,
            store_path,
            state,
            watcher,
            socket_server,
            pipeline,
            running: Arc::new(Mutex::new(false)),
            shutdown_requested: None,
        })
    }

    /// Set external shutdown flag (e.g., from Daemon signal handler)
    pub fn with_shutdown_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.shutdown_requested = Some(flag);
        self
    }

    /// Restore previously watched projects from store
    ///
    /// Reads projects from database and re-watches them if paths still exist.
    /// Stale projects (deleted paths) are removed from the store.
    /// Public for testing purposes.
    pub async fn restore_projects(&self) -> Result<()> {
        let store_path = self.store_path.clone();
        let projects = tokio::task::spawn_blocking(move || {
            let store = EventStore::new(store_path)?;
            store.list_projects()
        }).await??;

        let mut watcher = self.watcher.lock().await;
        let store_path_for_removal = self.store_path.clone();

        for (project_id, path) in projects {
            if path.exists() {
                // Path still exists, resume watching
                if let Err(e) = watcher.watch(path.clone()) {
                    tracing::warn!("failed to restore watch for {} ({}): {}", project_id, path.display(), e);
                    // Remove failed watch from store
                    let pid = project_id.clone();
                    let sp = store_path_for_removal.clone();
                    tokio::task::spawn_blocking(move || {
                        let store = EventStore::new(sp)?;
                        store.remove_project(&pid)
                    }).await.ok();
                } else {
                    // Restore project mapping
                    self.state.add_project(project_id.clone(), path.clone()).await;

                    // Rebuild tree state from filesystem
                    let tree_path = path.clone();
                    let tree_project_id = project_id.clone();
                    if let Ok(Ok(tree)) = tokio::task::spawn_blocking(move || {
                        let filter = PathFilter::default();
                        ProjectTree::from_directory(&tree_path, &filter)
                    }).await {
                        self.state.trees.write().await.insert(tree_project_id, tree);
                    }

                    tracing::info!("restored watch: {} -> {}", project_id, path.display());
                }
            } else {
                // Path deleted, remove stale project
                tracing::warn!("stale project removed: {} -> {}", project_id, path.display());
                let pid = project_id.clone();
                let sp = store_path_for_removal.clone();
                tokio::task::spawn_blocking(move || {
                    let store = EventStore::new(sp)?;
                    store.remove_project(&pid)
                }).await.ok();
            }
        }

        Ok(())
    }

    /// Start the daemon server
    ///
    /// This blocks until shutdown is requested.
    pub async fn run(&self) -> Result<()> {
        let mut running = self.running.lock().await;
        if *running {
            return Ok(());
        }
        *running = true;
        drop(running);

        // Restore previously watched projects
        self.restore_projects().await?;

        // Run initial prune cycle (catch up if daemon was down)
        let prune_scheduler = PruneScheduler::new(self.store_path.clone(), 90);
        match prune_scheduler.prune_cycle().await {
            Ok((events, analysis)) => {
                if events > 0 || analysis > 0 {
                    tracing::info!("initial prune: {} events, {} analysis records removed", events, analysis);
                }
            }
            Err(e) => tracing::warn!("initial prune failed: {}", e),
        }

        // Spawn periodic prune task (runs every 24h)
        let prune_scheduler_bg = prune_scheduler.clone();
        tokio::spawn(async move {
            prune_scheduler_bg.run().await;
        });

        // Bind socket server and set state
        {
            let mut socket = self.socket_server.lock().await;
            socket.bind()?;
            socket.set_state(self.state.clone());
            tracing::info!("socket server bound to {}", self.config.socket_path.display());
        }

        // Start watcher and get event receiver
        let mut watcher_guard = self.watcher.lock().await;
        let event_rx = watcher_guard.start()?;
        drop(watcher_guard);

        // Spawn event handler task
        let store_path = self.config.db_path.clone();
        let running = self.running.clone();
        let pipeline = self.pipeline.clone_ref();
        let state = self.state.clone();

        tokio::spawn(async move {
            let mut event_rx = event_rx;
            while let Some(event) = event_rx.recv().await {
                if *running.lock().await {
                    let path = store_path.clone();
                    let event_clone = event.clone();

                    // Step 1: persist event to store
                    tokio::task::spawn_blocking(move || {
                        if let Ok(store) = EventStore::new(path) {
                            if let Err(e) = store.insert(&event) {
                                tracing::error!("failed to write event: {}", e);
                            }
                        }
                    }).await.ok();

                    // Step 2: broadcast event to subscribers
                    state.subscriptions.publish_event(event_clone.clone()).await;

                    // Step 3: update tree state and broadcast patch if changed
                    {
                        let mut trees = state.trees.write().await;
                        if let Some(tree) = trees.get_mut(&event_clone.project_id) {
                            if let Some(patch) = tree.apply_event(&event_clone) {
                                state.subscriptions.publish_tree_patch(
                                    event_clone.project_id.clone(),
                                    patch,
                                ).await;
                            }
                        }
                    }

                    // Step 4: schedule analysis if created/modified event
                    if matches!(event_clone.event_type, ambient_fs_core::event::EventType::Created | ambient_fs_core::event::EventType::Modified) {
                        if let Some(project_root) = state.get_project(&event_clone.project_id).await {
                            pipeline.schedule_analysis(event_clone, project_root);
                        }
                    }
                }
            }
        });

        // Run socket server with signal handling for graceful shutdown
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate())?;
            let mut sigint = signal(SignalKind::interrupt())?;

            // Check external shutdown flag periodically
            let shutdown_flag = self.shutdown_requested.clone();
            let mut shutdown_interval = tokio::time::interval(tokio::time::Duration::from_secs(1));

            let socket_task = tokio::spawn({
                let socket_server = self.socket_server.clone();
                async move {
                    let mut socket = socket_server.lock().await;
                    socket.run().await
                }
            });

            tokio::select! {
                result = socket_task => result??,
                _ = sigterm.recv() => {
                    tracing::info!("SIGTERM received, shutting down");
                    self.shutdown().await?;
                }
                _ = sigint.recv() => {
                    tracing::info!("SIGINT received, shutting down");
                    self.shutdown().await?;
                }
                _ = shutdown_interval.tick() => {
                    // Check external shutdown flag (e.g., from Daemon signal handler)
                    if let Some(flag) = &shutdown_flag {
                        if flag.load(Ordering::Relaxed) {
                            tracing::info!("External shutdown flag set, shutting down");
                            self.shutdown().await?;
                        }
                    }
                }
            }
        }

        #[cfg(not(unix))]
        {
            let mut socket = self.socket_server.lock().await;
            socket.run().await?;
        }

        Ok(())
    }

    /// Shutdown the server
    pub async fn shutdown(&self) -> Result<()> {
        {
            let mut running = self.running.lock().await;
            *running = false;
        }
        {
            let socket = self.socket_server.lock().await;
            socket.shutdown()?;
        }
        {
            let mut watcher = self.watcher.lock().await;
            watcher.stop();
        }
        tracing::info!("daemon server shut down");
        Ok(())
    }

    /// Add a project to watch
    pub async fn watch_project(&self, path: PathBuf) -> Result<String> {
        let project_id = generate_project_id(&path);
        let mut watcher = self.watcher.lock().await;
        watcher.watch(path.clone())?;
        drop(watcher);

        // Add project to store
        let project_id_clone = project_id.clone();
        let path_clone = path.clone();
        let store_path = self.store_path.clone();
        tokio::task::spawn_blocking(move || {
            let store = EventStore::new(store_path)?;
            store.add_project(&project_id_clone, &path_clone)?;
            tracing::info!("watching project: {} -> {}", project_id_clone, path_clone.display());
            Ok::<(), anyhow::Error>(())
        }).await??;

        // Build initial tree state from filesystem
        let tree_path = path.clone();
        let tree_project_id = project_id.clone();
        let tree = tokio::task::spawn_blocking(move || {
            let filter = PathFilter::default();
            ProjectTree::from_directory(&tree_path, &filter)
        }).await??;
        self.state.trees.write().await.insert(tree_project_id, tree);

        self.state.add_project(project_id.clone(), path).await;

        Ok(project_id)
    }

    /// Remove a project from watch
    pub async fn unwatch_project(&self, project_id: &str) -> Result<()> {
        let store_path = self.store_path.clone();
        let project_id_owned = project_id.to_string();
        let project_id_for_log = project_id_owned.clone();

        // Get project path from store before removing
        let path_opt = tokio::task::spawn_blocking(move || {
            let store = EventStore::new(store_path)?;
            let path = store.get_project_path(&project_id_owned)?;
            store.remove_project(&project_id_owned)?;
            Ok::<Option<PathBuf>, anyhow::Error>(path)
        }).await??;

        // Unwatch the path if we found it
        if let Some(path) = path_opt {
            let mut watcher = self.watcher.lock().await;
            watcher.unwatch(path)?;
            tracing::info!("unwatched project: {}", project_id_for_log);
        }

        // Clean up tree state
        self.state.trees.write().await.remove(project_id);

        // Clean up project mapping
        self.state.remove_project(project_id).await;

        Ok(())
    }

    /// Query events from store
    pub async fn query_events(
        &self,
        project_id: Option<&str>,
        since: Option<Duration>,
        source: Option<Source>,
        limit: Option<usize>,
    ) -> Result<Vec<FileEvent>> {
        let db_path = self.config.db_path.clone();
        let project_id = project_id.map(|s| s.to_string());

        tokio::task::spawn_blocking(move || {
            let store = EventStore::new(db_path)?;
            let mut filter = ambient_fs_store::EventFilter::new();
            if let Some(pid) = project_id {
                filter = filter.project_id(pid);
            }
            if let Some(src) = source {
                filter = filter.source(src);
            }
            if let Some(lim) = limit {
                filter = filter.limit(lim);
            }
            // Handle since: convert Duration to DateTime
            if let Some(dur) = since {
                let since_secs = dur.num_seconds();
                let since_time = Utc::now() - chrono::Duration::seconds(since_secs);
                filter = filter.since(since_time);
            }
            Ok::<Vec<FileEvent>, anyhow::Error>(store.query(filter)?)
        }).await?
    }
}

/// Generate a project ID from path
fn generate_project_id(path: &PathBuf) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Prune scheduler for periodic cleanup of old events
#[derive(Debug, Clone)]
pub struct PruneScheduler {
    config: PruneConfig,
    store_path: PathBuf,
    interval: Duration,
}

impl PruneScheduler {
    /// Create new prune scheduler
    pub fn new(store_path: PathBuf, retention_days: i64) -> Self {
        Self {
            config: PruneConfig::new(retention_days),
            store_path,
            interval: Duration::hours(24), // 24h default
        }
    }

    /// Run a single prune cycle (for testing or manual trigger)
    pub async fn prune_cycle(&self) -> Result<(usize, usize)> {
        let store_path = self.store_path.clone();
        let cutoff = self.config.cutoff_timestamp();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&store_path)?;
            // Ensure schema exists (creates tables if missing)
            ambient_fs_store::migrations::ensure_schema(&conn)?;
            let events = EventPruner::prune_events_before(&conn, cutoff)?;
            let analysis = EventPruner::prune_analysis_before(&conn, cutoff)?;
            if events > 0 || analysis > 0 {
                EventPruner::vacuum(&conn)?;
            }
            Ok::<_, anyhow::Error>((events, analysis))
        }).await?
    }

    /// Run the scheduler loop (call in spawned task)
    pub async fn run(&self) {
        loop {
            tokio::time::sleep(self.interval.to_std().unwrap()).await;
            if let Err(e) = self.prune_cycle().await {
                tracing::error!("prune cycle failed: {}", e);
            } else {
                tracing::info!("prune cycle complete");
            }
        }
    }
}

impl Default for PruneScheduler {
    fn default() -> Self {
        Self {
            config: PruneConfig::default(),
            store_path: PathBuf::from(DEFAULT_DB_PATH),
            interval: Duration::hours(24),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    // ========== restore_projects ==========

    #[tokio::test]
    async fn restore_projects_rewatches_existing_paths() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create a project directory
        let project_dir = temp_dir.path().join("my-project");
        fs::create_dir_all(&project_dir).unwrap();

        // Add project to store
        let store = EventStore::new(db_path.clone()).unwrap();
        store.add_project("my-project", &project_dir).unwrap();

        // Create server (it should restore the project)
        let config = ServerConfig {
            db_path: db_path.clone(),
            socket_path: temp_dir.path().join("test.sock"),
            debounce_ms: 100,
            max_file_size_bytes: 1024,
            machine_id: "test-machine".to_string(),
        };

        let server = DaemonServer::new(config).await.unwrap();
        server.restore_projects().await.unwrap();

        // Verify project was restored to state
        assert!(server.state.has_project("my-project").await);
        let restored_path = server.state.get_project("my-project").await.unwrap();
        assert_eq!(restored_path, project_dir);
    }

    #[tokio::test]
    async fn restore_projects_removes_stale_projects() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Add a project with a path that doesn't exist
        let fake_path = temp_dir.path().join("does-not-exist");
        let store = EventStore::new(db_path.clone()).unwrap();
        store.add_project("stale-project", &fake_path).unwrap();

        // Verify it's in the store
        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 1);

        // Create server (it should clean up stale projects)
        let config = ServerConfig {
            db_path: db_path.clone(),
            socket_path: temp_dir.path().join("test.sock"),
            debounce_ms: 100,
            max_file_size_bytes: 1024,
            machine_id: "test-machine".to_string(),
        };

        let server = DaemonServer::new(config).await.unwrap();
        server.restore_projects().await.unwrap();

        // Verify project was removed from store
        let store = EventStore::new(db_path).unwrap();
        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 0, "stale project should be removed from store");

        // Verify not in state
        assert!(!server.state.has_project("stale-project").await);
    }

    #[tokio::test]
    async fn restore_projects_handles_multiple_projects() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create two project directories
        let proj1 = temp_dir.path().join("project-1");
        let proj2 = temp_dir.path().join("project-2");
        fs::create_dir_all(&proj1).unwrap();
        fs::create_dir_all(&proj2).unwrap();

        // Add projects to store
        let store = EventStore::new(db_path.clone()).unwrap();
        store.add_project("project-1", &proj1).unwrap();
        store.add_project("project-2", &proj2).unwrap();

        // Create server
        let config = ServerConfig {
            db_path: db_path.clone(),
            socket_path: temp_dir.path().join("test.sock"),
            debounce_ms: 100,
            max_file_size_bytes: 1024,
            machine_id: "test-machine".to_string(),
        };

        let server = DaemonServer::new(config).await.unwrap();
        server.restore_projects().await.unwrap();

        // Verify both projects restored
        assert!(server.state.has_project("project-1").await);
        assert!(server.state.has_project("project-2").await);

        let mut list = server.state.list_projects().await;
        list.sort();
        assert_eq!(list, vec!["project-1", "project-2"]);
    }

    #[tokio::test]
    async fn restore_projects_handles_mixed_valid_and_stale() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create one valid, one stale project
        let valid_proj = temp_dir.path().join("valid-project");
        fs::create_dir_all(&valid_proj).unwrap();
        let stale_proj = temp_dir.path().join("stale-project");

        // Add to store
        let store = EventStore::new(db_path.clone()).unwrap();
        store.add_project("valid-project", &valid_proj).unwrap();
        store.add_project("stale-project", &stale_proj).unwrap();

        // Create server
        let config = ServerConfig {
            db_path: db_path.clone(),
            socket_path: temp_dir.path().join("test.sock"),
            debounce_ms: 100,
            max_file_size_bytes: 1024,
            machine_id: "test-machine".to_string(),
        };

        let server = DaemonServer::new(config).await.unwrap();
        server.restore_projects().await.unwrap();

        // Verify only valid project remains
        assert!(server.state.has_project("valid-project").await);
        assert!(!server.state.has_project("stale-project").await);

        let store = EventStore::new(db_path).unwrap();
        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].0, "valid-project");
    }

    #[tokio::test]
    async fn restore_projects_does_nothing_when_empty() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create empty store
        let _store = EventStore::new(db_path.clone()).unwrap();

        // Create server
        let config = ServerConfig {
            db_path: db_path.clone(),
            socket_path: temp_dir.path().join("test.sock"),
            debounce_ms: 100,
            max_file_size_bytes: 1024,
            machine_id: "test-machine".to_string(),
        };

        let server = DaemonServer::new(config).await.unwrap();

        // Should not error
        server.restore_projects().await.unwrap();

        // State should be empty
        assert!(server.state.list_projects().await.is_empty());
    }

    // ========== PruneScheduler ==========

    #[tokio::test]
    async fn prune_scheduler_prunes_old_events() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create store and add old event
        let store = EventStore::new(db_path.clone()).unwrap();
        let old_event = FileEvent::new(
            ambient_fs_core::event::EventType::Created,
            "/old/file.txt",
            "test-proj",
            "m1",
        )
        .with_source(Source::User)
        .with_timestamp(Utc::now() - Duration::days(100));
        store.insert(&old_event).unwrap();

        // Add recent event
        let recent_event = FileEvent::new(
            ambient_fs_core::event::EventType::Created,
            "/new/file.txt",
            "test-proj",
            "m1",
        )
        .with_source(Source::User)
        .with_timestamp(Utc::now() - Duration::days(5));
        store.insert(&recent_event).unwrap();

        // Run prune
        let scheduler = PruneScheduler::new(db_path, 30); // 30 day retention
        let (events, analysis) = scheduler.prune_cycle().await.unwrap();

        assert_eq!(events, 1); // Only old event pruned
        assert_eq!(analysis, 0);

        // Verify recent event still exists
        let remaining = store.query(ambient_fs_store::EventFilter::new().limit(10)).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].file_path, "/new/file.txt");
    }

    #[tokio::test]
    async fn prune_scheduler_skips_vacuum_when_nothing_pruned() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create store with only recent events
        let store = EventStore::new(db_path.clone()).unwrap();
        let recent = FileEvent::new(
            ambient_fs_core::event::EventType::Created,
            "/recent/file.txt",
            "test-proj",
            "m1",
        )
        .with_source(Source::User)
        .with_timestamp(Utc::now() - Duration::days(1));
        store.insert(&recent).unwrap();

        // Run prune with long retention (nothing should be pruned)
        let scheduler = PruneScheduler::new(db_path, 365);
        let (events, analysis) = scheduler.prune_cycle().await.unwrap();

        assert_eq!(events, 0);
        assert_eq!(analysis, 0);

        // Event still exists
        let remaining = store.query(ambient_fs_store::EventFilter::new().limit(10)).unwrap();
        assert_eq!(remaining.len(), 1);
    }

    #[tokio::test]
    async fn prune_scheduler_default_retention_90_days() {
        let scheduler = PruneScheduler::default();
        assert_eq!(scheduler.config.retention_days, 90);
    }

    #[tokio::test]
    async fn prune_scheduler_custom_retention() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let scheduler = PruneScheduler::new(db_path, 14);
        assert_eq!(scheduler.config.retention_days, 14);
    }

    #[tokio::test]
    async fn prune_scheduler_interval_is_24h() {
        let scheduler = PruneScheduler::default();
        assert_eq!(scheduler.interval, Duration::hours(24));
    }
}

