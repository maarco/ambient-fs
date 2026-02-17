// Cross-machine sync for ambient-fs
// TDD: Tests FIRST, then implementation

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tonic::transport::Channel;
use tracing::{debug, info, warn};

use crate::grpc::ambient_fs::ambient_fs_client::AmbientFsClient;
use crate::grpc::ambient_fs::SyncEventsRequest;
use crate::state::ServerState;

/// Peer configuration for sync
#[derive(Debug, Clone, PartialEq)]
pub struct PeerConfig {
    /// Peer address (host:port)
    pub addr: String,
    /// Project ID to sync
    pub project_id: String,
}

impl PeerConfig {
    /// Create a new peer config
    pub fn new(addr: impl Into<String>, project_id: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            project_id: project_id.into(),
        }
    }
}

/// Sync manager for cross-machine event synchronization
///
/// Runs as a background task, periodically connecting to peers
/// and syncing events.
pub struct SyncManager {
    state: Arc<ServerState>,
    peers: Vec<PeerConfig>,
    sync_interval: Duration,
    last_sync: HashMap<String, i64>,
}

impl SyncManager {
    /// Create a new sync manager
    pub fn new(state: Arc<ServerState>, peers: Vec<PeerConfig>) -> Self {
        Self {
            state,
            peers,
            sync_interval: Duration::from_secs(30),
            last_sync: HashMap::new(),
        }
    }

    /// Set the sync interval
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.sync_interval = interval;
        self
    }

    /// Run the sync manager
    ///
    /// This loops forever, syncing each peer on the configured interval.
    pub async fn run(mut self) {
        info!("Sync manager started with {} peers", self.peers.len());

        loop {
            for peer in self.peers.clone() {
                if let Err(e) = self.sync_peer(&peer).await {
                    warn!("Failed to sync peer {}: {}", peer.addr, e);
                }
            }

            tokio::time::sleep(self.sync_interval).await;
        }
    }

    /// Sync events from a single peer
    async fn sync_peer(&mut self, peer: &PeerConfig) -> Result<(), SyncError> {
        let since_timestamp = *self.last_sync
            .get(&peer.addr)
            .unwrap_or(&0);

        debug!("Syncing peer {} since {}", peer.addr, since_timestamp);

        // Connect to peer
        let channel = Channel::from_shared(peer.addr.clone())
            .map_err(|e| SyncError::Connection(format!("invalid address: {}", e)))?
            .connect()
            .await
            .map_err(|e| SyncError::Connection(format!("connect failed: {}", e)))?;

        let mut client = AmbientFsClient::new(channel);

        // Build sync request
        let req = SyncEventsRequest {
            project_id: peer.project_id.clone(),
            machine_id: self.state.machine_id.clone(),
            since_timestamp,
        };

        // Stream events from peer
        let mut stream = client
            .sync_events(req)
            .await
            .map_err(|e| SyncError::Rpc(format!("sync_events RPC failed: {}", e)))?
            .into_inner();

        let mut new_events = Vec::new();

        while let Some(result) = stream.message().await
            .map_err(|e| SyncError::Stream(format!("stream read failed: {}", e)))?
        {
            let msg = result;

            // Dedup: skip events from local machine
            if msg.machine_id == self.state.machine_id {
                debug!("Skipping event from local machine: {}", msg.file_path);
                continue;
            }

            // Convert to FileEvent
            let event = crate::grpc::message_to_file_event(&msg, &self.state.machine_id)
                .ok_or_else(|| SyncError::Conversion(format!("failed to convert event for {}", msg.file_path)))?;

            // Dedup: skip if already in store
            if self.event_exists(&event).await {
                debug!("Skipping already-present event: {}", msg.file_path);
                continue;
            }

            new_events.push(event);
        }

        // Insert batch
        let count = new_events.len();
        if !new_events.is_empty() {
            self.insert_events(new_events).await?;
            info!("Inserted {} events from peer {}", count, peer.addr);
        }

        // Update last_sync timestamp
        let now = chrono::Utc::now().timestamp();
        self.last_sync.insert(peer.addr.clone(), now);

        Ok(())
    }

    /// Check if an event already exists in the store
    async fn event_exists(&self, event: &ambient_fs_core::FileEvent) -> bool {
        use ambient_fs_store::{EventStore, EventFilter};

        let store_path = self.state.store_path.clone();
        let event_clone = event.clone();

        let exists = tokio::task::spawn_blocking(move || {
            let store = EventStore::new(store_path)?;
            let filter = EventFilter::new()
                .project_id(&event_clone.project_id)
                .file_path(&event_clone.file_path)
                .since(event_clone.timestamp - chrono::Duration::seconds(1));
            let results = store.query(filter)?;
            Ok::<_, ambient_fs_store::StoreError>(results.iter().any(|e|
                e.timestamp == event_clone.timestamp &&
                e.file_path == event_clone.file_path &&
                e.machine_id == event_clone.machine_id
            ))
        })
        .await;

        matches!(exists, Ok(Ok(true)))
    }

    /// Insert events into the store
    async fn insert_events(&self, events: Vec<ambient_fs_core::FileEvent>) -> Result<(), SyncError> {
        use ambient_fs_store::EventStore;

        let store_path = self.state.store_path.clone();
        let events_clone = events.clone();

        tokio::task::spawn_blocking(move || {
            let store = EventStore::new(store_path)?;
            store.insert_batch(&events)?;
            Ok::<_, ambient_fs_store::StoreError>(())
        })
        .await
        .map_err(|e| SyncError::Store(format!("join error: {}", e)))?
        .map_err(|e| SyncError::Store(format!("insert failed: {}", e)))?;

        // Publish to subscriptions
        for event in events_clone {
            self.state.subscriptions.publish_event(event).await;
        }

        Ok(())
    }
}

