tauri plugin implementation
==========================

status: design
created: 2026-02-16
affects: integrations/tauri-plugin-ambient-fs/


beads: ambient-fs-ch4 (skeleton), ambient-fs-lgc (ipc bridge)
------------------------------------------------------------


overview
--------

this spec defines the complete implementation of the tauri plugin
that bridges ambient-fsd to tauri apps. it combines the skeleton
structure (ch4) with the IPC bridge commands/events (lgc) into
a single actionable implementation spec.

the plugin lives outside the main workspace at integrations/tauri-plugin-ambient-fs/
because it's a reusable component that can be used by any tauri app
(kollabor or others).


directory structure
--------------------

  integrations/tauri-plugin-ambient-fs/
    Cargo.toml
    README.md
    src/
      lib.rs              plugin entry point, Plugin trait impl
      state.rs            managed plugin state (client + connection)
      commands.rs         IPC command handlers (invoke)
      events.rs           event forwarding from daemon to tauri
      config.rs           plugin config (auto-launch, socket path)
    guest-js/
      index.ts            TypeScript API for frontend
      types.ts            TypeScript type definitions


Cargo.toml
-----------

  [package]
  name = "tauri-plugin-ambient-fs"
  version = "0.1.0"
  edition = "2021"

  [dependencies]
  tauri = { version = "2", features = ["specta"] }
  ambient-fs-client = { path = "../../crates/ambient-fs-client" }
  ambient-fs-core = { path = "../../crates/ambient-fs-core" }
  serde = { version = "1", features = ["derive"] }
  serde_json = "1"
  tokio = { version = "1", features = ["sync", "time", "net"] }
  tracing = "0.1"
  thiserror = "2"

  [features]
  default = ["auto-launch"]
  auto-launch = []  # enables daemon auto-launch on plugin init


lib.rs
------

  use tauri::{AppHandle, Manager, Plugin, Runtime, State};
  use tauri::plugin::{Builder, TauriPlugin};
  use state::PluginState;

  /// Initialize the ambient-fs plugin
  pub fn init() -> TauriPlugin {
      Builder::new("ambient-fs")
          .invoke_handler(tauri::generate_handler![
              commands::watch_project,
              commands::unwatch_project,
              commands::query_events,
              commands::query_awareness,
              commands::query_tree,
              commands::attribute,
              commands::query_agents,
              commands::get_status,
          ])
          .setup(|app, _api| {
              let state = PluginState::new(app.clone());
              app.manage(state);
              Ok(())
          })
          .build()
  }


