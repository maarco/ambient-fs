# ambient-fs

A standalone filesystem awareness daemon. Watches project directories, logs every file event with source attribution, runs background content analysis, and exposes it all via Unix socket API.

Like Facebook's Watchman, but with:
- Event sourcing (append-only log in SQLite)
- Source attribution (user, AI agent, git, build, voice)
- Content analysis (TODO counting, import extraction, line counts)
- Multi-machine sync ready (future: gRPC gateway)

## Quick Start

```bash
# Install
cargo install ambient-fsd

# Start daemon
ambient-fsd start

# Watch a project
ambient-fsd watch /path/to/project

# Query events
ambient-fsd events --since 1h --source ai_agent

# Stop daemon
ambient-fsd stop
```

## Architecture

```
ambient-fsd (daemon)
    |
    +-- watcher (notify) -> event store (SQLite)
    |                      |
    |                      v
    +------------------ content analyzer
                            |
                        socket server (Unix socket)
```

## Crates

- **ambient-fs-core** - Pure types: FileEvent, FileAwareness, FileAnalysis, PathFilter
- **ambient-fs-store** - SQLite event store with WAL mode
- **ambient-fs-watcher** - Filesystem watching with notify 8, debouncing, blake3 dedup
- **ambient-fs-analyzer** - Content analysis (TODOs, imports, line counts)
- **ambient-fs-server** - Unix socket server with JSON-RPC protocol
- **ambient-fsd** - CLI daemon binary
- **ambient-fs-client** - Rust client library

## Protocol

Unix socket at `/tmp/ambient-fs.sock` with JSON-RPC 2.0:

```json
{"jsonrpc":"2.0","method":"subscribe","params":{"project_id":"my-project"},"id":1}
{"jsonrpc":"2.0","method":"events","params":{"since":3600},"id":2}
```

## License

MIT
