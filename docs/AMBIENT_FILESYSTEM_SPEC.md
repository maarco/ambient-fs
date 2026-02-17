# ambient-fs: Standalone Filesystem Awareness Daemon

## Overview

A standalone daemon that watches project directories, logs every filesystem
event with attribution, runs background content analysis, and exposes it all
via a client API. Any app (Kollabor, VS Code plugin, CLI tool, remote agent)
connects to the daemon and gets live filesystem intelligence.

This is not an embedded library. It's a service that runs independently.
Like Watchman, but with event sourcing, content analysis, and multi-machine
sync built in.

```
$ ambient-fsd start
$ ambient-fsd watch ~/dev/my-project
$ ambient-fsd events --since="1h" --source=ai_agent

  10:32  ai_agent  created   src/api/routes.ts      (chat: "Fix auth")
  10:33  user      modified  README.md
  10:41  git       added     48 files                (git pull main)
```


## System Architecture

```
+-----------------------------------------------------------+
|                    ambient-fsd (daemon)                     |
|                                                             |
|  +-------------+  +---------------+  +------------------+  |
|  |  Watcher    |  |  Event Store  |  |  Content         |  |
|  |  (notify)   |->|  (SQLite)     |->|  Analyzer        |  |
|  |             |  |               |  |  (ast-grep+grep) |  |
|  +-------------+  +-------+-------+  +--------+---------+  |
|                            |                   |            |
|                    +-------v-------------------v-------+    |
|                    |     Awareness Aggregator          |    |
|                    +---------------+-------------------+    |
|                                    |                        |
|                    +---------------v-------------------+    |
|                    |     Server (Unix Socket + gRPC)   |    |
|                    +-----------------------------------+    |
+-----------------------------------------------------------+
          |                    |                    |
          v                    v                    v
    +----------+        +----------+        +-----------+
    | Kollabor |        | CLI tool |        | VS Code   |
    | (Tauri)  |        | ambient  |        | extension |
    +----------+        +----------+        +-----------+
```


## Daemon Process

### Lifecycle

```
ambient-fsd start       start daemon (background, auto-restarts)
ambient-fsd stop        graceful shutdown
ambient-fsd status      show running state, watched projects, stats
ambient-fsd watch PATH  add project to watch list
ambient-fsd unwatch ID  remove project
ambient-fsd events      query event log (filters, since, source)
ambient-fsd awareness   query file awareness state
ambient-fsd analyze     trigger manual analysis of a file/project
ambient-fsd sync        show remote sync status (future)
```

### Auto-start

On macOS: launchd plist for auto-start on login.
On Linux: systemd user service.
Config at `~/.config/ambient-fs/config.toml`.

### Config

```toml
# ~/.config/ambient-fs/config.toml

[daemon]
socket_path = "/tmp/ambient-fs.sock"   # Unix socket for local clients
grpc_port = 0                          # 0 = disabled, set for remote access
log_level = "info"
pid_file = "/tmp/ambient-fs.pid"

[store]
path = "~/.local/share/ambient-fs/events.db"
retention_days = 90                    # prune events older than this
wal_mode = true

[watcher]
debounce_ms = 100                      # batch rapid events
ignore_patterns = [
  ".git", "node_modules", "target", "dist",
  ".next", "__pycache__", ".DS_Store",
  "*.swp", "*.tmp", "*.pyc",
]
build_output_patterns = [              # auto-attributed as source=build
  "dist/", "build/", "target/release/",
  "target/debug/", ".next/", ".nuxt/",
]
max_file_size_bytes = 10_485_760       # skip files > 10MB

[analyzer]
enabled = true
max_concurrent = 2                     # background analysis threads
max_file_size_bytes = 1_048_576        # skip analysis for files > 1MB
languages = ["typescript", "javascript", "rust", "python", "vue", "markdown"]

[sync]
enabled = false                        # future: remote sync
machine_id = ""                        # auto-generated on first run
```


## Crate Structure

