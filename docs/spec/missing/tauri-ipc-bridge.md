tauri IPC bridge events and commands
=====================================

status: design
created: 2026-02-16
affects: integrations/tauri-plugin-ambient-fs/ (new)


beads: ambient-fs-lgc
----------------------

overview
--------

the tauri plugin needs to bridge between the ambient-fs
daemon (Unix socket) and Kollabor's frontend (Tauri IPC).
this means exposing IPC commands that the Vue frontend can
call, and emitting IPC events when the daemon sends
notifications.


IPC commands (frontend -> backend)
------------------------------------

each command wraps a daemon JSON-RPC call:

  ambient_fs_watch_project(path: String) -> String
    calls daemon: method "watch_project", params { path }
    returns: project_id

  ambient_fs_unwatch_project(project_id: String) -> ()
    calls daemon: method "unwatch_project", params { project_id }

  ambient_fs_get_awareness(project_id: String, file_path: String) -> FileAwareness
    calls daemon: method "query_awareness", params { project_id, path }
    returns: FileAwareness JSON

  ambient_fs_get_events(project_id: String, since: Option<i64>,
                        source: Option<String>, limit: Option<i32>) -> Vec<FileEvent>
    calls daemon: method "query_events", params { project_id, since, source, limit }
    returns: array of FileEvent JSON

  ambient_fs_attribute(project_id: String, file_path: String,
                       source: String, source_id: Option<String>) -> ()
    calls daemon: method "attribute", params { file_path, project_id, source, source_id }

  ambient_fs_status() -> DaemonStatus
    pings daemon, returns connection state + watched projects count

  ambient_fs_query_agents() -> Vec<AgentInfo>
    calls daemon: method "query_agents"
    returns: array of agent state JSON


IPC events (backend -> frontend)
----------------------------------

emitted by the plugin when daemon sends notifications
after a subscribe() call:

  ambient-fs-event
    payload: FileEvent (same schema as daemon)
    emitted for every file event in subscribed projects

  ambient-fs-awareness-changed
    payload: { projectId, filePath, awareness: FileAwareness }
    emitted when awareness data changes for a file
    (triggered by file events -- plugin queries awareness
    after each event and emits if changed)

  ambient-fs-analysis-complete
    payload: { projectId, filePath, analysis: FileAnalysis }
    emitted when LLM analysis completes for a file
    (daemon sends this as a notification after tier 2 analysis)

  ambient-fs-connected
    payload: { status: "connected" | "disconnected" }
    emitted when connection to daemon changes


implementation
--------------

this is part of the tauri plugin (ch4). the plugin
maintains a persistent connection to the daemon and
translates between protocols.

  plugin.rs:
    - holds AmbientFsClient (from ambient-fs-client crate)
    - on init: connect to daemon, subscribe to all projects
    - on event: emit Tauri IPC event to window
    - reconnect logic: retry every 5s if disconnected

  bridge.rs:
    - register_commands(): registers all IPC commands
    - each command function: receives AppHandle, extracts
      plugin state, calls client method, returns result

  the plugin uses ambient-fs-client as its daemon interface.
  no raw socket handling -- the client crate handles that.


event forwarding flow
-----------------------

  1. plugin subscribes to project via client.subscribe()
  2. daemon broadcasts FileEvent to subscription
  3. client receives notification on stream
  4. plugin spawns task to handle:
     a. emit "ambient-fs-event" to Tauri window
     b. call client.query_awareness() for the changed file
     c. emit "ambient-fs-awareness-changed" with result
  5. frontend composable (useAmbientFs) receives events
     and updates reactive state


test strategy
-------------

unit tests (in plugin crate):
  - command registration works
  - IPC command functions accept correct params
  - event payload serialization matches expected schema

integration tests:
  - plugin connects to daemon on init
  - IPC command calls daemon and returns result
  - daemon event -> Tauri IPC event pipeline works
  - reconnect after daemon restart

note: full integration requires Tauri runtime, so some
tests may need to be manual or use tauri::test utilities.


depends on
----------

  - ch4 tauri plugin skeleton (open)
  - ambient-fs-client (done + enhancements in progress)
  - attribution API (9ie, in progress)
  - subscribe streaming in client (partial)