state.rs
--------

  use std::sync::{Arc, Mutex, RwLock, atomic::{AtomicBool, Ordering}};
  use tauri::AppHandle;
  use ambient_fs_client::{AmbientFsClient, EventFilter};
  use ambient_fs_core::{FileEvent, FileAwareness, Source};

  pub struct PluginState {
      client: Arc<Mutex<Option<AmbientFsClient>>>,
      connected: Arc<AtomicBool>,
      socket_path: String,
      app_handle: AppHandle,
      auto_launch: bool,
  }

  impl PluginState {
      pub fn new(app_handle: AppHandle) -> Self {
          let socket_path = std::env::var("AMBIENT_FS_SOCKET")
              .unwrap_or_else(|_| "/tmp/ambient-fs.sock".to_string());
          let auto_launch = cfg!(feature = "auto-launch");

          let state = Self {
              client: Arc::new(Mutex::new(None)),
              connected: Arc::new(AtomicBool::new(false)),
              socket_path,
              app_handle,
              auto_launch,
          };

          // spawn connection task
          let state_clone = state.clone();
          tokio::spawn(async move {
              state_clone.connection_loop().await;
          });

          state
      }

      /// Main connection loop: retry until connected
      async fn connection_loop(&self) {
          loop {
              match self.try_connect().await {
                  Ok(_) => {
                      self.connected.store(true, Ordering::SeqCst);
                      events::emit_connected(&self.app_handle, true);
                      self.run_event_loop().await;
                      // event loop ended = connection lost
                      self.connected.store(false, Ordering::SeqCst);
                      events::emit_connected(&self.app_handle, false);
                  }
                  Err(_) => {
                      self.connected.store(false, Ordering::SeqCst);
                      if self.auto_launch {
                          self.try_launch_daemon().await;
                      }
                  }
              }
              tokio::time::sleep(Duration::from_secs(5)).await;
          }
      }

      async fn try_connect(&self) -> Result<()> {
          let client = AmbientFsClient::connect(&self.socket_path).await?;
          *self.client.lock().unwrap() = Some(client);
          Ok(())
      }

      #[cfg(feature = "auto-launch")]
      async fn try_launch_daemon(&self) {
          // attempt to spawn ambient-fsd if in PATH
          let _ = tokio::process::Command::new("ambient-fsd")
              .arg("daemon")
              .spawn();
      }

      /// Get client, return error if not connected
      pub async fn with_client<F, R>(&self, f: F) -> Result<R>
      where
          F: FnOnce(&mut AmbientFsClient) -> Result<R>,
      {
          let mut client_guard = self.client.lock().unwrap();
          let client = client_guard.as_mut()
              .ok_or_else(|| anyhow!("not connected to daemon"))?;
          f(client)
      }

      pub fn is_connected(&self) -> bool {
          self.connected.load(Ordering::SeqCst)
      }
  }


commands.rs
-----------

  use tauri::{State, AppHandle};
  use ambient_fs_core::{FileEvent, FileAwareness, Source};
  use ambient_fs_client::EventFilter;
  use crate::state::PluginState;

  /// Watch a project directory
  #[tauri::command]
  pub async fn watch_project(
      state: State<'_, PluginState>,
      path: String,
  ) -> Result<String, String> {
      state.with_client(|client| {
          async move {
              let project_id = client.watch_project(&path).await
                  .map_err(|e| e.to_string())?;
              Ok(project_id)
          }.await
      }).await.map_err(|e| e.to_string())
  }

  /// Stop watching a project
  #[tauri::command]
  pub async fn unwatch_project(
      state: State<'_, PluginState>,
      project_id: String,
  ) -> Result<(), String> {
      state.with_client(|client| {
          async move {
              client.unwatch_project(&project_id).await
                  .map_err(|e| e.to_string())
          }.await
      }).await.map_err(|e| e.to_string())
  }

  /// Query events with optional filter
  #[tauri::command]
  pub async fn query_events(
      state: State<'_, PluginState>,
      project_id: Option<String>,
      since: Option<i64>,
      source: Option<String>,
      limit: Option<usize>,
  ) -> Result<Vec<FileEvent>, String> {
      state.with_client(|client| {
          async move {
              let filter = EventFilter {
                  project_id,
                  since,
                  source,
                  limit,
              };
              client.query_events(filter).await
                  .map_err(|e| e.to_string())
          }.await
      }).await.map_err(|e| e.to_string())
  }

  /// Query awareness for a single file
  #[tauri::command]
  pub async fn query_awareness(
      state: State<'_, PluginState>,
      project_id: String,
      file_path: String,
  ) -> Result<FileAwareness, String> {
      state.with_client(|client| {
          async move {
              client.query_awareness(&project_id, &file_path).await
                  .map_err(|e| e.to_string())
          }.await
      }).await.map_err(|e| e.to_string())
  }

  /// Query the file tree for a project
  #[tauri::command]
  pub async fn query_tree(
      state: State<'_, PluginState>,
      project_id: String,
  ) -> Result<TreeNode, String> {
      // this calls a daemon method that returns the cached tree
      // tree_state.rs in server maintains this
      state.with_client(|client| {
          async move {
              let params = json!({ "project_id": project_id });
              let response = client.send_request("query_tree", &params).await
                  .map_err(|e| e.to_string())?;
              serde_json::from_value(response)
                  .map_err(|e| e.to_string())
          }.await
      }).await.map_err(|e| e.to_string())
  }

  /// Attribute a file change to a source
  #[tauri::command]
  pub async fn attribute(
      state: State<'_, PluginState>,
      project_id: String,
      file_path: String,
      source: String,
      source_id: Option<String>,
  ) -> Result<(), String> {
      state.with_client(|client| {
          async move {
              client.attribute(&project_id, &file_path, &source, source_id.as_deref()).await
                  .map_err(|e| e.to_string())
          }.await
      }).await.map_err(|e| e.to_string())
  }

  /// Query active agents
  #[tauri::command]
  pub async fn query_agents(
      state: State<'_, PluginState>,
  ) -> Result<Vec<serde_json::Value>, String> {
      state.with_client(|client| {
          async move {
              client.query_agents().await
                  .map_err(|e| e.to_string())
          }.await
      }).await.map_err(|e| e.to_string())
  }

  /// Get daemon connection status
  #[tauri::command]
  pub async fn get_status(
      state: State<'_, PluginState>,
  ) -> Result<DaemonStatus, String> {
      Ok(DaemonStatus {
          connected: state.is_connected(),
      })
  }

  #[derive(Debug, Clone, Serialize)]
  struct DaemonStatus {
      connected: bool,
  }


