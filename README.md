# ambient-fs

A standalone filesystem awareness daemon. Watches project directories, logs every file event with source attribution, runs background content analysis, and exposes it all via Unix socket API.

Like Facebook's Watchman, but with:
- Event sourcing (append-only log in SQLite)
- Source attribution (user, AI agent, git, build, voice)
- Content analysis (TODO counting, import extraction, line counts)
- Multi-machine sync via gRPC gateway
- Agent activity tracking and AI source detection

## Quick Start

```bash
# Install
cargo install ambient-fsd

# Watch a project
ambient-fsd watch /path/to/project

# Start daemon
ambient-fsd start

# Query events
ambient-fsd events --since 1h --source ai_agent

# Get file awareness
ambient-fsd awareness <project-id> /path/to/file.rs

# Stop daemon
ambient-fsd stop
```

## Architecture

```
ambient-fsd (daemon)
    |
    +-- FsWatcher (notify) ---------> EventStore (SQLite)
    |                                       |
    +-- AgentTracker (JSONL)                v
    |                              SubscriptionManager
    +-- ContentAnalyzer                     |
    |                              Unix socket server
    +-- gRPC Gateway          (JSON-RPC 2.0 protocol)
         (cross-machine sync)
```

## Crates

- `ambient-fs-core` - Pure types: FileEvent, FileAwareness, FileAnalysis, PathFilter, TreeNode
- `ambient-fs-store` - SQLite event store with WAL mode and project registry
- `ambient-fs-watcher` - Filesystem watching with notify 8, debouncing, blake3 dedup
- `ambient-fs-analyzer` - Content analysis: line counts, TODOs, LLM-enhanced imports/exports
- `ambient-fs-server` - Unix socket JSON-RPC server, subscription manager, agent tracking
- `ambient-fsd` - CLI daemon binary
- `ambient-fs-client` - Async Rust client library

## Integrations

- `integrations/ambient-fs-python` - Async/sync Python client
- `integrations/ambient-fs-node` - TypeScript/Node.js client
- `integrations/tauri-plugin-ambient-fs` - Tauri 2 plugin with frontend API

## Protocol

Unix socket at `/tmp/ambient-fs.sock`, JSON-RPC 2.0:

| Method                 | Description                              |
|------------------------|------------------------------------------|
| watch_project          | Start watching a directory               |
| unwatch_project        | Stop watching a project                  |
| subscribe              | Subscribe to live events for a project   |
| unsubscribe            | Stop subscription                        |
| query_events           | Query event log with filters             |
| query_awareness        | Get aggregated file state                |
| query_tree             | Get directory tree for a project         |
| watch_agents           | Register agent activity directory        |
| unwatch_agents         | Unregister agent activity directory      |
| query_agents           | Get active AI agents                     |
| report_agent_activity  | Inject agent activity for AI attribution |
| attribute              | Manually attribute a change to a source  |

## Source Attribution

Every event carries a `source` field:
- `user` - Human typed/saved the file
- `ai_agent` - AI tool edited the file (detected via AgentTracker)
- `git` - Git operation (merge, rebase, checkout)
- `build` - Build tool output (target/, dist/, .next/)
- `voice` - Voice coding tool

## AI Agent Detection

When an AI agent is editing a file, call `report_agent_activity` via the socket:

```json
{"jsonrpc":"2.0","method":"report_agent_activity","params":{
  "ts": 1234567890,
  "agent": "claude-code",
  "action": "edit",
  "file": "/abs/path/to/file.rs",
  "tool": "claude-code",
  "intent": "fixing the bug"
},"id":1}
```

Any filesystem events on that file while the agent is active are stored with `source: ai_agent`.
Mark the agent done with `"done": true` in a follow-up call.

## Config

`~/.config/ambient-fs/config.toml`

```toml
[daemon]
socket_path = "/tmp/ambient-fs.sock"
log_level = "info"

[store]
path = "~/.local/share/ambient-fs/events.db"
retention_days = 90

[watcher]
debounce_ms = 100
```

## Tests

661 tests across 7 crates, 0 failures.

```bash
cargo test
```

## License

MIT