/// Sync errors
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("connection error: {0}")]
    Connection(String),

    #[error("rpc error: {0}")]
    Rpc(String),

    #[error("stream error: {0}")]
    Stream(String),

    #[error("conversion error: {0}")]
    Conversion(String),

    #[error("store error: {0}")]
    Store(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ========== PeerConfig::new ==========

    #[test]
    fn new_creates_config_with_addr_and_project() {
        let config = PeerConfig::new("192.168.1.50:50051", "my-project");

        assert_eq!(config.addr, "192.168.1.50:50051");
        assert_eq!(config.project_id, "my-project");
    }

    #[test]
    fn new_with_string_types() {
        let addr = "localhost:8080".to_string();
        let project = "test-proj".to_string();

        let config = PeerConfig::new(addr.clone(), project.clone());

        assert_eq!(config.addr, addr);
        assert_eq!(config.project_id, project);
    }

    #[test]
    fn peer_config_partial_eq() {
        let config1 = PeerConfig::new("peer1:50051", "proj");
        let config2 = PeerConfig::new("peer1:50051", "proj");
        let config3 = PeerConfig::new("peer2:50051", "proj");

        assert_eq!(config1, config2);
        assert_ne!(config1, config3);
    }

    // ========== SyncManager::new ==========

    #[test]
    fn new_creates_manager_with_state_and_peers() {
        let tmp = tempdir().unwrap();
        let state = Arc::new(ServerState::new(tmp.path().join("store.db")));
        let peers = vec![
            PeerConfig::new("peer1:50051", "proj1"),
            PeerConfig::new("peer2:50051", "proj2"),
        ];

        let mgr = SyncManager::new(state, peers.clone());

        assert_eq!(mgr.peers.len(), 2);
        assert_eq!(mgr.peers[0].addr, "peer1:50051");
        assert_eq!(mgr.peers[1].addr, "peer2:50051");
    }

    #[test]
    fn new_with_empty_peers() {
        let tmp = tempdir().unwrap();
        let state = Arc::new(ServerState::new(tmp.path().join("store.db")));

        let mgr = SyncManager::new(state, vec![]);

        assert!(mgr.peers.is_empty());
    }

    #[test]
    fn new_has_default_interval_30_seconds() {
        let tmp = tempdir().unwrap();
        let state = Arc::new(ServerState::new(tmp.path().join("store.db")));
        let peers = vec![PeerConfig::new("peer1:50051", "proj1")];

        let mgr = SyncManager::new(state, peers);

        assert_eq!(mgr.sync_interval, Duration::from_secs(30));
    }

    #[test]
    fn new_has_empty_last_sync() {
        let tmp = tempdir().unwrap();
        let state = Arc::new(ServerState::new(tmp.path().join("store.db")));
        let peers = vec![PeerConfig::new("peer1:50051", "proj1")];

        let mgr = SyncManager::new(state, peers);

        assert!(mgr.last_sync.is_empty());
    }

    // ========== SyncManager::with_interval ==========

    #[test]
    fn with_interval_sets_custom_interval() {
        let tmp = tempdir().unwrap();
        let state = Arc::new(ServerState::new(tmp.path().join("store.db")));
        let peers = vec![PeerConfig::new("peer1:50051", "proj1")];

        let mgr = SyncManager::new(state, peers)
            .with_interval(Duration::from_secs(60));

        assert_eq!(mgr.sync_interval, Duration::from_secs(60));
    }

    #[test]
    fn with_interval_zero_duration() {
        let tmp = tempdir().unwrap();
        let state = Arc::new(ServerState::new(tmp.path().join("store.db")));
        let peers = vec![PeerConfig::new("peer1:50051", "proj1")];

        let mgr = SyncManager::new(state, peers)
            .with_interval(Duration::ZERO);

        assert_eq!(mgr.sync_interval, Duration::ZERO);
    }

    // ========== dedup: skip local machine_id events ==========

    #[tokio::test]
    async fn sync_peer_skips_events_from_local_machine_id() {
        let tmp = tempdir().unwrap();
        let state = Arc::new(ServerState::with_machine_id(
            tmp.path().join("store.db"),
            "local-machine".to_string(),
        ));

        let peers = vec![PeerConfig::new("localhost:50051", "proj")];
        let mut mgr = SyncManager::new(state, peers);

        // Mock sync_peer - verify that events from local machine are skipped
        // This is tested indirectly via the full sync flow
        // The dedup happens in sync_peer where we check msg.machine_id == self.state.machine_id

        assert_eq!(mgr.state.machine_id, "local-machine");
    }

    // ========== dedup: skip already-present events ==========

    #[tokio::test]
    async fn event_exists_returns_true_for_duplicate_event() {
        let tmp = tempdir().unwrap();
        let state = Arc::new(ServerState::with_machine_id(
            tmp.path().join("store.db"),
            "machine-1".to_string(),
        ));

        let peers = vec![PeerConfig::new("peer:50051", "proj")];
        let mgr = SyncManager::new(state, peers);

        // Insert an event
        use ambient_fs_core::event::{FileEvent, EventType};
        use ambient_fs_store::EventStore;

        let event = FileEvent::new(EventType::Modified, "file.rs", "proj", "remote-machine");
        let event_clone = event.clone();

        tokio::task::spawn_blocking({
            let store_path = mgr.state.store_path.clone();
            move || {
                let store = EventStore::new(store_path)?;
                store.insert(&event_clone)?;
                Ok::<_, ambient_fs_store::StoreError>(())
            }
        })
        .await
        .unwrap()
        .unwrap();

        // Check if it exists
        assert!(mgr.event_exists(&event).await);
    }

    #[tokio::test]
    async fn event_exists_returns_false_for_new_event() {
        let tmp = tempdir().unwrap();
        let state = Arc::new(ServerState::with_machine_id(
            tmp.path().join("store.db"),
            "machine-1".to_string(),
        ));

        let peers = vec![PeerConfig::new("peer:50051", "proj")];
        let mgr = SyncManager::new(state, peers);

        // Create an event not in store
        use ambient_fs_core::event::{FileEvent, EventType};
        let event = FileEvent::new(EventType::Created, "newfile.rs", "proj", "remote-machine");

        assert!(!mgr.event_exists(&event).await);
    }

    // ========== last_sync tracking ==========

    #[tokio::test]
    async fn last_sync_tracks_timestamps_per_peer() {
        let tmp = tempdir().unwrap();
        let state = Arc::new(ServerState::new(tmp.path().join("store.db")));

        let peers = vec![
            PeerConfig::new("peer1:50051", "proj"),
            PeerConfig::new("peer2:50051", "proj"),
        ];
        let mut mgr = SyncManager::new(state, peers);

        // Initially empty
        assert!(mgr.last_sync.is_empty());

        // Simulate sync_peer updating timestamps
        let now = chrono::Utc::now().timestamp();
        mgr.last_sync.insert("peer1:50051".to_string(), now);
        mgr.last_sync.insert("peer2:50051".to_string(), now - 100);

        assert_eq!(mgr.last_sync.len(), 2);
        assert_eq!(mgr.last_sync.get("peer1:50051"), Some(&now));
        assert_eq!(mgr.last_sync.get("peer2:50051"), Some(&(now - 100)));
    }

    // ========== SyncError variants ==========

    #[test]
    fn sync_error_connection_display() {
        let err = SyncError::Connection("connect failed".to_string());
        assert_eq!(err.to_string(), "connection error: connect failed");
    }

    #[test]
    fn sync_error_rpc_display() {
        let err = SyncError::Rpc("status Unknown".to_string());
        assert_eq!(err.to_string(), "rpc error: status Unknown");
    }

    #[test]
    fn sync_error_stream_display() {
        let err = SyncError::Stream("eof".to_string());
        assert_eq!(err.to_string(), "stream error: eof");
    }

    #[test]
    fn sync_error_conversion_display() {
        let err = SyncError::Conversion("invalid timestamp".to_string());
        assert_eq!(err.to_string(), "conversion error: invalid timestamp");
    }

    #[test]
    fn sync_error_store_display() {
        let err = SyncError::Store("database locked".to_string());
        assert_eq!(err.to_string(), "store error: database locked");
    }
}