```
ambient-fs/                            workspace root (standalone repo)
  Cargo.toml                           workspace definition
  LICENSE                              MIT

  crates/
    ambient-fs-core/                   pure types + logic, no IO
      src/
        lib.rs
        event.rs                       FileEvent, EventType, Source
        awareness.rs                   FileAwareness, ChangeFrequency
        analysis.rs                    FileAnalysis, ImportRef, LintHint
        filter.rs                      path filtering (ignore patterns)
        tree.rs                        incremental tree ops (add/remove/rename)

    ambient-fs-store/                  SQLite event store
      src/
        lib.rs
        store.rs                       EventStore (write events, query)
        migrations.rs                  schema versioning
        cache.rs                       file_analysis cache table
        prune.rs                       retention policy, cleanup

    ambient-fs-watcher/                filesystem watching
      src/
        lib.rs
        watcher.rs                     notify integration + debounce
        dedup.rs                       blake3 content hashing
        attribution.rs                 source detection (git, build, etc.)

    ambient-fs-analyzer/               content analysis engine
      src/
        lib.rs
        analyzer.rs                    FileAnalyzer trait
        ast_grep.rs                    ast-grep-core integration
        grep.rs                        grep crate (ripgrep) integration
        languages.rs                   per-language analysis configs

    ambient-fs-server/                 daemon server
      src/
        lib.rs
        socket.rs                      Unix socket server
        grpc.rs                        gRPC server (tonic)
        protocol.rs                    request/response types
        subscriptions.rs               live event subscriptions

    ambient-fsd/                       daemon binary
      src/
        main.rs                        CLI entry (clap)
        daemon.rs                      daemonize, signal handling
        config.rs                      config.toml loading

    ambient-fs-client/                 client library (Rust)
      src/
        lib.rs
        client.rs                      connect, query, subscribe
        builder.rs                     fluent API

  integrations/
    tauri-plugin-ambient-fs/           Tauri plugin for Kollabor
      src/
        lib.rs                         plugin init, command handlers
        bridge.rs                      daemon client -> Tauri IPC events

    ambient-fs-node/                   Node.js bindings (napi-rs)
      src/
        lib.rs

    ambient-fs-python/                 Python bindings (PyO3)
      src/
        lib.rs
```


## Client Protocol

### Unix Socket (primary, local)

JSON-RPC over Unix domain socket. Low latency, zero config.

```
// Request
{"jsonrpc":"2.0","method":"subscribe","params":{"project_id":"abc"},"id":1}

// Response (stream)
{"jsonrpc":"2.0","result":{"type":"event","data":{"event_type":"created","file_path":"src/new.ts","source":"ai_agent","source_id":"chat_42","timestamp":1708100000}}}
```

### gRPC (remote, future)

Protocol buffers for cross-machine communication.

```protobuf
syntax = "proto3";
package ambient_fs;

service AmbientFs {
  // One-shot queries
  rpc GetAwareness (AwarenessRequest) returns (AwarenessResponse);
  rpc GetEvents (EventsRequest) returns (EventsResponse);
  rpc WatchProject (WatchRequest) returns (WatchResponse);

  // Streaming
  rpc Subscribe (SubscribeRequest) returns (stream FileEvent);
  rpc SubscribeAwareness (SubscribeRequest) returns (stream FileAwareness);

  // Remote sync
  rpc SyncEvents (stream FileEvent) returns (stream FileEvent);
}

message FileEvent {
  int64  timestamp = 1;
  string event_type = 2;   // created/modified/deleted/renamed
  string file_path = 3;
  string project_id = 4;
  string source = 5;       // user/ai_agent/git/build/voice
  string source_id = 6;
  string machine_id = 7;
  string content_hash = 8;
  string old_path = 9;     // renames only
}

message FileAwareness {
  string file_path = 1;
  string project_id = 2;
  int64  last_modified = 3;
  string modified_by = 4;
  string modified_by_label = 5;
  string active_agent = 6;
  int32  chat_references = 7;
  int32  todo_count = 8;
  int32  lint_hints = 9;
  int32  line_count = 10;
  string change_frequency = 11; // hot/warm/cold
}
```


