# ambient-fs-server

Unix socket server for ambient-fs. JSON-RPC 2.0 protocol, subscription management, agent tracking, and awareness aggregation.

## Features

- **SocketServer** - Unix domain socket server
  - `bind(path)` - Bind socket, removes existing socket file
  - `set_state(state)` - Set shared ServerState
  - `run()` - Accept connections (async)
  - `shutdown()` - Graceful shutdown via broadcast channel

- **ServerState** - Shared state across all connections
  - `store_path` - Path to SQLite database
  - `subscriptions` - SubscriptionManager for live event broadcasts
  - `projects` - Project ID -> Path mapping (RwLock)
  - `watchers` - Project ID -> FsWatcher mapping
  - `agent_tracker` - AgentTracker for AI source attribution
  - `machine_id` - Stable identifier for this machine

- **SubscriptionManager** - Per-project broadcast channels
  - `subscribe(project_id)` - Get `broadcast::Receiver<Notification>`
  - `unsubscribe(project_id)` - Decrement count, drop channel if zero
  - `publish_event(event)` - Broadcast to all project subscribers

- **AgentTracker** - AI agent activity tracking
  - `update_from_activity(activity)` - Record agent editing a file
  - `get_active_agent(file_path)` - Check if an agent is editing a file
  - `get_all_agents()` - List all non-stale agents
  - Stale agents are auto-expired after 5 minutes of inactivity

- **ConnectionState** - Per-connection subscription tracking
  - Tracks `HashMap<project_id, broadcast::Receiver<Notification>>`

## Protocol Methods

| Method                | Params                                     | Returns                               |
|-----------------------|--------------------------------------------|---------------------------------------|
| watch_project         | path (required)                            | {watching: true, project_id: "..."}   |
| unwatch_project       | project_id (required)                      | true                                  |
| subscribe             | project_id (required)                      | {subscribed: true, project_id: "..."} |
| unsubscribe           | project_id (required)                      | true                                  |
| query_events          | project_id, since, source, limit (all opt) | Array of FileEvent                    |
| query_awareness       | project_id, path (required)                | FileAwareness or null                 |
| query_tree            | project_id (required)                      | TreeNode                              |
| watch_agents          | path (required)                            | {watching: true, path: "..."}         |
| unwatch_agents        | path (required)                            | true                                  |
| query_agents          | file (optional)                            | Array of AgentInfo                    |
| report_agent_activity | ts, agent, action, file (required) + opts  | {recorded: true}                      |
| attribute             | file_path, project_id, source (required)   | {attributed: true}                    |

### report_agent_activity

Injects agent activity into the AgentTracker. Any filesystem events on `file` while the agent
is active will be stored with `source: ai_agent`.

```json
{"jsonrpc":"2.0","method":"report_agent_activity","params":{
  "ts": 1700000000,
  "agent": "claude-code",
  "action": "edit",
  "file": "/abs/path/to/file.rs",
  "tool": "claude-code",
  "intent": "fixing the auth bug",
  "done": false
},"id":1}
```

Mark done when finished:
```json
{"jsonrpc":"2.0","method":"report_agent_activity","params":{
  "ts": 1700000060,
  "agent": "claude-code",
  "action": "edit",
  "file": "/abs/path/to/file.rs",
  "done": true
},"id":2}
```

### Notifications (push, no id)

After subscribing, the server pushes these to the client:

```json
{"jsonrpc":"2.0","method":"event","params":{...FileEvent...}}
{"jsonrpc":"2.0","method":"awareness_changed","params":{"project_id":"...","file_path":"...","awareness":{...}}}
{"jsonrpc":"2.0","method":"analysis_complete","params":{"project_id":"...","file_path":"...","line_count":42,"todo_count":3}}
{"jsonrpc":"2.0","method":"tree_patch","params":{"project_id":"...","patch":{...}}}
```

## Usage

```rust
use ambient_fs_server::{SocketServer, ServerState};
use std::sync::Arc;

let mut server = SocketServer::new("/tmp/ambient-fs.sock".into());
server.bind()?;

let state = Arc::new(ServerState::new("/path/to/events.db".into()));
server.set_state(state);

server.run().await?;
```

## Socket Path

Default: `/tmp/ambient-fs.sock`

## License

MIT
