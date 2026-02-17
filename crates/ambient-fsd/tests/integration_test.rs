//! End-to-end integration tests for ambient-fs daemon
//!
//! Tests the full pipeline: watch -> event -> store -> tree state -> awareness
//!
//! NOTE: These tests use real filesystem events and need actual file creation

use std::fs;
use tempfile::TempDir;

use ambient_fsd::server::{DaemonServer, ServerConfig, PruneScheduler};
use ambient_fs_store::{EventStore, EventFilter};
use ambient_fs_core::event::{FileEvent, EventType, Source};

// ========== test_watch_creates_initial_tree ==========

#[tokio::test]
async fn test_watch_creates_initial_tree() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let socket_path = temp_dir.path().join("test.sock");

    // Create a temp project directory with some files
    let project_dir = temp_dir.path().join("my-project");
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(project_dir.join("README.md"), b"# My Project").unwrap();
    fs::create_dir_all(project_dir.join("src")).unwrap();
    fs::write(project_dir.join("src/main.rs"), b"fn main() {}").unwrap();
    fs::create_dir_all(project_dir.join("tests")).unwrap();

    // Create DaemonServer
    let config = ServerConfig {
        db_path: db_path.clone(),
        socket_path,
        debounce_ms: 50,
        max_file_size_bytes: 1024 * 1024,
        machine_id: "test-machine".to_string(),
    };
    let server = DaemonServer::new(config).await.unwrap();

    // Watch the project
    let project_id = server.watch_project(project_dir.clone()).await.unwrap();

    // Verify project is in state
    assert!(server.state.has_project(&project_id).await);
    assert_eq!(server.state.get_project(&project_id).await.unwrap(), project_dir);

    // Verify tree was created
    let tree = server.state.get_tree(&project_id).await.unwrap();
    assert_eq!(tree.project_id, project_id);

    // Verify tree structure has the files we created
    assert!(tree.find("README.md").is_some());
    assert!(tree.find("src/main.rs").is_some());
    assert!(tree.find("tests").is_some());

    // Project should be listed
    let projects = server.state.list_projects().await;
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0], project_id);
}

// ========== test_event_flow_store_to_awareness ==========

#[tokio::test]
async fn test_event_flow_store_to_awareness() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let socket_path = temp_dir.path().join("test.sock");

    // Create temp project dir
    let project_dir = temp_dir.path().join("test-proj");
    fs::create_dir_all(&project_dir).unwrap();

    // Create DaemonServer
    let config = ServerConfig {
        db_path: db_path.clone(),
        socket_path,
        debounce_ms: 50,
        max_file_size_bytes: 1024 * 1024,
        machine_id: "test-machine".to_string(),
    };
    let server = DaemonServer::new(config).await.unwrap();

    // Watch the project
    let project_id = server.watch_project(project_dir.clone()).await.unwrap();

    // Manually insert an event into the store to simulate a watcher event
    // (In real operation, the watcher generates events, but for testing
    // we bypass the need to run the full event loop)
    let test_event = FileEvent::new(
        EventType::Created,
        "test_file.txt",
        &project_id,
        "test-machine",
    )
    .with_source(Source::User);

    let store = EventStore::new(db_path.clone()).unwrap();
    store.insert(&test_event).unwrap();

    // Query events back from the server
    let events = server.query_events(
        Some(&project_id),
        None,
        None,
        Some(10),
    ).await.unwrap();

    // Should have the event we just inserted
    assert!(!events.is_empty(), "Should have at least one event");
    let found = events.iter().any(|e| e.file_path == "test_file.txt");
    assert!(found, "Should have found event for test_file.txt");

    // The tree won't have the file because we didn't go through the watcher
    // event loop that applies patches to the tree. This is expected - the
    // test verifies the event was stored correctly.
}

// ========== test_unwatch_cleans_up ==========

#[tokio::test]
async fn test_unwatch_cleans_up() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let socket_path = temp_dir.path().join("test.sock");

    // Create project dir
    let project_dir = temp_dir.path().join("cleanup-proj");
    fs::create_dir_all(&project_dir).unwrap();

    // Create DaemonServer
    let config = ServerConfig {
        db_path: db_path.clone(),
        socket_path,
        debounce_ms: 50,
        max_file_size_bytes: 1024 * 1024,
        machine_id: "test-machine".to_string(),
    };
    let server = DaemonServer::new(config).await.unwrap();

    // Watch project
    let project_id = server.watch_project(project_dir.clone()).await.unwrap();

    // Verify it's being watched
    assert!(server.state.has_project(&project_id).await);
    assert!(server.state.get_tree(&project_id).await.is_some());

    // Unwatch the project
    server.unwatch_project(&project_id).await.unwrap();

    // Verify cleanup
    assert!(!server.state.has_project(&project_id).await);
    assert!(server.state.get_tree(&project_id).await.is_none());
    assert!(server.state.get_project(&project_id).await.is_none());

    // Verify removed from store
    let store = EventStore::new(db_path).unwrap();
    let projects = store.list_projects().unwrap();
    assert!(!projects.iter().any(|(id, _)| id == &project_id));
}

// ========== test_prune_scheduler_runs ==========