## Event Store Schema

SQLite database at path from config (default: ~/.local/share/ambient-fs/events.db)

```sql
-- Core event log (append-only)
CREATE TABLE file_events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  timestamp   INTEGER NOT NULL,
  event_type  TEXT    NOT NULL,       -- created/modified/deleted/renamed
  file_path   TEXT    NOT NULL,       -- relative to project root
  project_id  TEXT    NOT NULL,
  source      TEXT    NOT NULL DEFAULT 'user',
  source_id   TEXT,
  machine_id  TEXT    NOT NULL,
  content_hash TEXT,
  old_path    TEXT
);

CREATE INDEX idx_events_project_path ON file_events(project_id, file_path);
CREATE INDEX idx_events_project_time ON file_events(project_id, timestamp DESC);
CREATE INDEX idx_events_source ON file_events(project_id, source);
CREATE INDEX idx_events_machine ON file_events(machine_id, timestamp DESC);

-- Content analysis cache
CREATE TABLE file_analysis (
  file_path    TEXT NOT NULL,
  project_id   TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  analyzed_at  INTEGER NOT NULL,
  exports      TEXT,                   -- JSON array
  imports      TEXT,                   -- JSON array
  todo_count   INTEGER DEFAULT 0,
  lint_hints   TEXT,                   -- JSON array
  line_count   INTEGER DEFAULT 0,
  PRIMARY KEY (project_id, file_path)
);

-- Watched projects
CREATE TABLE projects (
  id           TEXT PRIMARY KEY,
  path         TEXT NOT NULL UNIQUE,
  name         TEXT NOT NULL,
  added_at     INTEGER NOT NULL,
  active       INTEGER NOT NULL DEFAULT 1
);

-- Schema version tracking
CREATE TABLE migrations (
  version      INTEGER PRIMARY KEY,
  applied_at   INTEGER NOT NULL
);
```

### Attribution Logic

```
source detection priority (highest to lowest):

  1. explicit attribution
     client sends source + source_id with the event
     used by: Kollabor tool_runner (ai_agent + chat_id)
              voice pipeline (voice)

  2. git detection
     if event path is inside .git/ or event follows
     a git command (detected via .git/index mtime change)
     source = "git"

  3. build detection
     if file_path matches build_output_patterns from config
     source = "build"

  4. default
     source = "user"
```


## Client Libraries

### Rust Client

```rust
use ambient_fs_client::AmbientFsClient;

#[tokio::main]
async fn main() -> Result<()> {
    let client = AmbientFsClient::connect_local().await?;

    // Watch a project
    client.watch("/path/to/project").await?;

    // Query events
    let events = client.events()
        .project("my-project")
        .since(Duration::from_secs(3600))
        .source("ai_agent")
        .query().await?;

    // Subscribe to live events
    let mut stream = client.subscribe("my-project").await?;
    while let Some(event) = stream.next().await {
        println!("{}: {} {} by {}",
            event.file_path, event.event_type,
            event.timestamp, event.source);
    }

    // Get file awareness
    let awareness = client.awareness("my-project", "src/auth.ts").await?;
    println!("modified {}s ago by {}", awareness.age_secs(), awareness.modified_by);

    // Report attribution (from Kollabor tool_runner)
    client.attribute()
        .project("my-project")
        .file("src/auth.ts")
        .source("ai_agent")
        .source_id("chat_42")
        .send().await?;

    Ok(())
}
```

### CLI

```bash
# query events from last hour
$ ambient events --since=1h
  10:32  ai_agent  created   my-project/src/routes.ts
  10:33  user      modified  my-project/README.md

# filter by source
$ ambient events --source=ai_agent --since=today

# get awareness for a file
$ ambient awareness my-project src/auth.ts
  last modified: 2 minutes ago
  modified by:   ai_agent (chat: "Fix auth flow")
  references:    3 chats
  todos:         1
  lint hints:    0
  lines:         142
  frequency:     hot

# show project stats
$ ambient stats my-project
  files watched:  847
  events today:   234
  by source:
    user:         189
    ai_agent:     31
    git:          12
    build:        2
```

