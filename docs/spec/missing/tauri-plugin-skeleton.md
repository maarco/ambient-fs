tauri plugin skeleton
======================

status: design
created: 2026-02-16
affects: integrations/tauri-plugin-ambient-fs/ (new directory)


beads: ambient-fs-ch4
----------------------

overview
--------

Kollabor needs a Tauri plugin that bridges ambient-fs daemon
communication into the Tauri IPC layer. the plugin directory
exists but is empty.


directory structure
--------------------

  integrations/tauri-plugin-ambient-fs/
    Cargo.toml
    src/
      lib.rs         plugin entry point, init()
      bridge.rs      IPC command handlers
      state.rs       plugin state (client connection)
      events.rs      event forwarding to Tauri window


Cargo.toml
-----------

  [package]
  name = "tauri-plugin-ambient-fs"
  version = "0.1.0"
  edition = "2021"

  [dependencies]
  tauri = { version = "2", features = [] }
  ambient-fs-client = { path = "../../crates/ambient-fs-client" }
  ambient-fs-core = { path = "../../crates/ambient-fs-core" }
  serde = { version = "1", features = ["derive"] }
  serde_json = "1"
  tokio = { version = "1", features = ["sync", "time"] }
  tracing = "0.1"


lib.rs
------

  pub fn init() -> TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("ambient-fs")
      .invoke_handler(tauri::generate_handler![
        bridge::watch_project,
        bridge::unwatch_project,
        bridge::get_awareness,
        bridge::get_events,
        bridge::attribute,
        bridge::status,
        bridge::query_agents,
      ])
      .setup(|app, _api| {
        // initialize plugin state
        let state = AmbientFsState::new();
        app.manage(state);
        // spawn connection task
        let app_handle = app.clone();
        tokio::spawn(async move {
          connect_and_subscribe(app_handle).await;
        });
        Ok(())
      })
      .build()
  }


state.rs
--------

  pub struct AmbientFsState {
    client: Arc<Mutex<Option<AmbientFsClient>>>,
    connected: Arc<AtomicBool>,
    watched_projects: Arc<RwLock<Vec<String>>>,
  }

  impl AmbientFsState {
    pub fn new() -> Self
    pub async fn connect(&self) -> Result<()>
    pub async fn client(&self) -> Result<MutexGuard<AmbientFsClient>>
    pub fn is_connected(&self) -> bool
  }


bridge.rs
---------

  each function is a #[tauri::command]:

  #[tauri::command]
  async fn watch_project(
    state: tauri::State<'_, AmbientFsState>,
    path: String,
  ) -> Result<String, String>

  #[tauri::command]
  async fn get_awareness(
    state: tauri::State<'_, AmbientFsState>,
    project_id: String,
    file_path: String,
  ) -> Result<serde_json::Value, String>

  etc. each extracts state, calls client method, returns
  JSON-serializable result or error string.


events.rs
---------

  async fn connect_and_subscribe(app: AppHandle) {
    loop {
      match AmbientFsClient::connect_local().await {
        Ok(client) => {
          emit_connected(&app, true);
          // subscribe to all watched projects
          // forward events to Tauri window
          forward_events(client, &app).await;
          // if we get here, connection dropped
          emit_connected(&app, false);
        }
        Err(_) => {
          emit_connected(&app, false);
          tokio::time::sleep(Duration::from_secs(5)).await;
        }
      }
    }
  }

  fn emit_connected(app: &AppHandle, connected: bool) {
    app.emit("ambient-fs-connected", json!({ "status": if connected { "connected" } else { "disconnected" } })).ok();
  }


daemon auto-launch
-------------------

  on plugin init, if daemon isn't running (connect fails):
  1. try to spawn ambient-fsd binary
  2. wait up to 5s for socket to appear
  3. connect

  this is optional behavior, controlled by config.
  if the user manages the daemon manually, skip auto-launch.

  implementation:
    std::process::Command::new("ambient-fsd")
      .arg("start")
      .spawn()


test strategy
-------------

unit tests:
  - state initializes correctly
  - command functions have correct signatures
  - event payload serialization

integration tests:
  - plugin init connects to running daemon
  - IPC commands work end-to-end
  - event forwarding works
  - reconnect after daemon restart
  - auto-launch spawns daemon

note: requires Tauri 2 test harness for full integration.
basic struct/logic tests work without Tauri.


depends on
----------

  - ambient-fs-client (done + enhancements in progress)
  - Tauri 2.x plugin API
  - ambient-fsd binary built and in PATH
