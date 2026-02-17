# ambient-fs-core

Core types for ambient-fs. Pure data structures with no IO.

## Types

- **FileEvent** - A filesystem event with attribution
  - `timestamp`, `event_type`, `file_path`, `project_id`
  - `source` (user, ai_agent, git, build, voice)
  - `source_id`, `machine_id`, `content_hash`

- **FileAwareness** - Aggregated per-file state
  - Last modified, modified by, active agent
  - TODO count, lint hints, line count
  - Change frequency (hot, warm, cold)

- **FileAnalysis** - Content analysis results
  - Imports, exports, TODO count, lint hints
  - Line count, content hash for cache invalidation

- **PathFilter** - Glob-style ignore patterns
  - Default ignores: .git, node_modules, target, dist, etc.
  - Max file size checking

- **TreeNode** - Incremental tree operations
  - `add_node`, `remove_node`, `rename_node`, `find_node`

## Usage

```rust
use ambient_fs_core::event::{FileEvent, EventType, Source};

let event = FileEvent::new(EventType::Created, "src/main.rs", "my-project", "machine-1")
    .with_source(Source::AiAgent)
    .with_source_id("chat_42")
    .with_content_hash("abc123");
```

## License

MIT
