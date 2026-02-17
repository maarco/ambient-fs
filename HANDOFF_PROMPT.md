# Session Handoff: ambient-fs daemon build

You are continuing the build of the ambient-fs standalone filesystem awareness daemon.

## Context

Read these files first:
1. `CLAUDE.md` - Project overview, current state, dev rules
2. `docs/AMBIENT_FILESYSTEM_SPEC.md` - Full daemon spec
3. `docs/AMBIENT_FS_KOLLABOR_INTEGRATION.md` - Client integration spec

## What's Done

`ambient-fs-core` is complete with 61 passing tests. All 5 modules:
- event.rs, awareness.rs, analysis.rs, filter.rs, tree.rs

The workspace has 7 crates scaffolded, all compiling. Dependencies are
at latest versions (verified Feb 2026).

## What To Build Next

Build each crate using TDD (tests first, then implementation).
Follow the dependency order:

### 1. ambient-fs-store (SQLite event store)
- store.rs: EventStore struct - init db, insert events, query with filters
- migrations.rs: Schema versioning (CREATE TABLE file_events, file_analysis, projects, migrations)
- cache.rs: FileAnalysis cache table operations
- prune.rs: Retention policy (delete events older than N days)
- Use rusqlite 0.38 with bundled feature, WAL mode
- Schema is in the spec under "Event Store Schema"

### 2. ambient-fs-watcher (filesystem watching)
- watcher.rs: Wrap notify 8 crate, configurable debounce
- dedup.rs: blake3 content hashing, skip events if hash unchanged
- attribution.rs: Source detection (git via .git/index mtime, build via path patterns, client-reported, default=user)
- Use PathFilter from ambient-fs-core for ignore patterns

### 3. ambient-fs-analyzer (content analysis)
- analyzer.rs: FileAnalyzer trait, analyze(path) -> FileAnalysis
- languages.rs: Per-language configs (what to extract per file type)
- Start simple: line count, TODO/FIXME counting, basic import extraction
- ast-grep integration can come later

### 4. ambient-fs-server (daemon server)
- socket.rs: Unix domain socket server (tokio)
- protocol.rs: JSON-RPC request/response types
- subscriptions.rs: Live event subscriptions (broadcast channel)
- Wire together: store + watcher + analyzer

### 5. ambient-fsd (daemon binary)
- main.rs: clap CLI (start, stop, status, watch, unwatch, events, awareness)
- daemon.rs: Daemonize with PID file, signal handling (SIGTERM)
- config.rs: Load ~/.config/ambient-fs/config.toml

### 6. ambient-fs-client (client library)
- client.rs: Connect to Unix socket, send JSON-RPC, receive responses
- builder.rs: Fluent API for queries (events().project("x").since(1h).query())

## Rules

- TDD: Write failing tests first, then make them pass
- DRY: Core types come from ambient-fs-core, don't duplicate
- Security: Validate all file paths (no directory traversal), sanitize SQL inputs (rusqlite params)
- Don't touch anything in ../kollabor-app-v1/
- No emojis in code or output
- Keep tests focused and fast (use tempdir for filesystem tests)
- WAL mode for SQLite (concurrent reads during writes)
- All public APIs need tests
