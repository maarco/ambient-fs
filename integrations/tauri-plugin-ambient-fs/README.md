# tauri-plugin-ambient-fs

Tauri plugin for bridging [ambient-fsd](../../crates/ambient-fsd) to Tauri applications.

## Features

- Watch project directories for file events
- Query file awareness (change frequency, attribution, analysis)
- Attribute changes to sources (user, AI agents, git, etc.)
- Query active agents
- Real-time event forwarding to frontend

## Installation

### Rust (in `src-tauri/Cargo.toml`)

```toml
[dependencies]
tauri-plugin-ambient-fs = { path = "../../ambient-fs/integrations/tauri-plugin-ambient-fs" }
```

### TypeScript (in `package.json`)

```json
{
  "dependencies": {
    "@ambient-fs/tauri-plugin": "file:../ambient-fs/integrations/tauri-plugin-ambient-fs/guest-js"
  }
}
```

## Usage

### Initialize Plugin

```rust,no_run
// src-tauri/src/main.rs
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_ambient_fs::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

### Frontend API

```typescript
import * as AmbientFs from '@ambient-fs/tauri-plugin';

// Watch a project
const projectId = await AmbientFs.watchProject('/home/user/project');

// Query events
const events = await AmbientFs.queryEvents({
    project_id: projectId,
    limit: 100
});

// Query awareness for a file
const awareness = await AmbientFs.queryAwareness(projectId, 'src/main.rs');

// Attribute a change
await AmbientFs.attribute(projectId, 'src/auth.rs', 'ai_agent', 'chat-42');

// Listen to events
const unlisten = AmbientFs.onFileEvent((event) => {
    console.log('file changed:', event.file_path, event.event_type);
});

// Cleanup
unlisten();
```

### Vue Composable Example

```typescript
// composables/useAmbientFs.ts
import { ref } from 'vue';
import * as AmbientFs from '@ambient-fs/tauri-plugin';

export function useAmbientFs() {
    const isConnected = ref(false);
    const projectAwareness = ref(new Map<string, AmbientFs.FileAwareness>());

    // Connection status
    const unlistenConnected = AmbientFs.onConnectedChanged((connected) => {
        isConnected.value = connected;
    });

    // Awareness updates
    const unlistenAwareness = AmbientFs.onAwarenessChanged(({ file_path, awareness }) => {
        projectAwareness.value.set(file_path, awareness);
    });

    // Cleanup on unmount
    onUnmounted(() => {
        unlistenConnected();
        unlistenAwareness();
    });

    return {
        isConnected,
        projectAwareness,
        watchProject: AmbientFs.watchProject,
        queryAwareness: AmbientFs.queryAwareness,
        attribute: AmbientFs.attribute,
    };
}
```

## Configuration

The plugin reads the socket path from the `AMBIENT_FS_SOCKET` environment variable, defaulting to `/tmp/ambient-fs.sock`.

```bash
export AMBIENT_FS_SOCKET=/tmp/ambient-fs.sock
```

## Features

- `auto-launch` (default): Automatically spawn the daemon if not running

## Events

Events are emitted to the frontend:

- `ambient-fs://connected` - Connection status changed
- `ambient-fs://event` - File event occurred
- `ambient-fs://awareness-changed` - File awareness updated
- `ambient-fs://analysis-complete` - File analysis completed

## IPC Commands

- `watch_project` - Start watching a directory
- `unwatch_project` - Stop watching a project
- `query_events` - Query events with filter
- `query_awareness` - Query awareness for a file
- `query_tree` - Query file tree for a project
- `attribute` - Attribute a change to a source
- `query_agents` - Query active agents
- `get_status` - Get daemon connection status

## Development

```bash
# Build plugin
cargo build --package tauri-plugin-ambient-fs

# Run tests
cargo test --package tauri-plugin-ambient-fs
```

## License

MIT