### Node.js Client (future)

```javascript
const { AmbientFs } = require('ambient-fs');

const client = await AmbientFs.connect();

// Subscribe to events
const stream = client.subscribe('my-project');
stream.on('event', (event) => {
  console.log(`${event.filePath}: ${event.eventType} by ${event.source}`);
});

// Query awareness
const awareness = await client.awareness('my-project', 'src/auth.ts');
```

### Python Client (future)

```python
from ambient_fs import AmbientFs

client = AmbientFs.connect()

# Query events
events = client.events(project="my-project", since="1h", source="ai_agent")
for event in events:
    print(f"{event.file_path}: {event.event_type} by {event.source}")
```


## Kollabor Integration

Kollabor connects to ambient-fsd via a Tauri plugin.

### Plugin Architecture

```
Kollabor (Tauri)
  |
  v
tauri-plugin-ambient-fs
  |
  +--> ambient-fs-client (connects to daemon)
  |
  +--> IPC bridge (daemon events -> Tauri window events)
  |
  +--> attribution bridge (tool_runner -> daemon attribution API)
```

### Plugin Setup

```rust
// kollabor: src-tauri/Cargo.toml
[dependencies]
tauri-plugin-ambient-fs = { path = "../ambient-fs/integrations/tauri-plugin-ambient-fs" }

// kollabor: src-tauri/src/main.rs
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_ambient_fs::init())
        .run(tauri::generate_context!())
}
```

### Frontend (Vue)

```typescript
// composable wrapping the Tauri plugin
import { useAmbientFs } from '@/composables/useAmbientFs';

const { awareness, events, subscribe } = useAmbientFs();

// Get awareness for visible tree nodes
const fileAwareness = await awareness.getProject('my-project');

// Subscribe to live events
subscribe('my-project', (event) => {
  // incremental tree patch
  if (event.eventType === 'created') addTreeNode(event.filePath);
  if (event.eventType === 'deleted') removeTreeNode(event.filePath);
});
```

### FileTreeNode Rendering (Kollabor-specific)

```
auth.ts  ◉ 2m  ✦3  ⚠1
         |     |   |
         |     |   +-- 1 lint hint (orange)
         |     +------ 3 chats reference this (cyan)
         +------------ modified 2m ago, dot color = source
                       green=user, purple=ai, blue=git

routes.ts  ◉ claude editing...     (pulsing purple dot)
notes.md   ◉ transcribing...      (pulsing green dot)
```


## Remote Sync (Phase 5)

```
machine A                 gateway                machine B
+-----------+            +---------+            +-----------+
| ambient-  |  <--gRPC-->| ambient |<--gRPC-->  | ambient-  |
| fsd       |            | -gateway|            | fsd       |
| (local)   |            | (relay) |            | (remote)  |
+-----------+            +---------+            +-----------+
     |                                               |
     v                                               v
  Kollabor                                      AI Agent
  (your mac)                                    (cloud VM)
```

Sync protocol:
- Each daemon has a unique machine_id
- Events tagged with machine_id at creation
- SyncEvents gRPC stream: bidirectional event exchange
- Each daemon only sends events the other hasn't seen
  (track last-seen timestamp per remote machine_id)
- Append-only = no conflicts, naturally mergeable
- Gateway is optional: daemons can sync peer-to-peer


## Beads Stories

### Epic: kollabor-app-v1-xz7 (Ambient Filesystem Awareness System)

---

### Phase 0: Daemon Skeleton

**P0-1: Create ambient-fs workspace and crate structure**
- Create standalone repo or workspace directory
- Set up Cargo workspace with all crates (empty stubs)
- ambient-fs-core, ambient-fs-store, ambient-fs-watcher,
  ambient-fs-analyzer, ambient-fs-server, ambient-fsd,
  ambient-fs-client
- Add MIT license, basic README
- Priority: P1

**P0-2: Implement daemon lifecycle (ambient-fsd)**
- clap CLI: start, stop, status, watch, unwatch, events
- Daemonize with PID file
- Signal handling (SIGTERM graceful shutdown)
- Config loading from ~/.config/ambient-fs/config.toml
- Priority: P1

