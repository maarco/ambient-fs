# Client query_tree() Implementation Spec

## Overview

Add `query_tree()` method to `AmbientFsClient` to retrieve the complete file tree
for a watched project. The server already implements `handle_query_tree` in
`socket.rs` - this spec is for the client-side implementation only.

## Current State

- Server handler `handle_query_tree` exists at `crates/ambient-fs-server/src/socket.rs:1035-1082`
- Protocol method `Method::QueryTree` already defined
- `TreeNode` type exists in `ambient-fs-core` with full serde support
- Client has other similar query methods: `query_events`, `query_awareness`, `query_agents`

## Implementation Requirements

### 1. Method Signature

Add to `AmbientFsClient` in `crates/ambient-fs-client/src/client.rs`:

```rust
/// Query the complete file tree for a project
///
/// Returns the root TreeNode containing all files and directories
/// in the watched project. Returns error if project is not being watched.
///
/// # Arguments
/// * `project_id` - The project identifier (e.g., "my-project")
///
/// # Returns
/// * `Result<TreeNode>` - The root tree node or error
///
/// # Errors
/// * `ClientError::NotConnected` - if client is not connected
/// * `ClientError::DaemonError` - if project not found or daemon error
/// * `ClientError::InvalidResponse` - if response cannot be parsed
pub async fn query_tree(&mut self, project_id: &str) -> Result<TreeNode>;
```

### 2. JSON-RPC Protocol

- Method: `"query_tree"`
- Params: `{ "project_id": string }`
- Response: TreeNode serialized as JSON

Request example:
```json
{
  "jsonrpc": "2.0",
  "method": "query_tree",
  "params": { "project_id": "my-project" },
  "id": 1
}
```

Success response example:
```json
{
  "jsonrpc": "2.0",
  "result": {
    "name": "my-project",
    "path": "",
    "is_dir": true,
    "children": [
      {
        "name": "src",
        "path": "src",
        "is_dir": true,
        "children": [
          { "name": "main.rs", "path": "src/main.rs", "is_dir": false, "children": [] }
        ]
      },
      { "name": "Cargo.toml", "path": "Cargo.toml", "is_dir": false, "children": [] }
    ]
  },
  "id": 1
}
```

Error response example:
```json
{
  "jsonrpc": "2.0",
  "error": { "code": -32602, "message": "project not found: my-project" },
  "id": 1
}
```

### 3. Implementation Pattern

Follow the same pattern as `query_awareness` (client.rs:199-208):

```rust
pub async fn query_tree(&mut self, project_id: &str) -> Result<TreeNode> {
    let params = json!({
        "project_id": project_id,
    });
    let response = self.send_request("query_tree", &params).await?;
    serde_json::from_value(response)
        .map_err(|_| ClientError::InvalidResponse)
}
```

### 4. Re-export TreeNode

Add to `crates/ambient-fs-client/src/lib.rs`:

```rust
pub use client::{
    AmbientFsClient, ClientError, DEFAULT_SOCKET_PATH, EventFilter, Notification,
    AwarenessChangedParams, AnalysisCompleteParams, TreePatchParams, Result,
    TreeNode,  // ADD THIS
};

// Also need to re-export from core in client.rs:
pub use ambient_fs_core::tree::TreeNode;
```

Or re-export directly from core in lib.rs:

```rust
pub use ambient_fs_core::tree::TreeNode;
```

### 5. Tests

Add to `client.rs` tests module. Uses `mock_client()` helper which creates
a client via `UnixStream::pair()` + `AmbientFsClient::from_stream()`:

```rust
#[tokio::test]
async fn query_tree_sends_request_and_parses_response() {
    let (mut client, server) = mock_client();

    tokio::spawn(async move {
        let (read_half, mut write_half) = server.into_split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let req: JsonValue = serde_json::from_str(&line).unwrap();
        let id = req["id"].as_u64().unwrap();
        assert_eq!(req["method"], "query_tree");
        assert_eq!(req["params"]["project_id"], "my-project");

        let tree = json!({
            "name": "my-project", "path": "", "is_dir": true,
            "children": [
                {"name": "src", "path": "src", "is_dir": true, "children": [
                    {"name": "main.rs", "path": "src/main.rs", "is_dir": false, "children": []}
                ]},
                {"name": "README.md", "path": "README.md", "is_dir": false, "children": []}
            ]
        });
        let resp = format!("{}\n", json!({"jsonrpc":"2.0","result":tree,"id":id}));
        write_half.write_all(resp.as_bytes()).await.unwrap();
    });

    let tree = client.query_tree("my-project").await.unwrap();
    assert_eq!(tree.name, "my-project");
    assert!(tree.is_dir);
    assert_eq!(tree.children.len(), 2);
    assert_eq!(tree.children[0].name, "src");
}

#[tokio::test]
async fn query_tree_connection_closed_returns_error() {
    let (mut client, server) = mock_client();
    drop(server);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let result = client.query_tree("my-project").await;
    assert!(result.is_err());
}

#[test]
fn tree_node_serialization_roundtrip() {
    use ambient_fs_core::tree::TreeNode;

    let mut tree = TreeNode::dir("root", "");
    tree.children.push(TreeNode::file("README.md", "README.md"));
    tree.children.push(TreeNode::file("src/main.rs", "src/main.rs"));

    let json = serde_json::to_string(&tree).unwrap();
    let deserialized: TreeNode = serde_json::from_str(&json).unwrap();

    assert_eq!(tree, deserialized);
}
```

### 6. Error Cases

Handle these error scenarios:

1. **Not connected** -> `ClientError::NotConnected`
2. **Project not found** -> `ClientError::DaemonError("project not found: ...")`
3. **Invalid response format** -> `ClientError::InvalidResponse`
4. **Malformed JSON in TreeNode** -> `ClientError::InvalidResponse`

## Files to Modify

1. `crates/ambient-fs-client/src/client.rs`
   - Add `pub use ambient_fs_core::tree::TreeNode;` at top
   - Add `query_tree()` method after `query_agents()` (around line 236)
   - Add tests in `#[cfg(test)]` module

2. `crates/ambient-fs-client/src/lib.rs`
   - Add `TreeNode` to pub use list OR re-export from core

## Verification

After implementation:

1. Unit tests should pass:
   ```bash
   cargo test -p ambient-fs-client query_tree
   ```

2. Integration test with real daemon:
   ```rust
   // Watch a project
   client.watch_project("/path/to/project").await?;

   // Query tree
   let tree = client.query_tree("project").await?;
   assert_eq!(tree.name, "project");
   ```

3. Check that TreeNode is accessible from client crate:
   ```rust
   use ambient_fs_client::{AmbientFsClient, TreeNode};
   ```

## Notes

- TreeNode already has full `Serialize`/`Deserialize` derives
- No changes needed to server - handler exists and works
- This is purely client-side plumbing
- The tree structure is read-only from client perspective (no mutation methods needed)
