# CLAUDE.md - ambient-fs

## What This Is

A standalone filesystem awareness daemon. Watches project directories,
logs every file event with source attribution, runs background content
analysis, and exposes it all via Unix socket API.

Like Facebook's Watchman, but with event sourcing, content analysis,
and multi-machine sync built in.

## Specs

Read these first:
- `docs/AMBIENT_FILESYSTEM_SPEC.md` - Full daemon spec (architecture, protocol, schema, stories)
- `docs/AMBIENT_FS_KOLLABOR_INTEGRATION.md` - How Kollabor connects as a client

## Current State

### Done
- Cargo workspace with 7 crates, all compiling
- `ambient-fs-core` fully implemented with 61 passing tests (TDD)
  - event.rs: FileEvent, EventType, Source (with serde, FromStr, builder pattern)
  - awareness.rs: FileAwareness, ChangeFrequency (with relative_time, from_age)
  - analysis.rs: FileAnalysis, ImportRef, LintHint, LintSeverity
  - filter.rs: PathFilter (glob-style ignore patterns, max file size)
  - tree.rs: TreeNode with add_node/remove_node/rename_node/find_node (sorted insertion, dirs-first)

### Not Started (in dependency order)
1. `ambient-fs-store` - SQLite event store (store.rs, migrations.rs, cache.rs, prune.rs)
2. `ambient-fs-watcher` - notify crate wrapper (watcher.rs, dedup.rs, attribution.rs)
3. `ambient-fs-analyzer` - Content analysis (analyzer.rs, languages.rs)
4. `ambient-fs-server` - Unix socket JSON-RPC server (socket.rs, protocol.rs, subscriptions.rs)
5. `ambient-fsd` - CLI binary (main.rs, daemon.rs, config.rs)
6. `ambient-fs-client` - Client library (client.rs, builder.rs)

## Development Rules

- TDD: Write tests FIRST, then implement
- DRY: No duplicate logic across crates
- Security: Validate all paths (no traversal), sanitize inputs
- No IO in ambient-fs-core (pure types + logic only)
- Don't touch the Kollabor app (../kollabor-app-v1/)

## Commands

```bash
cargo check                        # Type check workspace
cargo test                         # Run all tests
cargo test -p ambient-fs-core      # Run specific crate tests
cargo test -p ambient-fs-store     # etc.
cargo build                        # Build everything
cargo build -p ambient-fsd         # Build daemon binary only
```

## Crate Dependency Graph

```
ambient-fs-core (no deps, pure types)
  |
  +-> ambient-fs-store (+ rusqlite)
  +-> ambient-fs-watcher (+ notify, blake3)
  +-> ambient-fs-analyzer (+ tokio)
  +-> ambient-fs-client (+ tokio)
  |
  +-> ambient-fs-server (store + watcher + analyzer + tokio)
       |
       +-> ambient-fsd (server + clap + toml + tracing)
```

## Workspace Dependencies (verified Feb 2026)

```
serde 1, serde_json 1, tokio 1, thiserror 2, tracing 0.1,
tracing-subscriber 0.3, blake3 1, rusqlite 0.38 (bundled),
notify 8, clap 4, toml 1, uuid 1, chrono 0.4
```
