# ambient-fs: Kollabor Integration Spec

## Overview

This spec covers how Kollabor connects to the ambient-fsd daemon.
For the daemon itself, see AMBIENT_FILESYSTEM_SPEC.md.

Kollabor becomes a CLIENT of ambient-fsd. It does not embed the
filesystem watching, event storage, or content analysis directly.
Instead, it connects via Unix socket and receives events + awareness
data from the daemon.


## Integration Architecture

```
ambient-fsd (separate process)
  |
  |  Unix socket (/tmp/ambient-fs.sock)
  |
  v
tauri-plugin-ambient-fs (Rust, in Kollabor's Tauri backend)
  |
  |  Tauri IPC events
  |
  v
useAmbientFs.ts (Vue composable)
  |
  |  reactive state
  |
  v
UniversalSidebar / FileTreeNode (UI)
```


## Tauri Plugin: tauri-plugin-ambient-fs

Lives at: ambient-fs/integrations/tauri-plugin-ambient-fs/

### Responsibilities

1. Auto-launch ambient-fsd if not running
2. Connect to daemon via Unix socket
3. Register Kollabor's projects with the daemon
4. Bridge daemon events to Tauri window events
5. Bridge Kollabor's tool_runner attribution to daemon
6. Expose IPC commands for frontend queries

### Rust API

```rust
// src-tauri/Cargo.toml
[dependencies]
tauri-plugin-ambient-fs = { path = "../ambient-fs/integrations/tauri-plugin-ambient-fs" }

// src-tauri/src/main.rs
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_ambient_fs::init())
        .run(tauri::generate_context!())
}
```

### IPC Commands (registered by plugin)

```
Frontend -> Backend:

  ambient_fs_watch_project(path)
    Register a project directory with the daemon.
    Returns project_id.

  ambient_fs_unwatch_project(project_id)
    Stop watching a project.

  ambient_fs_get_awareness(project_id, file_path)
    Get FileAwareness for a single file.

  ambient_fs_get_project_awareness(project_id)
    Get FileAwareness for all files in project.
    Returns Map<file_path, FileAwareness>.

  ambient_fs_get_events(project_id, filters)
    Query event log. Filters: since, source, file_path.
    Returns FileEvent[].

  ambient_fs_attribute(project_id, file_path, source, source_id)
    Report source attribution for a file change.
    Called by tool_runner when AI modifies files.

  ambient_fs_status()
    Get daemon status (running, watched projects, stats).
```

### IPC Events (emitted by plugin)

```
Backend -> Frontend:

  ambient-fs-event
    Payload: FileEvent
    Emitted for every file event in watched projects.

  ambient-fs-awareness-changed
    Payload: { projectId, filePath, awareness: FileAwareness }
    Emitted when a file's awareness data changes.

  ambient-fs-analysis-complete
    Payload: { projectId, filePath, analysis: FileAnalysis }
    Emitted when background analysis finishes for a file.

  ambient-fs-connected
    Payload: { status: "connected" | "disconnected" }
    Emitted when connection to daemon changes.
```


## Vue Composable: useAmbientFs

Location: src-vue/src/composables/useAmbientFs.ts

### API

```typescript
import { useAmbientFs } from '@/composables/useAmbientFs';

const {
  // State
  isConnected,                    // ref<boolean>
  projectAwareness,               // ref<Map<string, FileAwareness>>

  // Actions
  watchProject,                   // (path: string) => Promise<string>
  unwatchProject,                 // (projectId: string) => Promise<void>
  getAwareness,                   // (projectId, filePath) => Promise<FileAwareness>
  getProjectAwareness,            // (projectId) => Promise<Map<string, FileAwareness>>
  getEvents,                      // (projectId, filters) => Promise<FileEvent[]>
  attribute,                      // (projectId, filePath, source, sourceId) => Promise<void>

  // Subscriptions (auto-cleaned on unmount)
  onFileEvent,                    // (callback: (event: FileEvent) => void) => void
  onAwarenessChanged,             // (callback: (data) => void) => void
} = useAmbientFs();
```