events.rs
---------

  use tauri::AppHandle;
  use ambient_fs_core::FileEvent;
  use crate::state::PluginState;

  /// Event names emitted to frontend
  pub const EVT_EVENT: &str = "ambient-fs://event";
  pub const EVT_AWARENESS: &str = "ambient-fs://awareness-changed";
  pub const EVT_ANALYSIS: &str = "ambient-fs://analysis-complete";
  pub const EVT_CONNECTED: &str = "ambient-fs://connected";

  pub fn emit_connected(app: &AppHandle, connected: bool) {
      let payload = json!({ "connected": connected });
      app.emit(EVT_CONNECTED, payload).ok();
  }

  impl PluginState {
      /// Main event loop: receive notifications from daemon, emit to tauri
      async fn run_event_loop(&self) {
          // this requires the client to support streaming notifications
          // the current client doesn't have this yet, so we'd need to add:
          // 1. a subscribe() method that returns a broadcast receiver
          // 2. or a poll_notifications() method for periodic checking
          //
          // for now, the implementation would poll or extend the client
          // with notification streaming support.

          // placeholder: periodic awareness polling for watched projects
          let mut interval = tokio::time::interval(Duration::from_secs(30));
          loop {
              interval.tick().await;
              if let Err(_) = self.poll_awareness_updates().await {
                  break; // connection lost
              }
          }
      }

      async fn poll_awareness_updates(&self) -> Result<()> {
          // query awareness for files in watched projects
          // emit events if changed
          // (this is a simplified version; real impl would use daemon push)
          Ok(())
      }
  }


config.rs
---------

  /// Plugin configuration options
  #[derive(Debug, Clone)]
  pub struct AmbientFsConfig {
      /// Path to ambient-fsd socket
      pub socket_path: Option<String>,
      /// Whether to auto-launch the daemon
      pub auto_launch: bool,
      /// Connection timeout
      pub connect_timeout_secs: u64,
  }

  impl Default for AmbientFsConfig {
      fn default() -> Self {
          Self {
              socket_path: None,
              auto_launch: true,
              connect_timeout_secs: 5,
          }
      }
  }


