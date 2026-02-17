// Shared server state for socket connections
// TDD: Tests FIRST, then implementation

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use ambient_fs_watcher::FsWatcher;
use crate::agents::AgentTracker;
use crate::subscriptions::SubscriptionManager;

/// Shared state accessible to all connection handlers
#[derive(Clone)]
pub struct ServerState {
    /// Path to the SQLite database
    pub store_path: PathBuf,
    /// Event subscription manager
    pub subscriptions: SubscriptionManager,
    /// Project ID -> Path mapping
    pub projects: Arc<RwLock<HashMap<String, PathBuf>>>,
    /// Project ID -> Watcher mapping (one watcher per project)
    pub watchers: Arc<RwLock<HashMap<String, Arc<Mutex<FsWatcher>>>>>,
    /// Machine ID for event attribution
    pub machine_id: String,
    /// Agent activity tracker
    pub agent_tracker: AgentTracker,
}

impl ServerState {
    /// Create a new ServerState
    pub fn new(store_path: PathBuf) -> Self {
        Self {
            store_path,
            subscriptions: SubscriptionManager::new(),
            projects: Arc::new(RwLock::new(HashMap::new())),
            watchers: Arc::new(RwLock::new(HashMap::new())),
            machine_id: hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "unknown".to_string()),
            agent_tracker: AgentTracker::with_default_timeout(),
        }
    }

    /// Create a new ServerState with a custom machine_id
    pub fn with_machine_id(store_path: PathBuf, machine_id: String) -> Self {
        Self {
            store_path,
            subscriptions: SubscriptionManager::new(),
            projects: Arc::new(RwLock::new(HashMap::new())),
            watchers: Arc::new(RwLock::new(HashMap::new())),
            machine_id,
            agent_tracker: AgentTracker::with_default_timeout(),
        }
    }

    /// Get the store path
    pub fn store_path(&self) -> &PathBuf {
        &self.store_path
    }

    /// Add a project to the watch list
    pub async fn add_project(&self, project_id: String, path: PathBuf) {
        let mut projects = self.projects.write().await;
        projects.insert(project_id, path);
    }

    /// Remove a project from the watch list
    pub async fn remove_project(&self, project_id: &str) -> Option<PathBuf> {
        let mut projects = self.projects.write().await;
        projects.remove(project_id)
    }

    /// Get a project's path
    pub async fn get_project(&self, project_id: &str) -> Option<PathBuf> {
        let projects = self.projects.read().await;
        projects.get(project_id).cloned()
    }

    /// List all project IDs
    pub async fn list_projects(&self) -> Vec<String> {
        let projects = self.projects.read().await;
        projects.keys().cloned().collect()
    }

    /// Check if a project is being watched
    pub async fn has_project(&self, project_id: &str) -> bool {
        let projects = self.projects.read().await;
        projects.contains_key(project_id)
    }

    /// Get a watcher for a project
    pub async fn get_watcher(&self, project_id: &str) -> Option<Arc<Mutex<FsWatcher>>> {
        let watchers = self.watchers.read().await;
        watchers.get(project_id).cloned()
    }

    /// Add a watcher for a project
    pub async fn add_watcher(&self, project_id: String, watcher: FsWatcher) {
        let mut watchers = self.watchers.write().await;
        watchers.insert(project_id, Arc::new(Mutex::new(watcher)));
    }

    /// Remove a watcher for a project
    pub async fn remove_watcher(&self, project_id: &str) -> Option<Arc<Mutex<FsWatcher>>> {
        let mut watchers = self.watchers.write().await;
        watchers.remove(project_id)
    }

    /// Get the active agent for a specific file
    pub async fn get_active_agent(&self, file_path: &str) -> Option<String> {
        self.agent_tracker.get_active_agent(file_path).await
    }

    /// Update agent state from an activity line
    pub async fn update_agent_activity(&self, activity: &crate::agents::AgentActivity) {
        self.agent_tracker.update_from_activity(activity).await;
    }

    /// Get all active agents
    pub async fn get_all_agents(&self) -> Vec<crate::agents::AgentState> {
        self.agent_tracker.get_all_agents().await
    }

    /// Prune stale agents
    pub async fn prune_stale_agents(&self) -> usize {
        self.agent_tracker.prune_stale().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ========== ServerState::new ==========

    #[test]
    fn new_creates_state_with_store_path() {
        let path = PathBuf::from("/tmp/test.db");
        let state = ServerState::new(path.clone());

        assert_eq!(state.store_path(), &path);
    }

    #[test]
    fn new_has_empty_projects() {
        let path = PathBuf::from("/tmp/test.db");
        let state = ServerState::new(path);

        // projects should be empty
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                assert!(state.list_projects().await.is_empty());
                assert!(!state.has_project("anything").await);
            });
    }

    // ========== ServerState::add_project ==========

    #[tokio::test]
    async fn add_project_inserts_mapping() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        state.add_project("proj-1".to_string(), PathBuf::from("/home/user/proj1")).await;

        assert!(state.has_project("proj-1").await);
        assert_eq!(state.get_project("proj-1").await, Some(PathBuf::from("/home/user/proj1")));
    }

    #[tokio::test]
    async fn add_multiple_projects() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        state.add_project("proj-1".to_string(), PathBuf::from("/a/b")).await;
        state.add_project("proj-2".to_string(), PathBuf::from("/c/d")).await;
        state.add_project("proj-3".to_string(), PathBuf::from("/e/f")).await;

        let mut projects = state.list_projects().await;
        projects.sort();
        assert_eq!(projects, vec!["proj-1", "proj-2", "proj-3"]);
    }

    #[tokio::test]
    async fn add_project_overwrites_existing() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        state.add_project("proj-1".to_string(), PathBuf::from("/old/path")).await;
        state.add_project("proj-1".to_string(), PathBuf::from("/new/path")).await;

        assert_eq!(state.get_project("proj-1").await, Some(PathBuf::from("/new/path")));
    }

    // ========== ServerState::remove_project ==========

    #[tokio::test]
    async fn remove_project_deletes_mapping() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        state.add_project("proj-1".to_string(), PathBuf::from("/a/b")).await;
        assert!(state.has_project("proj-1").await);

        let removed = state.remove_project("proj-1").await;
        assert_eq!(removed, Some(PathBuf::from("/a/b")));
        assert!(!state.has_project("proj-1").await);
    }

    #[tokio::test]
    async fn remove_nonexistent_project_returns_none() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        let removed = state.remove_project("nonexistent").await;
        assert!(removed.is_none());
    }

    // ========== ServerState::get_project ==========

    #[tokio::test]
    async fn get_project_returns_none_for_unknown() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        assert!(state.get_project("unknown").await.is_none());
    }

    #[tokio::test]
    async fn get_project_returns_path_for_known() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));
        let path = PathBuf::from("/home/user/myproject");

        state.add_project("my-project".to_string(), path.clone()).await;

        assert_eq!(state.get_project("my-project").await, Some(path));
    }

    // ========== ServerState::has_project ==========

    #[tokio::test]
    async fn has_project_returns_false_for_empty() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));
        assert!(!state.has_project("anything").await);
    }

    #[tokio::test]
    async fn has_project_returns_true_after_add() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        assert!(!state.has_project("proj-1").await);
        state.add_project("proj-1".to_string(), PathBuf::from("/a")).await;
        assert!(state.has_project("proj-1").await);
    }

    // ========== ServerState::list_projects ==========

    #[tokio::test]
    async fn list_projects_returns_empty_initially() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));
        assert!(state.list_projects().await.is_empty());
    }

    #[tokio::test]
    async fn list_projects_returns_all_ids() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        state.add_project("aaa".to_string(), PathBuf::from("/a")).await;
        state.add_project("zzz".to_string(), PathBuf::from("/z")).await;
        state.add_project("mmm".to_string(), PathBuf::from("/m")).await;

        let mut projects = state.list_projects().await;
        projects.sort();
        assert_eq!(projects, vec!["aaa", "mmm", "zzz"]);
    }

    // ========== ServerState::subscriptions ==========

    #[tokio::test]
    async fn subscriptions_field_accessible() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        // Should be able to use subscriptions
        let mut rx = state.subscriptions.subscribe("test-project").await;
        assert_eq!(state.subscriptions.subscriber_count("test-project").await, 1);

        // Broadcast works
        use ambient_fs_core::event::{FileEvent, EventType, Source};

        let event = FileEvent::new(
            EventType::Modified,
            "src/main.rs",
            "test-project",
            "test-machine",
        );
        state.subscriptions.broadcast(event).await;
    }

    // ========== Clone behavior ==========

    #[test]
    fn clone_shares_same_state() {
        let state1 = ServerState::new(PathBuf::from("/tmp/test.db"));
        let state2 = state1.clone();

        assert_eq!(state1.store_path(), state2.store_path());

        // Projects should be shared
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                state1.add_project("proj".to_string(), PathBuf::from("/p")).await;
                assert!(state2.has_project("proj").await);
            });
    }

    // ========== ServerState::watchers ==========

    #[tokio::test]
    async fn new_has_empty_watchers() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        let watcher = state.get_watcher("proj-1").await;
        assert!(watcher.is_none());
    }

    #[tokio::test]
    async fn add_watcher_stores_watcher() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        use ambient_fs_watcher::FsWatcher;
        let watcher = FsWatcher::new(100, "proj-1", "machine-1");
        state.add_watcher("proj-1".to_string(), watcher).await;

        let retrieved = state.get_watcher("proj-1").await;
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn remove_watcher_removes_from_state() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        use ambient_fs_watcher::FsWatcher;
        let watcher = FsWatcher::new(100, "proj-1", "machine-1");
        state.add_watcher("proj-1".to_string(), watcher).await;

        let removed = state.remove_watcher("proj-1").await;
        assert!(removed.is_some());

        let retrieved = state.get_watcher("proj-1").await;
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn remove_nonexistent_watcher_returns_none() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));

        let removed = state.remove_watcher("nonexistent").await;
        assert!(removed.is_none());
    }

    // ========== ServerState::machine_id ==========

    #[test]
    fn new_has_machine_id() {
        let state = ServerState::new(PathBuf::from("/tmp/test.db"));
        assert!(!state.machine_id.is_empty());
    }

    #[test]
    fn with_machine_id_uses_custom_id() {
        let state = ServerState::with_machine_id(PathBuf::from("/tmp/test.db"), "custom-machine".to_string());
        assert_eq!(state.machine_id, "custom-machine");
    }
}