### Reactive Awareness Map

The composable maintains a reactive Map<filePath, FileAwareness> for
the active project. This map is updated automatically when:
- ambient-fs-awareness-changed events arrive
- ambient-fs-event events arrive (for real-time recency)
- ambient-fs-analysis-complete events arrive

FileTreeNode reads from this map to render indicators.

### Auto-registration

When useProjects registers a new project or expands a project in
the sidebar, useAmbientFs automatically calls watchProject with
the project path. When a project is removed, unwatchProject is called.


## UniversalSidebar Changes

### Incremental Tree Patching

Replace filesystemChangeSignal watcher with ambient-fs events:

```typescript
// Current (full rescan):
watch(filesystemChangeSignal, async () => {
  await loadProjectTree(activeProject.value.id);
});

// New (incremental via ambient-fs):
const { onFileEvent } = useAmbientFs();

onFileEvent((event) => {
  if (event.projectId !== activeProject.value?.id) return;

  switch (event.eventType) {
    case 'created':
      addTreeNode(event.filePath);
      break;
    case 'deleted':
      removeTreeNode(event.filePath);
      break;
    case 'renamed':
      renameTreeNode(event.oldPath, event.filePath);
      break;
  }
});
```

### addTreeNode / removeTreeNode / renameTreeNode

New functions in useProjects.ts for surgical tree updates:

```typescript
function addTreeNode(filePath: string): void {
  // Parse path segments
  // Find parent folder node
  // Create new FileTreeNode
  // Insert at sorted position in parent.children
  // Trigger reactivity
}

function removeTreeNode(filePath: string): void {
  // Find node by path
  // Remove from parent.children
  // Clean up editor tabs if file was open
  // Trigger reactivity
}

function renameTreeNode(oldPath: string, newPath: string): void {
  // Find node by oldPath
  // Update path, name
  // Re-sort in parent if needed
  // Update editor tab references
  // Trigger reactivity
}
```


## FileTreeNode Awareness Overlay

### Props Addition

```typescript
// FileTreeNode.vue - new optional prop
interface Props {
  // ... existing props ...
  awareness?: FileAwareness | null;
}
```

### Rendering

After the filename, render up to 3 small indicators:

```
filename.ts  ◉ 2m  ✦3
             |     |
             |     +-- chat references (if > 0)
             +-------- source dot + relative time
```

Source dot colors (mapped from awareness.modifiedBy):
- user:     foreground/60 (subtle)
- ai_agent: purple-400
- git:      blue-400
- build:    orange-400
- voice:    green-400

Active agent (from awareness.activeAgent):
```
filename.ts  ◉ editing...   (pulsing dot, purple)
```

### CSS

```css
/* Pulsing dot for active agent */
@keyframes awareness-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}

.awareness-dot-active {
  animation: awareness-pulse 1.5s ease-in-out infinite;
}
```


## tool_runner Attribution Bridge

When tool_runner.rs executes a tool that creates/modifies files,
it reports attribution to ambient-fsd via the Tauri plugin.

### Integration Point

```rust
// src-tauri/src/commands/tool_runner.rs

// After a tool successfully writes a file:
async fn report_file_attribution(
    app_handle: &AppHandle,
    project_id: &str,
    file_path: &str,
    chat_id: &str,
) {
    // Call the ambient-fs plugin's attribution API
    if let Some(plugin) = app_handle.try_state::<AmbientFsPlugin>() {
        plugin.attribute(project_id, file_path, "ai_agent", chat_id).await;
    }
}
```

### Voice Pipeline Attribution

When the voice transcription pipeline creates or modifies a file:

```rust
// After transcription writes to a file:
plugin.attribute(project_id, file_path, "voice", transcription_id).await;
```


## ActivityFeedPanel

