# ambient-fs-server

Unix socket server for ambient-fs. JSON-RPC 2.0 protocol.

## Features

- **SocketServer** - Unix domain socket server
  - `bind(path)` - Bind socket to path
  - `run()` - Accept connections (async, blocking)
  - `shutdown()` - Graceful shutdown

- **JSON-RPC Protocol** - Request/response types
  - `Request { id, method, params }`
  - `Response { id, result, error }`
  - Methods: subscribe, events, awareness, watch_project

- **SubscriptionManager** - Live event broadcasts
  - `subscribe(project_id)` - Get event receiver
  - `broadcast(event)` - Send to all subscribers
  - Per-project tracking

## Protocol Methods

| Method     | Params                              | Returns               |
|------------|-------------------------------------|-----------------------|
| subscribe  | project_id                          | Stream of events      |
| events     | project_id, since, source, limit   | Array of events       |
| awareness  | project_id, path                   | FileAwareness         |
| watch      | path                                | success/error         |

## Usage

```rust
use ambient_fs_server::SocketServer;

let mut server = SocketServer::new("/tmp/ambient-fs.sock".into());
server.bind()?;
server.run().await?;
```

## Socket Path

Default: `/tmp/ambient-fs.sock`

## License

MIT
