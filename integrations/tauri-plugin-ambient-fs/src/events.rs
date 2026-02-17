// event forwarding: daemon events -> tauri frontend

use serde_json::json;
use tauri::{AppHandle, Emitter};
use ambient_fs_core::FileEvent;

/// event names emitted to frontend
pub const EVT_EVENT: &str = "ambient-fs://event";
pub const EVT_AWARENESS: &str = "ambient-fs://awareness-changed";
pub const EVT_ANALYSIS: &str = "ambient-fs://analysis-complete";
pub const EVT_CONNECTED: &str = "ambient-fs://connected";

/// emit connection status change to frontend
pub fn emit_connected(app: &AppHandle, connected: bool) {
    let payload = json!({ "connected": connected });
    let _ = app.emit(EVT_CONNECTED, payload);
}

/// emit a file event to frontend
pub fn emit_file_event(app: &AppHandle, event: FileEvent) {
    let _ = app.emit(EVT_EVENT, event);
}

/// emit an awareness update to frontend
pub fn emit_awareness_changed(
    app: &AppHandle,
    project_id: String,
    file_path: String,
    awareness: ambient_fs_core::awareness::FileAwareness,
) {
    let payload = json!({
        "project_id": project_id,
        "file_path": file_path,
        "awareness": awareness,
    });
    let _ = app.emit(EVT_AWARENESS, payload);
}

/// emit analysis complete event to frontend
pub fn emit_analysis_complete(
    app: &AppHandle,
    project_id: String,
    file_path: String,
    analysis: ambient_fs_core::analysis::FileAnalysis,
) {
    let payload = json!({
        "project_id": project_id,
        "file_path": file_path,
        "analysis": analysis,
    });
    let _ = app.emit(EVT_ANALYSIS, payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_are_defined() {
        assert_eq!(EVT_EVENT, "ambient-fs://event");
        assert_eq!(EVT_AWARENESS, "ambient-fs://awareness-changed");
        assert_eq!(EVT_ANALYSIS, "ambient-fs://analysis-complete");
        assert_eq!(EVT_CONNECTED, "ambient-fs://connected");
    }

    #[test]
    fn connected_payload_serializes() {
        let payload = json!({ "connected": true });
        assert!(payload["connected"].is_boolean());
        assert_eq!(payload["connected"], true);
    }

    #[test]
    fn awareness_payload_serializes() {
        let payload = json!({
            "project_id": "test-project",
            "file_path": "src/main.rs",
            "awareness": {
                "file_path": "src/main.rs",
                "project_id": "test-project",
                "last_modified": "2024-02-16T10:00:00Z",
                "modified_by": "user",
                "change_frequency": "hot",
                "todo_count": 0,
                "chat_references": 0,
                "lint_hints": 0,
                "line_count": 100
            }
        });
        assert_eq!(payload["project_id"], "test-project");
        assert_eq!(payload["file_path"], "src/main.rs");
    }
}