#[tokio::test]
async fn test_prune_scheduler_runs() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create store and insert events with different ages
    let store = EventStore::new(db_path.clone()).unwrap();

    // Add old event (100 days old)
    let old_event = FileEvent::new(
        EventType::Created,
        "old_file.rs",
        "test-proj",
        "test-machine",
    )
    .with_source(Source::User)
    .with_timestamp(chrono::Utc::now() - chrono::Duration::days(100));
    store.insert(&old_event).unwrap();

    // Add recent event (5 days old)
    let recent_event = FileEvent::new(
        EventType::Created,
        "recent_file.rs",
        "test-proj",
        "test-machine",
    )
    .with_source(Source::User)
    .with_timestamp(chrono::Utc::now() - chrono::Duration::days(5));
    store.insert(&recent_event).unwrap();

    // Verify both are in store before prune
    let all_events = store.query(EventFilter::new().limit(10)).unwrap();
    assert_eq!(all_events.len(), 2);

    // Create prune scheduler with 30 day retention
    let scheduler = PruneScheduler::new(db_path.clone(), 30);

    // Run prune cycle
    let (events_pruned, _analysis_pruned) = scheduler.prune_cycle().await.unwrap();

    // Should have pruned the old event
    assert_eq!(events_pruned, 1);

    // Verify recent event still exists
    let remaining = store.query(EventFilter::new().limit(10)).unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].file_path.contains("recent"));
}

// ========== test_restore_projects_on_restart ==========

#[tokio::test]
async fn test_restore_projects_on_restart() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let socket_path1 = temp_dir.path().join("test1.sock");
    let socket_path2 = temp_dir.path().join("test2.sock");

    // Create a project directory
    let project_dir = temp_dir.path().join("persistent-proj");
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(project_dir.join("README.md"), b"persistent project").unwrap();

    // First server: watch the project
    let config1 = ServerConfig {
        db_path: db_path.clone(),
        socket_path: socket_path1,
        debounce_ms: 50,
        max_file_size_bytes: 1024 * 1024,
        machine_id: "test-machine".to_string(),
    };
    let server1 = DaemonServer::new(config1).await.unwrap();

    let project_id = server1.watch_project(project_dir.clone()).await.unwrap();
    assert!(server1.state.has_project(&project_id).await);

    // Verify project is in store
    let store = EventStore::new(db_path.clone()).unwrap();
    let projects = store.list_projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].0, project_id);

    // Create a NEW server instance with same db (simulating restart)
    let config2 = ServerConfig {
        db_path: db_path.clone(),
        socket_path: socket_path2,
        debounce_ms: 50,
        max_file_size_bytes: 1024 * 1024,
        machine_id: "test-machine".to_string(),
    };
    let server2 = DaemonServer::new(config2).await.unwrap();

    // Call restore_projects (this is normally done in run())
    server2.restore_projects().await.unwrap();

    // Verify project was restored
    assert!(server2.state.has_project(&project_id).await);
    let restored_path = server2.state.get_project(&project_id).await.unwrap();
    assert_eq!(restored_path, project_dir);

    // Verify tree was recreated
    let tree = server2.state.get_tree(&project_id).await;
    assert!(tree.is_some(), "Tree should be restored for project");
}

// ========== test_multiple_projects_independently ==========

#[tokio::test]
async fn test_multiple_projects_independently() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let socket_path = temp_dir.path().join("test.sock");

    // Create two separate project directories
    let proj1 = temp_dir.path().join("project-alpha");
    let proj2 = temp_dir.path().join("project-beta");
    fs::create_dir_all(&proj1).unwrap();
    fs::create_dir_all(&proj2).unwrap();

    fs::write(proj1.join("alpha.txt"), b"alpha").unwrap();
    fs::write(proj2.join("beta.txt"), b"beta").unwrap();

    let config = ServerConfig {
        db_path: db_path.clone(),
        socket_path,
        debounce_ms: 50,
        max_file_size_bytes: 1024 * 1024,
        machine_id: "test-machine".to_string(),
    };
    let server = DaemonServer::new(config).await.unwrap();

    // Watch both projects
    let id1 = server.watch_project(proj1.clone()).await.unwrap();
    let id2 = server.watch_project(proj2.clone()).await.unwrap();

    assert_ne!(id1, id2);

    // Verify both are tracked
    assert!(server.state.has_project(&id1).await);
    assert!(server.state.has_project(&id2).await);

    // Verify trees are independent
    let tree1 = server.state.get_tree(&id1).await.unwrap();
    let tree2 = server.state.get_tree(&id2).await.unwrap();

    assert!(tree1.find("alpha.txt").is_some());
    assert!(tree1.find("beta.txt").is_none());

    assert!(tree2.find("beta.txt").is_some());
    assert!(tree2.find("alpha.txt").is_none());

    // Unwatch one, other should remain
    server.unwatch_project(&id1).await.unwrap();
    assert!(!server.state.has_project(&id1).await);
    assert!(server.state.has_project(&id2).await);
}

// ========== test_query_events ==========

#[tokio::test]
async fn test_query_events() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let socket_path = temp_dir.path().join("test.sock");

    let project_dir = temp_dir.path().join("query-test");
    fs::create_dir_all(&project_dir).unwrap();

    let config = ServerConfig {
        db_path: db_path.clone(),
        socket_path,
        debounce_ms: 50,
        max_file_size_bytes: 1024 * 1024,
        machine_id: "test-machine".to_string(),
    };
    let server = DaemonServer::new(config).await.unwrap();

    let project_id = server.watch_project(project_dir.clone()).await.unwrap();

    // Query events for the project
    let events = server.query_events(
        Some(&project_id),
        None,
        None,
        Some(10),
    ).await.unwrap();

    // Query succeeded (events may or may not exist depending on initial scan)
    let _ = events;

    // Query with since filter (last hour)
    let recent_events = server.query_events(
        Some(&project_id),
        Some(chrono::Duration::seconds(3600)),
        None,
        Some(10),
    ).await.unwrap();

    // Recent events should be a subset
    assert!(recent_events.len() <= events.len());
}