guest-js/index.ts (TypeScript API)
-----------------------------------

  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

  // IPC command wrappers
  export async function watchProject(path: string): Promise<string> {
      return invoke('plugin:ambient-fs|watch_project', { path });
  }

  export async function unwatchProject(projectId: string): Promise<void> {
      return invoke('plugin:ambient-fs|unwatch_project', { projectId });
  }

  export async function queryEvents(filter: EventFilter): Promise<FileEvent[]> {
      return invoke('plugin:ambient-fs|query_events', { filter });
  }

  export async function queryAwareness(projectId: string, filePath: string): Promise<FileAwareness> {
      return invoke('plugin:ambient-fs|query_awareness', { projectId, filePath });
  }

  export async function queryTree(projectId: string): Promise<TreeNode> {
      return invoke('plugin:ambient-fs|query_tree', { projectId });
  }

  export async function attribute(
      projectId: string,
      filePath: string,
      source: string,
      sourceId?: string,
  ): Promise<void> {
      return invoke('plugin:ambient-fs|attribute', { projectId, filePath, source, sourceId });
  }

  export async function queryAgents(): Promise<AgentInfo[]> {
      return invoke('plugin:ambient-fs|query_agents');
  }

  export async function getStatus(): Promise<DaemonStatus> {
      return invoke('plugin:ambient-fs|get_status');
  }

  // Event listeners
  export function onFileEvent(callback: (event: FileEvent) => void): UnlistenFn {
      return listen<FileEvent>('ambient-fs://event', (e) => callback(e.payload));
  }

  export function onAwarenessChanged(callback: (data: AwarenessChangedEvent) => void): UnlistenFn {
      return listen('ambient-fs://awareness-changed', (e) => callback(e.payload));
  }

  export function onAnalysisComplete(callback: (data: AnalysisCompleteEvent) => void): UnlistenFn {
      return listen('ambient-fs://analysis-complete', (e) => callback(e.payload));
  }

  export function onConnectedChanged(callback: (connected: boolean) => void): UnlistenFn {
      return listen<{connected: boolean}>('ambient-fs://connected', (e) => callback(e.payload.connected));
  }


guest-js/types.ts
-----------------

  // mirrors Rust types from ambient-fs-core

  export type EventType = 'created' | 'modified' | 'deleted' | 'renamed';
  export type Source = 'user' | 'ai_agent' | 'git' | 'build' | 'voice';
  export type ChangeFrequency = 'hot' | 'warm' | 'cold';

  export interface FileEvent {
      timestamp: string;           // ISO 8601
      event_type: EventType;
      file_path: string;
      project_id: string;
      source: Source;
      source_id?: string;
      machine_id: string;
      content_hash?: string;
      old_path?: string;
  }

  export interface FileAwareness {
      file_path: string;
      project_id: string;
      last_modified: string;        // ISO 8601
      modified_by: Source;
      modified_by_label?: string;
      active_agent?: string;
      chat_references: number;
      todo_count: number;
      lint_hints: number;
      line_count: number;
      change_frequency: ChangeFrequency;
  }

  export interface TreeNode {
      name: string;
      path: string;
      is_dir: boolean;
      children: TreeNode[];
  }

  export interface EventFilter {
      project_id?: string;
      since?: number;              // unix timestamp
      source?: string;
      limit?: number;
  }

  export interface AgentInfo {
      id: string;
      name: string;
      status: 'active' | 'idle' | 'error';
      project_id?: string;
      file_path?: string;
  }

  export interface AwarenessChangedEvent {
      project_id: string;
      file_path: string;
      awareness: FileAwareness;
  }

  export interface AnalysisCompleteEvent {
      project_id: string;
      file_path: string;
      analysis: FileAnalysis;
  }

  export interface FileAnalysis {
      imports: ImportRef[];
      exports: ExportRef[];
      todos: TodoHint[];
      lint: LintHint[];
  }

  export interface ImportRef {
      module: string;
      line: number;
  }

  export interface ExportRef {
      name: string;
      line: number;
  }

  export interface TodoHint {
      line: number;
      text: string;
      tag: 'TODO' | 'FIXME' | 'HACK' | 'NOTE';
  }

  export interface LintHint {
      line: number;
      message: string;
      severity: 'error' | 'warning' | 'info';
  }

  export interface DaemonStatus {
      connected: boolean;
  }


