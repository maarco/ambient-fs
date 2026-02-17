// IPC command handlers for tauri frontend

use serde::Serialize;
use tauri::State;
use ambient_fs_core::{FileEvent, awareness::FileAwareness, tree::TreeNode};
use ambient_fs_client::EventFilter;
use crate::state::PluginState;

/// daemon connection status response
#[derive(Debug, Clone, Serialize)]
pub struct DaemonStatus {
    pub connected: bool,
}

/// watch a project directory
///
/// returns the project_id assigned by the daemon
#[tauri::command]
pub async fn watch_project(
    state: State<'_, PluginState>,
    path: String,
) -> Result<String, String> {
    let mut client_guard = state.client.lock().await;
    let client = client_guard.as_mut().ok_or("not connected to daemon")?;
    client.watch_project(&path).await.map_err(|e| e.to_string())
}

/// stop watching a project
#[tauri::command]
pub async fn unwatch_project(
    state: State<'_, PluginState>,
    project_id: String,
) -> Result<(), String> {
    let mut client_guard = state.client.lock().await;
    let client = client_guard.as_mut().ok_or("not connected to daemon")?;
    client.unwatch_project(&project_id).await.map_err(|e| e.to_string())
}

/// query events with optional filter
#[tauri::command]
pub async fn query_events(
    state: State<'_, PluginState>,
    project_id: Option<String>,
    since: Option<i64>,
    source: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<FileEvent>, String> {
    let mut client_guard = state.client.lock().await;
    let client = client_guard.as_mut().ok_or("not connected to daemon")?;

    let filter = EventFilter {
        project_id,
        since,
        source,
        limit,
    };

    client.query_events(filter).await.map_err(|e| e.to_string())
}

/// query awareness for a single file
#[tauri::command]
pub async fn query_awareness(
    state: State<'_, PluginState>,
    project_id: String,
    file_path: String,
) -> Result<FileAwareness, String> {
    let mut client_guard = state.client.lock().await;
    let client = client_guard.as_mut().ok_or("not connected to daemon")?;
    client.query_awareness(&project_id, &file_path).await.map_err(|e| e.to_string())
}

/// query the file tree for a project
///
/// note: this requires daemon support for tree queries (tree_state.rs)
#[tauri::command]
pub async fn query_tree(
    _state: State<'_, PluginState>,
    _project_id: String,
) -> Result<TreeNode, String> {
    // placeholder: query_tree requires daemon support
    Err("query_tree not yet implemented in daemon".to_string())
}

/// attribute a file change to a source
#[tauri::command]
pub async fn attribute(
    state: State<'_, PluginState>,
    project_id: String,
    file_path: String,
    source: String,
    source_id: Option<String>,
) -> Result<(), String> {
    let mut client_guard = state.client.lock().await;
    let client = client_guard.as_mut().ok_or("not connected to daemon")?;
    client.attribute(&project_id, &file_path, &source, source_id.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// query active agents
#[tauri::command]
pub async fn query_agents(
    state: State<'_, PluginState>,
) -> Result<Vec<serde_json::Value>, String> {
    let mut client_guard = state.client.lock().await;
    let client = client_guard.as_mut().ok_or("not connected to daemon")?;
    client.query_agents().await.map_err(|e| e.to_string())
}

/// get daemon connection status
#[tauri::command]
pub async fn get_status(
    state: State<'_, PluginState>,
) -> Result<DaemonStatus, String> {
    Ok(DaemonStatus {
        connected: state.is_connected(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_status_serializes() {
        let status = DaemonStatus { connected: true };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("connected"));
        assert!(json.contains("true"));
    }

    #[test]
    fn daemon_status_connected_false() {
        let status = DaemonStatus { connected: false };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("false"));
    }

    #[test]
    fn event_filter_can_be_constructed() {
        let filter = EventFilter {
            project_id: Some("test-project".to_string()),
            since: Some(1708100000),
            source: Some("ai_agent".to_string()),
            limit: Some(100),
        };
        assert_eq!(filter.project_id.as_deref(), Some("test-project"));
        assert_eq!(filter.since, Some(1708100000));
    }
}
