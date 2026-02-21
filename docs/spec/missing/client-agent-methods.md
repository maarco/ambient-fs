# Client Agent Methods Implementation Spec

## Overview

Add agent-related methods to the `ambient-fs-client` crate to enable:
1. Watching agent activity directories (JSONL files)
2. Unwatching agent directories
3. Reporting agent activity directly
4. Querying active agents with typed `AgentInfo` struct

## Server API Reference

The server already implements these handlers in `crates/ambient-fs-server/src/socket.rs`:

- `handle_watch_agents` (line 763): Expects `{path: "/abs/path"}` -> `{"watching": true, "path": "..."}`
- `handle_unwatch_agents` (line 817): Expects `{path: "/abs/path"}` -> `true`
- `handle_query_agents` (line 846): Optional `{file: "..."}` -> `[AgentInfo]`
- `handle_report_agent_activity` (line 1206): Expects `AgentActivity` fields -> `{"recorded": true}`

## 1. AgentInfo Struct

**Location**: `crates/ambient-fs-client/src/client.rs` (new struct, after `TreePatchParams`)

```rust
/// Information about an active agent
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Unique agent identifier
    pub agent_id: String,
    /// Files this agent is currently working on
    pub files: Vec<String>,
    /// Unix timestamp of last activity
    pub last_seen: i64,
    /// Human-readable description of current work
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// AI tool being used (claude-code, cursor, etc)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Session/conversation identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}
```

No `is_stale()` method -- that would pull `chrono` into the client crate
for a one-liner the caller can do themselves: `(now - info.last_seen) > timeout`.

**JSON Format**:

```json
{
  "agent_id": "claude-7",
  "files": ["src/auth.rs", "src/main.rs"],
  "last_seen": 1708099200,
  "intent": "fixing auth bypass",
  "tool": "claude-code",
  "session": "conv-42"
}
```

## 2. AgentActivity Struct

**Location**: `crates/ambient-fs-client/src/client.rs` (new struct, after `AgentInfo`)

```rust
/// Agent activity record for reporting
///
/// Mirrors ambient-fs-server::agents::AgentActivity exactly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentActivity {
    /// Unix timestamp (seconds)
    pub ts: i64,
    /// Agent identifier
    pub agent: String,
    /// What the agent is doing (edit, read, etc)
    pub action: String,
    /// Relative file path being acted on
    pub file: String,
    /// Optional project identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Which AI tool (claude-code, cursor, etc)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Session/conversation id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Line numbers being edited
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<u32>>,
    /// 0.0-1.0 confidence level
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// True when agent is finished with this file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
}

impl AgentActivity {
    /// Minimum required fields constructor
    pub fn new(ts: i64, agent: impl Into<String>, action: impl Into<String>, file: impl Into<String>) -> Self {
        Self {
            ts,
            agent: agent.into(),
            action: action.into(),
            file: file.into(),
            project: None,
            tool: None,
            session: None,
            intent: None,
            lines: None,
            confidence: None,
            done: None,
        }
    }

    /// Check if this activity marks a file as done
    pub fn is_done(&self) -> bool {
        self.done.unwrap_or(false)
    }
}
```

**JSON Format**:

```json
{
  "ts": 1708099200,
  "agent": "claude-7",
  "action": "edit",
  "file": "src/auth.rs",
  "project": "ambient-fs",
  "tool": "claude-code",
  "session": "conv-42",
  "intent": "fixing auth bypass",
  "lines": [42, 67],
  "done": false
}
```

## 3. Client Methods

**Location**: `crates/ambient-fs-client/src/client.rs` (add to `impl AmbientFsClient`)

### 3.1 watch_agents

```rust
/// Watch a directory for agent activity JSONL files
///
/// JSON-RPC method: watch_agents
/// Params: {"path": "/absolute/path/to/.agents"}
/// Returns: {"watching": true, "path": "..."}
pub async fn watch_agents(&mut self, path: &str) -> Result<()> {
    let params = json!({ "path": path });
    self.send_request("watch_agents", &params).await?;
    Ok(())
}
```