**P0-3: Implement Unix socket server**
- JSON-RPC over Unix domain socket
- Accept connections, parse requests, route to handlers
- Connection lifecycle (connect, subscribe, disconnect)
- Priority: P1

---

### Phase 1: Event Store + Watcher

**P1-1: Create SQLite event store (ambient-fs-store)**
- Initialize events.db with schema
- WAL mode for concurrent reads during writes
- Migration system for future schema changes
- Write event API (insert, batch insert)
- Query API (by project, by path, by source, by time range)
- Priority: P1

**P1-2: Wire notify watcher to event store**
- ambient-fs-watcher: wrap notify crate
- Configurable ignore patterns from config.toml
- Debounce rapid events (configurable ms)
- On event: write to store + broadcast to subscribers
- Priority: P1

**P1-3: Add blake3 content deduplication**
- Hash file content on modify events
- Skip event if content_hash unchanged
- blake3 for speed (inline on every modify, ~1GB/s)
- Priority: P1

**P1-4: Implement source attribution**
- Auto-detect git changes (watch .git/index mtime)
- Auto-detect build output (match config patterns)
- Client-reported attribution API (for Kollabor tool_runner)
- Default to source=user
- Priority: P1

**P1-5: Add CLI query commands**
- ambient events: query event log with filters
- ambient stats: project statistics
- ambient status: daemon health, watched projects
- Priority: P2

---

### Phase 2: Content Analysis

**P2-1: Integrate ast-grep-core**
- ambient-fs-analyzer crate
- FileAnalyzer trait: analyze(path) -> FileAnalysis
- tree-sitter parsing for: TS, JS, Rust, Python, Vue, Markdown
- Extract: exports, imports, TODOs, lint hints, line count
- Priority: P2

**P2-2: Create analysis cache**
- file_analysis table in events.db
- Keyed by (project_id, file_path, content_hash)
- Auto-invalidate when hash changes
- Priority: P2

**P2-3: Wire analysis to file events**
- On create/modify: check hash, spawn background analysis
- Throttle: max N concurrent (from config)
- Skip files > max_file_size_bytes
- Broadcast analysis-complete to subscribers
- Priority: P2

**P2-4: Add cross-reference indexing (grep crate)**
- Use ripgrep grep crate for import/reference scanning
- Build file->file reference index
- Incremental: only re-scan changed file's imports
- Priority: P3

---

### Phase 3: Awareness Aggregator

**P3-1: Implement awareness aggregation**
- Combine: latest event + analysis cache + active agents
- Produce FileAwareness per file
- Compute changeFrequency (hot/warm/cold)
- Priority: P2

**P3-2: Add awareness query API**
- get_awareness(project, path) -> FileAwareness
- get_project_awareness(project) -> Map<path, FileAwareness>
- Serve via Unix socket + gRPC
- Priority: P2

**P3-3: Add awareness subscription**
- Clients subscribe to awareness changes for a project
- Server pushes FileAwareness diffs (only changed files)
- Priority: P2

---

### Phase 4: Kollabor Integration

**P4-1: Create tauri-plugin-ambient-fs**
- Tauri plugin that connects to ambient-fsd
- Start daemon if not running (auto-launch)
- Bridge daemon events -> Tauri IPC events
- Priority: P2

**P4-2: Create useAmbientFs composable**
- Vue composable wrapping the Tauri plugin
- Subscribe to project events
- Expose reactive FileAwareness map
- Priority: P2

**P4-3: Implement incremental tree patching**
- On file-created event: add node to project tree
- On file-deleted event: remove node from tree
- On file-renamed event: update node in-place
- Keep loadProjectTree as manual refresh fallback
- Priority: P2

**P4-4: Add awareness overlay to FileTreeNode**
- Render indicators after filename
- Source dot (color-coded), relative time
- Pulsing dot for active agents
- Priority: P2