Vue composable: useAmbientFs.ts (kollabor usage)
-------------------------------------------------

  this would live in kollabor's codebase, not the plugin.

  import { ref, computed, onMounted, onUnmounted } from 'vue';
  import * as AmbientFs from '@ambient-fs/tauri-plugin';

  export function useAmbientFs() {
      const isConnected = ref(false);
      const projectAwareness = ref<Map<string, FileAwareness>>(new Map());

      let unlisten: (() => void)[] = [];

      onMounted(() => {
          // connection status
          unlisten.push(
              AmbientFs.onConnectedChanged((connected) => {
                  isConnected.value = connected;
              })
          );

          // awareness updates
          unlisten.push(
              AmbientFs.onAwarenessChanged(({ file_path, awareness }) => {
                  projectAwareness.value.set(file_path, awareness);
              })
          );

          // update awareness on file events
          unlisten.push(
              AmbientFs.onFileEvent(async (event) => {
                  const awareness = await AmbientFs.queryAwareness(
                      event.project_id,
                      event.file_path
                  );
                  projectAwareness.value.set(event.file_path, awareness);
              })
          );
      });

      onUnmounted(() => {
          unlisten.forEach(fn => fn());
      });

      return {
          isConnected,
          projectAwareness,
          watchProject: AmbientFs.watchProject,
          unwatchProject: AmbientFs.unwatchProject,
          queryAwareness: AmbientFs.queryAwareness,
          queryEvents: AmbientFs.queryEvents,
          attribute: AmbientFs.attribute,
          queryAgents: AmbientFs.queryAgents,
          getStatus: AmbientFs.getStatus,
      };
  }


usage in kollabor
-----------------

  // src-tauri/Cargo.toml
  [dependencies]
  tauri-plugin-ambient-fs = { path = "../../ambient-fs/integrations/tauri-plugin-ambient-fs" }

  // src-tauri/src/main.rs
  fn main() {
      tauri::Builder::default()
          .plugin(tauri_plugin_ambient_fs::init())
          .run(tauri::generate_context!())
          .expect("error while running tauri application");
  }

  // package.json (kollabor frontend)
  {
      "dependencies": {
          "@ambient-fs/tauri-plugin": "file:../ambient-fs/integrations/tauri-plugin-ambient-fs"
      }
  }


testing strategy
-----------------

unit tests (in plugin crate):
  - PluginState initializes correctly
  - command functions accept correct params
  - event payloads serialize correctly
  - error handling returns proper error strings

integration tests (requires tauri-test or manual):
  - plugin connects to running daemon
  - IPC commands call daemon and return results
  - connection status updates on connect/disconnect
  - auto-launch spawns daemon if not running
  - event forwarding works end-to-end

test files:
  tests/test_commands.rs    - command logic tests
  tests/test_events.rs      - event serialization tests
  tests/test_integration.rs - full integration with mock daemon


blocking issues
---------------

1. client notification streaming
   - current AmbientFsClient doesn't support receiving notifications
   - needs subscribe() method or streaming channel
   - temporary: poll for awareness updates every 30s
   - proper: extend client with notification receiver

2. tree_state query in daemon
   - query_tree command needs server support
   - tree_state.rs in server should expose this via JSON-RPC
   - for now: frontend can build tree from events

3. reconnection handling
   - if daemon restarts, plugin needs to re-subscribe to projects
   - track watched projects in state, re-subscribe on reconnect


implementation order
--------------------

1. core structure: lib.rs, state.rs, Cargo.toml
2. commands.rs: all IPC handlers
3. events.rs: connection status emit (polling version)
4. guest-js: TypeScript API + types
5. tests: unit tests for commands
6. integration: test with real daemon
7. (future) streaming events when client supports it


dependencies
------------

  - ambient-fs-client (done)
  - ambient-fs-core (done)
  - tauri 2.x
  - tokio (runtime)