**Request Example**:
```json
{"jsonrpc":"2.0","method":"watch_agents","params":{"path":"/home/user/project/.agents"},"id":1}
```

**Response Example**:
```json
{"jsonrpc":"2.0","result":{"watching":true,"path":"/home/user/project/.agents"},"id":1}
```

### 3.2 unwatch_agents

```rust
/// Stop watching a directory for agent activity
///
/// JSON-RPC method: unwatch_agents
/// Params: {"path": "/absolute/path/to/.agents"}
/// Returns: true
pub async fn unwatch_agents(&mut self, path: &str) -> Result<()> {
    let params = json!({ "path": path });
    self.send_request("unwatch_agents", &params).await?;
    Ok(())
}
```

**Request Example**:
```json
{"jsonrpc":"2.0","method":"unwatch_agents","params":{"path":"/home/user/project/.agents"},"id":2}
```

**Response Example**:
```json
{"jsonrpc":"2.0","result":true,"id":2}
```

### 3.3 report_agent_activity

```rust
/// Report agent activity to the daemon
///
/// JSON-RPC method: report_agent_activity
/// Params: AgentActivity struct fields
/// Returns: {"recorded": true}
pub async fn report_agent_activity(&mut self, activity: &AgentActivity) -> Result<()> {
    self.send_request("report_agent_activity", activity).await?;
    Ok(())
}
```

**Request Example**:
```json
{
  "jsonrpc":"2.0",
  "method":"report_agent_activity",
  "params":{
    "ts":1708099200,
    "agent":"claude-7",
    "action":"edit",
    "file":"src/auth.rs",
    "intent":"fixing auth bypass",
    "tool":"claude-code"
  },
  "id":3
}
```

**Response Example**:
```json
{"jsonrpc":"2.0","result":{"recorded":true},"id":3}
```

### 3.4 query_agents (updated signature)

**Current signature** (line 231):
```rust
pub async fn query_agents(&mut self) -> Result<Vec<serde_json::Value>>
```

**Updated signature** (two methods -- unfiltered + filtered):
```rust
/// Query all active agents
///
/// JSON-RPC method: query_agents
/// Params: {}
/// Returns: Vec<AgentInfo>
pub async fn query_agents(&mut self) -> Result<Vec<AgentInfo>> {
    let empty = json!({});
    let response = self.send_request("query_agents", &empty).await?;
    serde_json::from_value(response)
        .map_err(|_| ClientError::InvalidResponse)
}

/// Query agents working on a specific file
///
/// JSON-RPC method: query_agents
/// Params: {"file": "src/main.rs"}
/// Returns: Vec<AgentInfo>
pub async fn query_agents_for_file(&mut self, file: &str) -> Result<Vec<AgentInfo>> {
    let params = json!({ "file": file });
    let response = self.send_request("query_agents", &params).await?;
    serde_json::from_value(response)
        .map_err(|_| ClientError::InvalidResponse)
}
```

**Request Example** (all agents):
```json
{"jsonrpc":"2.0","method":"query_agents","id":4}
```

**Request Example** (filter by file):
```json
{"jsonrpc":"2.0","method":"query_agents","params":{"file":"src/main.rs"},"id":4}
```

**Response Example**:
```json
{
  "jsonrpc":"2.0",
  "result":[
    {
      "agent_id":"claude-7",
      "files":["src/auth.rs","src/main.rs"],
      "last_seen":1708099200,
      "intent":"fixing auth bypass",
      "tool":"claude-code",
      "session":"conv-42"
    },
    {
      "agent_id":"cursor-1",
      "files":["src/lib.rs"],
      "last_seen":1708099250,
      "intent":null,
      "tool":"cursor",
      "session":null
    }
  ],
  "id":4
}
```

## 4. lib.rs Re-exports

**Location**: `crates/ambient-fs-client/src/lib.rs`

**Update** (after line 9):

```rust
pub use client::{
    AmbientFsClient, ClientError, DEFAULT_SOCKET_PATH, EventFilter, Notification,
    AwarenessChangedParams, AnalysisCompleteParams, TreePatchParams, Result,
    AgentInfo, AgentActivity,  // NEW
};
```

