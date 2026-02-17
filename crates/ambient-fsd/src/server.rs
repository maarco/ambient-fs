//! Daemon server that wires together store + watcher + socket server.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use chrono::Utc;

use ambient_fs_core::event::{FileEvent, Source};
use ambient_fs_store::EventStore;
use ambient_fs_watcher::FsWatcher;
use ambient_fs_server::SocketServer;

use anyhow::Result;
use ambient_fs_server::ServerState;

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
    state: Arc<ServerState>,
    watcher: Arc<Mutex<FsWatcher>>,
    socket_server: Arc<Mutex<SocketServer>>,
    running: Arc<Mutex<bool>>,
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

        let store_path = config.db_path.clone();
        Ok(Self {
            config,
            store_path,
            state,
            watcher,
            socket_server,
            running: Arc::new(Mutex::new(false)),
        })
    }

    /// Restore previously watched projects from store
    ///
    /// Reads projects from database and re-watches them if paths still exist.
    /// Stale projects (deleted paths) are removed from the store.
    async fn restore_projects(&self) -> Result<()> {
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
                    self.state.add_project(project_id.clone(), path).await;
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

        tokio::spawn(async move {
            let mut event_rx = event_rx;
            while let Some(event) = event_rx.recv().await {
                if *running.lock().await {
                    let path = store_path.clone();
                    tokio::task::spawn_blocking(move || {
                        // Create a new store connection for this task
                        if let Ok(store) = EventStore::new(path) {
                            if let Err(e) = store.insert(&event) {
                                tracing::error!("failed to write event: {}", e);
                            }
                        }
                    }).await.ok();
                }
            }
        });

        // Run socket server (this blocks)
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
            let mut socket = self.socket_server.lock().await;
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
                let since_secs = dur.as_secs() as i64;
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
