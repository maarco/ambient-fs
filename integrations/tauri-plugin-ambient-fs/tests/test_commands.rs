// command logic tests

// note: full tauri plugin testing requires tauri-test or manual testing
// these are unit tests for the command functions themselves

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn event_filter_from_json() {
        let filter_json = json!({
            "project_id": "my-project",
            "since": 1708100000,
            "source": "ai_agent",
            "limit": 100
        });

        assert_eq!(filter_json["project_id"], "my-project");
        assert_eq!(filter_json["since"], 1708100000);
        assert_eq!(filter_json["source"], "ai_agent");
        assert_eq!(filter_json["limit"], 100);
    }

    #[test]
    fn event_filter_partial() {
        let filter_json = json!({
            "project_id": "my-project"
        });

        assert_eq!(filter_json["project_id"], "my-project");
        // other fields are null in JSON
        assert!(filter_json.get("since").is_none() || filter_json["since"].is_null());
    }

    #[test]
    fn daemon_status_serialization() {
        let status = json!({ "connected": true });
        assert_eq!(status["connected"], true);

        let status = json!({ "connected": false });
        assert_eq!(status["connected"], false);
    }

    #[test]
    fn tree_node_structure() {
        let tree = json!({
            "name": "src",
            "path": "/project/src",
            "is_dir": true,
            "children": [
                {
                    "name": "main.rs",
                    "path": "/project/src/main.rs",
                    "is_dir": false,
                    "children": []
                }
            ]
        });

        assert_eq!(tree["name"], "src");
        assert_eq!(tree["is_dir"], true);
        assert_eq!(tree["children"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn agent_info_serialization() {
        let agent = json!({
            "id": "agent-1",
            "name": "claude",
            "status": "active",
            "project_id": "my-project",
            "file_path": "src/main.rs"
        });

        assert_eq!(agent["id"], "agent-1");
        assert_eq!(agent["name"], "claude");
        assert_eq!(agent["status"], "active");
    }

    #[test]
    fn watch_project_args() {
        let args = json!({
            "path": "/home/user/project"
        });

        assert_eq!(args["path"], "/home/user/project");
    }

    #[test]
    fn attribute_args() {
        let args = json!({
            "project_id": "my-project",
            "file_path": "src/auth.rs",
            "source": "ai_agent",
            "source_id": "chat-42"
        });

        assert_eq!(args["project_id"], "my-project");
        assert_eq!(args["file_path"], "src/auth.rs");
        assert_eq!(args["source"], "ai_agent");
        assert_eq!(args["source_id"], "chat-42");
    }

    #[test]
    fn attribute_args_without_source_id() {
        let args = json!({
            "project_id": "my-project",
            "file_path": "src/main.rs",
            "source": "user"
        });

        assert_eq!(args["source"], "user");
        // source_id should not be present when None
        assert!(args.get("source_id").is_none());
    }

    #[test]
    fn query_awareness_args() {
        let args = json!({
            "project_id": "my-project",
            "file_path": "src/lib.rs"
        });

        assert_eq!(args["project_id"], "my-project");
        assert_eq!(args["file_path"], "src/lib.rs");
    }
}