## 5. Tests

**Location**: `crates/ambient-fs-client/src/client.rs` (add to `#[cfg(test)] mod tests`)

### 5.1 watch_agents tests

All tests use `mock_client()` helper (creates client via UnixStream::pair()
+ AmbientFsClient::from_stream()).

```rust
#[tokio::test]
async fn watch_agents_sends_request() {
    let (mut client, server) = mock_client();

    tokio::spawn(async move {
        let (read_half, mut write_half) = server.into_split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let req: JsonValue = serde_json::from_str(&line).unwrap();
        let id = req["id"].as_u64().unwrap();
        assert_eq!(req["method"], "watch_agents");
        assert_eq!(req["params"]["path"], "/home/user/.agents");

        let resp = format!("{}\n", json!({"jsonrpc":"2.0","result":{"watching":true,"path":"/home/user/.agents"},"id":id}));
        write_half.write_all(resp.as_bytes()).await.unwrap();
    });

    client.watch_agents("/home/user/.agents").await.unwrap();
}

#[tokio::test]
async fn watch_agents_connection_closed() {
    let (mut client, server) = mock_client();
    drop(server);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let result = client.watch_agents("/home/user/.agents").await;
    assert!(result.is_err());
}
```

### 5.2 unwatch_agents tests

```rust
#[tokio::test]
async fn unwatch_agents_sends_request() {
    let (mut client, server) = mock_client();

    tokio::spawn(async move {
        let (read_half, mut write_half) = server.into_split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let req: JsonValue = serde_json::from_str(&line).unwrap();
        let id = req["id"].as_u64().unwrap();
        assert_eq!(req["method"], "unwatch_agents");

        let resp = format!("{}\n", json!({"jsonrpc":"2.0","result":true,"id":id}));
        write_half.write_all(resp.as_bytes()).await.unwrap();
    });

    client.unwatch_agents("/home/user/.agents").await.unwrap();
}
```

### 5.3 report_agent_activity tests

```rust
#[tokio::test]
async fn report_agent_activity_sends_request() {
    let (mut client, server) = mock_client();

    tokio::spawn(async move {
        let (read_half, mut write_half) = server.into_split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let req: JsonValue = serde_json::from_str(&line).unwrap();
        let id = req["id"].as_u64().unwrap();
        assert_eq!(req["method"], "report_agent_activity");
        assert_eq!(req["params"]["agent"], "claude-7");
        assert_eq!(req["params"]["file"], "src/auth.rs");

        let resp = format!("{}\n", json!({"jsonrpc":"2.0","result":{"recorded":true},"id":id}));
        write_half.write_all(resp.as_bytes()).await.unwrap();
    });

    let activity = AgentActivity::new(1708099200, "claude-7", "edit", "src/auth.rs");
    client.report_agent_activity(&activity).await.unwrap();
}

#[test]
fn report_agent_activity_serialization() {
    let activity = AgentActivity {
        ts: 1708099200,
        agent: "claude-7".to_string(),
        action: "edit".to_string(),
        file: "src/auth.rs".to_string(),
        project: Some("ambient-fs".to_string()),
        tool: Some("claude-code".to_string()),
        intent: Some("fixing bugs".to_string()),
        lines: Some(vec![42, 67]),
        confidence: None,
        done: Some(false),
        session: None,
    };

    let json = serde_json::to_string(&activity).unwrap();
    assert!(json.contains("claude-7"));
    assert!(json.contains("src/auth.rs"));
    assert!(json.contains("fixing bugs"));
    assert!(!json.contains("confidence")); // None fields omitted
}
```

### 5.4 AgentInfo tests

