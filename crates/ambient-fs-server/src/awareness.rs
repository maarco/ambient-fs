// Awareness aggregator - combines event, analysis, and agent data
// TDD: Tests FIRST, then implementation

use ambient_fs_core::awareness::FileAwareness;
use ambient_fs_store::{EventStore, FileAnalysisCache};

use crate::state::ServerState;

/// Error type for awareness building operations
#[derive(Debug, thiserror::Error)]
pub enum AwarenessError {
    #[error("store error: {0}")]
    Store(String),

    #[error("cache error: {0}")]
    Cache(String),
}

pub type Result<T> = std::result::Result<T, AwarenessError>;

/// Build FileAwareness by aggregating data from multiple sources.
///
/// Combines:
/// - EventStore: latest event for last_modified, modified_by, change_frequency
/// - FileAnalysisCache: todo_count, lint_hints, line_count
/// - AgentTracker: active_agent
///
/// Returns None if no event exists for the file (file has no history).
pub async fn build_awareness(
    state: &ServerState,
    project_id: &str,
    file_path: &str,
) -> Result<Option<FileAwareness>> {
    let store_path = state.store_path.clone();
    let project_id_owned = project_id.to_string();
    let file_path_owned = file_path.to_string();

    // Step 1: Query event store for latest event
    let event_opt = tokio::task::spawn_blocking(move || {
        let store = EventStore::new(store_path)
            .map_err(|e| AwarenessError::Store(e.to_string()))?;
        store.get_latest(&project_id_owned, &file_path_owned)
            .map_err(|e| AwarenessError::Store(e.to_string()))
    })
    .await
    .map_err(|e| AwarenessError::Store(format!("join error: {}", e)))??;

    // If no event exists, return None (no history for this file)
    let event = match event_opt {
        Some(e) => e,
        None => return Ok(None),
    };

    // Step 2: Query analysis cache
    let store_path = state.store_path.clone();
    let project_id_clone = project_id.to_string();
    let file_path_clone = file_path.to_string();

    let analysis_opt = tokio::task::spawn_blocking(move || {
        let cache_path = store_path.with_file_name("analysis.db");
        let cache = FileAnalysisCache::open(cache_path)
            .map_err(|e| AwarenessError::Cache(e.to_string()))?;
        cache.get(&project_id_clone, &file_path_clone)
            .map_err(|e| AwarenessError::Cache(e.to_string()))
    })
    .await
    .map_err(|e| AwarenessError::Cache(format!("join error: {}", e)))??;

    // Step 3: Get active agent and reference count
    let active_agent = state.get_active_agent(file_path).await;
    let chat_references = state.agent_tracker.get_reference_count(file_path).await;

    // Step 4: Combine into FileAwareness
    let mut awareness = FileAwareness::from_event_minimal(
        &event.file_path,
        &event.project_id,
        event.timestamp,
        event.source,
    );

    // Override with analysis data if available
    if let Some(analysis) = analysis_opt {
        awareness.todo_count = analysis.todo_count;
        awareness.lint_hints = analysis.lint_hints.len() as u32;
        awareness.line_count = analysis.line_count;
    }

    // Set active agent
    awareness.active_agent = active_agent;

    // Set chat references
    awareness.chat_references = chat_references;

    // Set modified_by_label if source_id exists (e.g., chat session id)
    awareness.modified_by_label = event.source_id;

    Ok(Some(awareness))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ambient_fs_core::event::{FileEvent, EventType, Source};
    use tempfile::tempdir;

    fn make_test_event(project_id: &str, file_path: &str) -> FileEvent {
        FileEvent::new(EventType::Modified, file_path, project_id, "test-machine")
            .with_source(Source::User)
    }

    // ========== build_awareness ==========

    #[tokio::test]
    async fn build_awareness_with_event_only_returns_partial_awareness() {
        let temp_dir = tempdir().unwrap();
        let store_path = temp_dir.path().join("events.db");
        let state = ServerState::new(store_path.clone());

        // Insert an event
        let event = make_test_event("proj-1", "src/main.rs");
        let store = EventStore::new(store_path).unwrap();
        store.insert(&event).unwrap();

        // Build awareness
        let result = build_awareness(&state, "proj-1", "src/main.rs").await;

        assert!(result.is_ok());
        let awareness = result.unwrap().unwrap();
        assert_eq!(awareness.file_path, "src/main.rs");
        assert_eq!(awareness.project_id, "proj-1");
        assert_eq!(awareness.modified_by, Source::User);
        assert_eq!(awareness.todo_count, 0); // no analysis = default 0
        assert_eq!(awareness.lint_hints, 0);
        assert_eq!(awareness.line_count, 0);
        assert!(awareness.active_agent.is_none());
    }

    #[tokio::test]
    async fn build_awareness_with_event_and_analysis_returns_full_awareness() {
        let temp_dir = tempdir().unwrap();
        let store_path = temp_dir.path().join("events.db");
        let analysis_path = temp_dir.path().join("analysis.db");
        let state = ServerState::new(store_path.clone());

        // Insert event
        let event = make_test_event("proj-1", "src/lib.rs");
        let store = EventStore::new(store_path).unwrap();
        store.insert(&event).unwrap();

        // Insert analysis
        use ambient_fs_core::analysis::{FileAnalysis, ImportRef, LintHint, LintSeverity};

        let analysis = FileAnalysis {
            file_path: "src/lib.rs".to_string(),
            project_id: "proj-1".to_string(),
            content_hash: "hash123".to_string(),
            exports: vec!["foo".to_string(), "bar".to_string()],
            imports: vec![ImportRef {
                path: "baz".to_string(),
                symbols: vec!["Qux".to_string()],
                line: 1,
            }],
            todo_count: 5,
            lint_hints: vec![LintHint {
                line: 10,
                column: 5,
                severity: LintSeverity::Warning,
                message: "unused".to_string(),
                rule: Some("dead_code".to_string()),
            }, LintHint {
                line: 20,
                column: 3,
                severity: LintSeverity::Error,
                message: "missing semicolon".to_string(),
                rule: Some("syntax".to_string()),
            }],
            line_count: 142,
        };

        let cache = FileAnalysisCache::open(analysis_path).unwrap();
        cache.put(&analysis).unwrap();

        // Build awareness
        let result = build_awareness(&state, "proj-1", "src/lib.rs").await;

        assert!(result.is_ok());
        let awareness = result.unwrap().unwrap();
        assert_eq!(awareness.todo_count, 5);
        assert_eq!(awareness.lint_hints, 2);
        assert_eq!(awareness.line_count, 142);
    }

    #[tokio::test]
    async fn build_awareness_with_no_event_returns_none() {
        let temp_dir = tempdir().unwrap();
        let store_path = temp_dir.path().join("events.db");
        let state = ServerState::new(store_path.clone());

        // No event inserted
        let result = build_awareness(&state, "proj-1", "src/main.rs").await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn build_awareness_with_active_agent_populates_agent_field() {
        let temp_dir = tempdir().unwrap();
        let store_path = temp_dir.path().join("events.db");
        let state = ServerState::new(store_path.clone());

        // Insert event
        let event = make_test_event("proj-1", "src/main.rs");
        let store = EventStore::new(store_path).unwrap();
        store.insert(&event).unwrap();

        // Register an active agent for this file
        use crate::agents::AgentActivity;
        let activity = AgentActivity {
            ts: chrono::Utc::now().timestamp(),
            agent: "agent-123".to_string(),
            action: "fix the bug".to_string(),
            file: "src/main.rs".to_string(),
            project: Some("proj-1".to_string()),
            tool: Some("editor".to_string()),
            session: Some("sess-456".to_string()),
            intent: Some("fixing bugs".to_string()),
            lines: Some(vec![10, 20]),
            confidence: Some(0.9),
            done: Some(false),
        };
        state.update_agent_activity(&activity).await;

        // Build awareness
        let result = build_awareness(&state, "proj-1", "src/main.rs").await;

        assert!(result.is_ok());
        let awareness = result.unwrap().unwrap();
        assert_eq!(awareness.active_agent, Some("agent-123".to_string()));
    }

    #[tokio::test]
    async fn build_awareness_change_frequency_calculated_from_event_age() {
        let temp_dir = tempdir().unwrap();
        let store_path = temp_dir.path().join("events.db");
        let state = ServerState::new(store_path.clone());

        // Insert an event (will be recent, so Hot)
        let event = make_test_event("proj-1", "src/main.rs");
        let store = EventStore::new(store_path).unwrap();
        store.insert(&event).unwrap();

        let result = build_awareness(&state, "proj-1", "src/main.rs").await;

        assert!(result.is_ok());
        let awareness = result.unwrap().unwrap();
        // Just created event should be Hot
        assert_eq!(awareness.change_frequency, ambient_fs_core::awareness::ChangeFrequency::Hot);
    }

    #[tokio::test]
    async fn build_awareness_with_source_id_sets_modified_by_label() {
        let temp_dir = tempdir().unwrap();
        let store_path = temp_dir.path().join("events.db");
        let state = ServerState::new(store_path.clone());

        // Insert event with source_id
        let mut event = make_test_event("proj-1", "src/main.rs");
        event.source_id = Some("chat-session-123".to_string());

        let store = EventStore::new(store_path).unwrap();
        store.insert(&event).unwrap();

        let result = build_awareness(&state, "proj-1", "src/main.rs").await;

        assert!(result.is_ok());
        let awareness = result.unwrap().unwrap();
        assert_eq!(awareness.modified_by_label, Some("chat-session-123".to_string()));
    }

    #[tokio::test]
    async fn build_awareness_filters_by_project_and_file() {
        let temp_dir = tempdir().unwrap();
        let store_path = temp_dir.path().join("events.db");
        let state = ServerState::new(store_path.clone());

        let store = EventStore::new(store_path).unwrap();

        // Insert events for different projects and files
        store.insert(&make_test_event("proj-1", "src/main.rs")).unwrap();
        store.insert(&make_test_event("proj-2", "src/main.rs")).unwrap();
        store.insert(&make_test_event("proj-1", "src/lib.rs")).unwrap();

        // Query proj-1/src/main.rs
        let result = build_awareness(&state, "proj-1", "src/main.rs").await;

        assert!(result.is_ok());
        let awareness = result.unwrap().unwrap();
        assert_eq!(awareness.project_id, "proj-1");
        assert_eq!(awareness.file_path, "src/main.rs");
    }

    #[tokio::test]
    async fn build_awareness_missing_cache_returns_zeros() {
        let temp_dir = tempdir().unwrap();
        let store_path = temp_dir.path().join("events.db");
        let state = ServerState::new(store_path.clone());

        // Insert event but no analysis
        let event = make_test_event("proj-1", "src/main.rs");
        let store = EventStore::new(store_path).unwrap();
        store.insert(&event).unwrap();

        // Don't create analysis.db at all

        let result = build_awareness(&state, "proj-1", "src/main.rs").await;

        // Should still work, just with zero analysis values
        assert!(result.is_ok());
        let awareness = result.unwrap().unwrap();
        assert_eq!(awareness.todo_count, 0);
        assert_eq!(awareness.lint_hints, 0);
        assert_eq!(awareness.line_count, 0);
    }

    #[tokio::test]
    async fn build_awareness_with_no_activity_has_zero_chat_references() {
        let temp_dir = tempdir().unwrap();
        let store_path = temp_dir.path().join("events.db");
        let state = ServerState::new(store_path.clone());

        // Insert event
        let event = make_test_event("proj-1", "src/main.rs");
        let store = EventStore::new(store_path).unwrap();
        store.insert(&event).unwrap();

        // No agent activity

        let result = build_awareness(&state, "proj-1", "src/main.rs").await;

        assert!(result.is_ok());
        let awareness = result.unwrap().unwrap();
        assert_eq!(awareness.chat_references, 0);
    }

    #[tokio::test]
    async fn build_awareness_with_activity_has_nonzero_chat_references() {
        let temp_dir = tempdir().unwrap();
        let store_path = temp_dir.path().join("events.db");
        let state = ServerState::new(store_path.clone());

        // Insert event
        let event = make_test_event("proj-1", "src/main.rs");
        let store = EventStore::new(store_path).unwrap();
        store.insert(&event).unwrap();

        // Add agent activity
        use crate::agents::AgentActivity;
        let now = chrono::Utc::now().timestamp();
        let activity = AgentActivity::new(now, "agent-1", "edit", "src/main.rs");
        state.update_agent_activity(&activity).await;

        // Add another activity on same file
        let activity2 = AgentActivity::new(now + 1, "agent-1", "read", "src/main.rs");
        state.update_agent_activity(&activity2).await;

        let result = build_awareness(&state, "proj-1", "src/main.rs").await;

        assert!(result.is_ok());
        let awareness = result.unwrap().unwrap();
        assert_eq!(awareness.chat_references, 2);
    }

    #[tokio::test]
    async fn build_awareness_chat_references_independent_per_file() {
        let temp_dir = tempdir().unwrap();
        let store_path = temp_dir.path().join("events.db");
        let state = ServerState::new(store_path.clone());

        // Insert events for two files
        let event1 = make_test_event("proj-1", "src/a.rs");
        let event2 = make_test_event("proj-1", "src/b.rs");
        let store = EventStore::new(store_path).unwrap();
        store.insert(&event1).unwrap();
        store.insert(&event2).unwrap();

        // Add activity: 3 refs to a.rs, 1 ref to b.rs
        use crate::agents::AgentActivity;
        let now = chrono::Utc::now().timestamp();
        for _ in 0..3 {
            let activity = AgentActivity::new(now, "agent-1", "edit", "src/a.rs");
            state.update_agent_activity(&activity).await;
        }
        let activity = AgentActivity::new(now, "agent-1", "edit", "src/b.rs");
        state.update_agent_activity(&activity).await;

        // Check a.rs
        let result_a = build_awareness(&state, "proj-1", "src/a.rs").await;
        assert!(result_a.is_ok());
        let awareness_a = result_a.unwrap().unwrap();
        assert_eq!(awareness_a.chat_references, 3);

        // Check b.rs
        let result_b = build_awareness(&state, "proj-1", "src/b.rs").await;
        assert!(result_b.is_ok());
        let awareness_b = result_b.unwrap().unwrap();
        assert_eq!(awareness_b.chat_references, 1);
    }
}
