# ambient-fs-client

Rust client library for ambient-fsd. Connects via Unix socket.

## Features

- **AmbientFsClient** - Connect to daemon
  - `connect(path)` - Connect to socket
  - `connect_local()` - Use default socket path
  - `watch(path)` - Add project to watch
  - `events(filter)` - Query events
  - `subscribe(project_id)` - Live event stream

## Usage

```rust
use ambient_fs_client::AmbientFsClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = AmbientFsClient::connect_local().await?;

    // Watch a project
    client.watch("/path/to/project").await?;

    // Query events
    let events = client.events()
        .project("my-project")
        .since(Duration::from_secs(3600))
        .query().await?;

    // Subscribe to live events
    let mut stream = client.subscribe("my-project").await?;
    while let Some(event) = stream.next().await {
        println!("{:?}", event);
    }

    Ok(())
}
```

## Socket Path

Default: `/tmp/ambient-fs.sock`

## License

MIT