```rust
#[test]
fn agent_info_deserialization() {
    let json = r#"{
        "agent_id": "claude-7",
        "files": ["src/auth.rs", "src/main.rs"],
        "last_seen": 1708099200,
        "intent": "fixing auth bypass",
        "tool": "claude-code",
        "session": "conv-42"
    }"#;

    let info: AgentInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.agent_id, "claude-7");
    assert_eq!(info.files.len(), 2);
    assert_eq!(info.intent, Some("fixing auth bypass".to_string()));
}

#[tokio::test]
async fn query_agents_response_parsing() {
    let agents_json = r#"{
        "jsonrpc":"2.0",
        "result":[
            {
                "agent_id":"claude-7",
                "files":["src/auth.rs"],
                "last_seen":1708099200,
                "intent":"fixing bugs",
                "tool":"claude-code",
                "session":"conv-42"
            }
        ],
        "id":1
    }"#;

    let response: JsonRpcResponse = serde_json::from_str(agents_json).unwrap();
    match response.payload {
        ResponsePayload::Success { result } => {
            let agents: Vec<AgentInfo> = serde_json::from_value(result).unwrap();
            assert_eq!(agents.len(), 1);
            assert_eq!(agents[0].agent_id, "claude-7");
        }
        ResponsePayload::Error { .. } => panic!("expected success"),
    }
}
```

### 5.5 AgentActivity tests

```rust
#[test]
fn agent_activity_new_creates_minimal() {
    let activity = AgentActivity::new(1708099200, "agent-1", "edit", "src/main.rs");
    assert_eq!(activity.ts, 1708099200);
    assert_eq!(activity.agent, "agent-1");
    assert_eq!(activity.action, "edit");
    assert_eq!(activity.file, "src/main.rs");
    assert!(activity.project.is_none());
    assert!(activity.intent.is_none());
}

#[test]
fn agent_activity_serialize_full() {
    let activity = AgentActivity {
        ts: 1708099200,
        agent: "claude-7".to_string(),
        action: "edit".to_string(),
        file: "src/auth.rs".to_string(),
        project: Some("ambient-fs".to_string()),
        tool: Some("claude-code".to_string()),
        session: Some("session-42".to_string()),
        intent: Some("fixing auth bypass".to_string()),
        lines: Some(vec![42, 67]),
        confidence: Some(0.95),
        done: Some(false),
    };

    let json = serde_json::to_string(&activity).unwrap();
    let parsed: AgentActivity = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed, activity);
    assert_eq!(parsed.intent, Some("fixing auth bypass".to_string()));
    assert_eq!(parsed.lines, Some(vec![42, 67]));
    assert_eq!(parsed.confidence, Some(0.95));
}

#[test]
fn agent_activity_is_done_checks_flag() {
    let mut activity = AgentActivity::new(1708099200, "agent-1", "edit", "src/foo.rs");
    assert!(!activity.is_done());

    activity.done = Some(true);
    assert!(activity.is_done());

    activity.done = Some(false);
    assert!(!activity.is_done());
}
```

## 6. Summary of Changes

### Files Modified

1. **crates/ambient-fs-client/src/client.rs**
   - Add `AgentInfo` struct (after `TreePatchParams`)
   - Add `AgentActivity` struct (after `AgentInfo`)
   - Add `watch_agents()` method to `AmbientFsClient`
   - Add `unwatch_agents()` method to `AmbientFsClient`
   - Add `report_agent_activity()` method to `AmbientFsClient`
   - Update `query_agents()` return type from `Vec<serde_json::Value>` to `Vec<AgentInfo>`
   - Add tests for all new methods and structs

2. **crates/ambient-fs-client/src/lib.rs**
   - Re-export `AgentInfo` and `AgentActivity`

### Dependencies

No new dependencies required. Uses existing:
- `serde` / `serde_json` for serialization
- `chrono` for timestamp handling (already in workspace)

### Compatibility

- **Breaking change**: `query_agents()` return type changes from `Vec<serde_json::Value>` to `Vec<AgentInfo>`
  - Users accessing raw JSON will need to use the typed struct
  - This is an improvement (type safety) and aligns with other methods

### Server Compatibility

All methods already implemented in server:
- `handle_watch_agents` (socket.rs:763)
- `handle_unwatch_agents` (socket.rs:817)
- `handle_query_agents` (socket.rs:846)
- `handle_report_agent_activity` (socket.rs:1206)

No server changes required.