**P4-5: Add hover tooltip with awareness detail**
- Full modification info, chat refs, line count, TODOs
- Priority: P3

**P4-6: Wire tool_runner attribution**
- When tool_runner.rs writes/creates a file:
  - Call ambient-fs-client attribute API
  - Pass source=ai_agent, source_id=chat_id
- When voice pipeline creates file: source=voice
- Priority: P2

---

### Phase 5: Activity Feed (Kollabor)

**P5-1: Create ActivityFeedPanel component**
- Vue component showing event stream from daemon
- Chronological feed: timestamp, source icon, event, path
- Priority: P3

**P5-2: Add feed filtering**
- By source, project, time range
- "What happened while I was away" preset
- Priority: P3

**P5-3: Register activity feed tab**
- Add to tab-registry.ts, available for all content types
- Priority: P3

---

### Phase 6: Remote Sync (future)

**P6-1: Implement gRPC server**
- tonic-based gRPC server in ambient-fs-server
- Expose all query + subscription APIs
- Priority: P4

**P6-2: Implement SyncEvents stream**
- Bidirectional event exchange between daemons
- Track last-seen per remote machine_id
- Append remote events to local store
- Priority: P4

**P6-3: Build ambient-gateway relay**
- Optional relay service for NAT traversal
- Daemons connect to gateway, gateway routes events
- Priority: P4

**P6-4: Remote awareness indicators in Kollabor**
- Distinguish local vs remote in FileTreeNode
- Different colors for remote agent activity
- Priority: P4

---

### Dependencies

```
Phase 0 (skeleton):
  P0-1 ──> P0-2 ──> P0-3

Phase 1 (events):
  P0-3 ──> P1-1 ──> P1-2 ──> P1-3
                       |
                       v
                     P1-4
                       |
                       v
                     P1-5

Phase 2 (analysis):
  P1-1 ──> P2-1 ──> P2-2 ──> P2-3 ──> P2-4

Phase 3 (awareness):
  P1-4 + P2-3 ──> P3-1 ──> P3-2 ──> P3-3

Phase 4 (kollabor):
  P0-3 + P3-3 ──> P4-1 ──> P4-2 ──> P4-3 ──> P4-4 ──> P4-5
                    |
                    v
                  P4-6

Phase 5 (feed):
  P4-2 ──> P5-1 ──> P5-2 ──> P5-3

Phase 6 (remote):
  P3-3 ──> P6-1 ──> P6-2 ──> P6-3 ──> P6-4
```

Parallelism opportunities:
- Phase 1 and Phase 2 share P1-1 (store) then diverge
- Phase 3 waits for both P1 and P2
- Phase 4 can start P4-1 as soon as P0-3 is done (mock data)
- Phase 6 is fully independent once Phase 3 is stable


## Risk Assessment

| Risk                                     | Mitigation                            |
|------------------------------------------|---------------------------------------|
| Daemon adds process overhead             | Minimal: idle daemon ~5MB RSS         |
| Socket communication latency             | Unix socket: sub-ms local IPC         |
| Daemon crash loses in-flight events      | WAL mode survives crashes, events     |
|                                          | are append-only (no partial writes)   |
| ast-grep slow on large files             | Skip files > 1MB, background threads  |
| Too many file events (npm install)       | Debounce + blake3 dedup + ignore list |
| gRPC complexity for remote sync          | Phase 6 is future, Unix socket first  |
| Multiple apps writing attribution at once| SQLite WAL handles concurrent writes  |
| Daemon not running when Kollabor starts  | Tauri plugin auto-launches daemon     |


## Open Questions

1. Should the daemon manage its own git integration (watch for
   commits, parse git log) or should clients report git events?

2. Should file_analysis include full AST data or just summary
   metrics? Full AST enables richer queries but larger cache.

3. For remote sync, should events include file content diffs
   or just metadata? Content sync is a different problem
   (more like rsync/syncthing territory).

4. Should the daemon support plugins/hooks for custom
   attribution sources? (e.g., Docker, CI/CD pipelines)

5. Naming: ambient-fs vs something catchier for the open-source
   release?