New component in src-vue/src/components/universal-panel/tabs/

### Design

```
+----------------------------------+
| Activity                    [F]  |
+----------------------------------+
| 10:32  ◉  routes.ts        +ai  |
| 10:33  ◉  README.md       edit  |
| 10:35  ◉  notes.md       voice  |
| 10:41  ◉  48 files         git  |
+----------------------------------+
| [F] = filter dropdown            |
+----------------------------------+
```

### Filters

- Source: all | user | ai | git | build | voice
- Time: 1h | today | this week | all
- Preset: "since I was away" (events since last Kollabor focus)

### Tab Registry

```typescript
// tab-registry.ts
{
  id: 'activity',
  label: 'Activity',
  icon: Activity,  // from lucide
  component: ActivityFeedTabContent,
  availableFor: ['file', 'chat', 'folder'],
}
```


## What Changes in Existing Code

### Removed

- filesystemChangeSignal usage in UniversalSidebar (replaced by ambient-fs events)
- useFilesystemWatcher.ts (stub, never worked, replaced entirely)
- workspace.rs watch_workspace_folder command (daemon handles watching now)
- nativeAPI.watchWorkspaceFolder (daemon handles this)
- nativeAPI.onWorkspaceFileCreated/Modified/Deleted (replaced by ambient-fs events)

### Kept

- Refresh button on project headers (fallback, calls loadProjectTree)
- loadProjectTree in useProjects (used for initial load + manual refresh)
- filesystemChangeSignal for tool execution (kept as intermediate until
  tool_runner attribution is wired, then removed)

### Added

- tauri-plugin-ambient-fs dependency in Cargo.toml
- useAmbientFs.ts composable
- FileAwareness TypeScript interface in types/
- Awareness overlay in FileTreeNode.vue
- ActivityFeedPanel component
- addTreeNode/removeTreeNode/renameTreeNode in useProjects.ts


## Beads Stories (Kollabor Integration Only)

These stories assume ambient-fsd phases 0-3 are complete.
They can be tracked under the same epic: kollabor-app-v1-xz7.

**INT-1: Add tauri-plugin-ambient-fs to Kollabor**
- Add dependency to src-tauri/Cargo.toml
- Register plugin in main.rs
- Auto-launch daemon if not running
- Priority: P2
- Depends on: ambient-fsd Phase 0 complete

**INT-2: Create useAmbientFs composable**
- Wrap plugin IPC commands
- Reactive FileAwareness map
- Auto-register projects with daemon
- Event subscriptions with cleanup
- Priority: P2
- Depends on: INT-1

**INT-3: Implement incremental tree patching**
- addTreeNode/removeTreeNode/renameTreeNode in useProjects.ts
- Wire to ambient-fs-event in UniversalSidebar
- Keep loadProjectTree as manual refresh fallback
- Priority: P2
- Depends on: INT-2

**INT-4: Add FileAwareness overlay to FileTreeNode**
- Accept awareness prop
- Render source dot + relative time
- Pulsing dot for active agents
- Priority: P2
- Depends on: INT-2

**INT-5: Wire tool_runner attribution**
- After tool writes file: call ambient-fs attribute API
- Pass source=ai_agent, source_id=chat_id
- Wire voice pipeline: source=voice
- Priority: P2
- Depends on: INT-1

**INT-6: Add hover tooltip with awareness detail**
- Full modification info on hover
- Chat references, line count, TODOs
- Priority: P3
- Depends on: INT-4

**INT-7: Create ActivityFeedPanel**
- Vue component showing daemon event stream
- Register in tab-registry
- Source + time filtering
- Priority: P3
- Depends on: INT-2

**INT-8: Remove old watcher code**
- Remove useFilesystemWatcher.ts
- Remove workspace.rs watch commands
- Remove filesystemChangeSignal (once attribution is wired)
- Remove nativeAPI workspace watcher methods
- Priority: P3
- Depends on: INT-3 + INT-5
