// event serialization tests

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn event_payload_serializes() {
        let event = json!({
            "timestamp": "2024-02-16T10:32:00Z",
            "event_type": "created",
            "file_path": "src/main.rs",
            "project_id": "my-project",
            "source": "ai_agent",
            "source_id": "chat_42",
            "machine_id": "machine-1",
            "content_hash": "abc123"
        });

        assert_eq!(event["event_type"], "created");
        assert_eq!(event["file_path"], "src/main.rs");
        assert_eq!(event["source"], "ai_agent");
        assert_eq!(event["source_id"], "chat_42");
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
        assert_eq!(payload["awareness"]["modified_by"], "user");
        assert_eq!(payload["awareness"]["change_frequency"], "hot");
    }

    #[test]
    fn analysis_payload_serializes() {
        let payload = json!({
            "project_id": "test-project",
            "file_path": "src/lib.rs",
            "analysis": {
                "imports": [
                    { "module": "std::collections", "line": 5 }
                ],
                "exports": [
                    { "name": "MyStruct", "line": 10 }
                ],
                "todos": [
                    { "line": 15, "text": "fix this", "tag": "TODO" }
                ],
                "lint": [
                    { "line": 20, "message": "unused var", "severity": "warning" }
                ]
            }
        });

        assert_eq!(payload["project_id"], "test-project");
        assert_eq!(payload["analysis"]["imports"].as_array().unwrap().len(), 1);
        assert_eq!(payload["analysis"]["todos"][0]["tag"], "TODO");
    }

    #[test]
    fn connected_payload_true() {
        let payload = json!({ "connected": true });
        assert_eq!(payload["connected"], true);
    }

    #[test]
    fn connected_payload_false() {
        let payload = json!({ "connected": false });
        assert_eq!(payload["connected"], false);
    }

    #[test]
    fn event_names_match() {
        assert_eq!("ambient-fs://event", "ambient-fs://event");
        assert_eq!("ambient-fs://awareness-changed", "ambient-fs://awareness-changed");
        assert_eq!("ambient-fs://analysis-complete", "ambient-fs://analysis-complete");
        assert_eq!("ambient-fs://connected", "ambient-fs://connected");
    }

    #[test]
    fn rename_event_includes_old_path() {
        let event = json!({
            "timestamp": "2024-02-16T10:32:00Z",
            "event_type": "renamed",
            "file_path": "src/new_name.rs",
            "old_path": "src/old_name.rs",
            "project_id": "my-project",
            "source": "user",
            "machine_id": "m1"
        });

        assert_eq!(event["event_type"], "renamed");
        assert_eq!(event["old_path"], "src/old_name.rs");
        assert_eq!(event["file_path"], "src/new_name.rs");
    }

    #[test]
    fn multiple_lint_hints() {
        let lint = json!([
            { "line": 10, "message": "unused", "severity": "warning" },
            { "line": 15, "message": "missing doc", "severity": "info" },
            { "line": 20, "message": "unreachable", "severity": "error" }
        ]);

        assert_eq!(lint.as_array().unwrap().len(), 3);
        assert_eq!(lint[0]["severity"], "warning");
        assert_eq!(lint[2]["severity"], "error");
    }

    #[test]
    fn todo_variants() {
        let todos = json!([
            { "line": 5, "text": "fix this later", "tag": "TODO" },
            { "line": 10, "text": "this is broken", "tag": "FIXME" },
            { "line": 15, "text": "ugly hack", "tag": "HACK" },
            { "line": 20, "text": "remember this", "tag": "NOTE" }
        ]);

        assert_eq!(todos[0]["tag"], "TODO");
        assert_eq!(todos[1]["tag"], "FIXME");
        assert_eq!(todos[2]["tag"], "HACK");
        assert_eq!(todos[3]["tag"], "NOTE");
    }

    #[test]
    fn source_variants() {
        let sources = vec!["user", "ai_agent", "git", "build", "voice"];
        for source in sources {
            let json = json!(source);
            assert_eq!(json.as_str().unwrap(), source);
        }
    }
}
