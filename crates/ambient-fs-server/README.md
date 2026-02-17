# ambient-fs-server

Unix socket server for ambient-fs. JSON-RPC 2.0 protocol.

## Features

- **SocketServer** - Unix domain socket server
  - `bind(path)` - Bind socket to path
  - `set_state(state)` - Set shared ServerState
  - `run()` - Accept connections (async, blocking)
  - `shutdown()` - Graceful shutdown

- **JSON-RPC Protocol** - Request/response types
  - `Request { jsonrpc, method, params, id }`
  - `Response { jsonrpc, result/error, id }`
  - Methods: subscribe, unsubscribe, query_events, query_awareness, watch_project, unwatch_project

- **ServerState** - Shared state across connections
  - `store_path` - Path to SQLite database
  - `subscriptions` - SubscriptionManager for live events
  - `projects` - Project ID -> Path mapping

- **SubscriptionManager** - Live event broadcasts
  - `subscribe(project_id)` - Get broadcast::Receiver<FileEvent>
  - `unsubscribe(project_id)` - Decrement subscriber count
  - `broadcast(event)` - Send to all subscribers of a project
  - `subscriber_count(project_id)` - Get active subscriber count
  - Per-project broadcast channels

- **ConnectionState** - Per-connection subscription tracking
  - Tracks HashMap<project_id, broadcast::Receiver<FileEvent>>
  - `add_subscription(project_id, receiver)` - Add subscription
  - `remove_subscription(project_id)` - Remove and drop receiver
  - `has_subscriptions()` - Check if any active subscriptions

## Protocol Methods

| Method          | Params                              | Returns                                |
|-----------------|-------------------------------------|----------------------------------------|
| subscribe       | project_id (required)               | {subscribed: true, project_id: "..."}  |
| unsubscribe     | project_id (required)               | true                                   |
| query_events    | project_id, since, source, limit   | Array of FileEvent                     |
| query_awareness | project_id, path (required)        | FileAwareness or null                  |
| watch_project   | path (required)                     | {watching: true, project_id: "..."}    |
| unwatch_project | project_id (required)               | true                                   |

### Subscribe

Subscribe to receive events for a project:

```json
{"jsonrpc":"2.0","method":"subscribe","params":{"project_id":"my-project"},"id":1}
```

Response:
```json
{"jsonrpc":"2.0","result":{"subscribed":true,"project_id":"my-project"},"id":1}
```

Error (missing project_id):
```json
{"jsonrpc":"2.0","error":{"code":-32602,"message":"Invalid params"},"id":1}
```

### Unsubscribe

Unsubscribe from a project's events:

```json
{"jsonrpc":"2.0","method":"unsubscribe","params":{"project_id":"my-project"},"id":2}
```

Response:
```json
{"jsonrpc":"2.0","result":true,"id":2}
```

## Usage

```rust
use ambient_fs_server::{SocketServer, ServerState};
use std::sync::Arc;

let mut server = SocketServer::new("/tmp/ambient-fs.sock".into());
server.bind()?;

let state = Arc::new(ServerState::new("/tmp/events.db".into()));
server.set_state(state);

server.run().await?;
```

## Socket Path

Default: `/tmp/ambient-fs.sock`

## Implementation Status

- [x] subscribe - Fully implemented
- [x] unsubscribe - Fully implemented
- [ ] query_events - Stub (returns empty array)
- [ ] query_awareness - Stub (returns null)
- [ ] watch_project - Partial implementation
- [ ] unwatch_project - Partial implementation
- [ ] watch_agents - Stub
- [ ] unwatch_agents - Stub
- [ ] query_agents - Stub

## License

MIT
