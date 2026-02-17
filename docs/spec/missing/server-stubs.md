server stubs (ambient-fs-server)
================================

status: stub implementations found by agents
found: 2026-02-16
affects: crates/ambient-fs-server/src/socket.rs


all 6 stubs are in handle_request() (socket.rs:230-257).
every method matches on the Method enum but returns a
hardcoded JSON value instead of doing real work.

the core problem: handle_request is a sync fn with no
access to the store, watcher, or subscription manager.
it needs to become async and receive shared state.


shared prerequisite: server state
---------------------------------

  before any of these can be implemented, handle_request
  needs access to shared state:

    struct ServerState {
        store_path: PathBuf,
        subscriptions: SubscriptionManager,
        watcher: Arc<Mutex<FsWatcher>>,
        projects: Arc<Mutex<HashMap<String, PathBuf>>>,
    }

  handle_connection needs to receive Arc<ServerState>.
  handle_request needs to become async and take &ServerState.

  socket.rs:114 currently spawns handle_connection(stream).
  needs to become handle_connection(stream, state.clone()).


e44: subscribe
--------------

  beads: ambient-fs-e44
  file:  socket.rs:232-235
  stub:  returns {"subscribed": true}

  what it should do:
    1. extract project_id from params (required)
    2. validate project_id exists (optional, warn if not)
    3. call state.subscriptions.subscribe(project_id)
    4. hold the broadcast::Receiver for this connection
    5. spawn a task that reads from receiver and writes
       events to the client stream as JSON-RPC notifications
    6. return {"subscribed": true, "project_id": "..."}

  the tricky part:
    subscribe creates a long-lived stream. the connection
    handler needs to select! between:
    - reading new requests from the client
    - forwarding events from the subscription receiver

    this means the current line-by-line request loop
    (socket.rs:172) needs to become a select! loop.

  params:
    {"project_id": "my-project"}  (required)

  response:
    {"subscribed": true, "project_id": "my-project"}

  notifications (server -> client, after subscribe):
    {"jsonrpc":"2.0","method":"event","params":{<FileEvent>}}

  depends on:
    - shared state prerequisite


kx2: unsubscribe
-----------------

  beads: ambient-fs-kx2
  file:  socket.rs:236-239
  stub:  returns true

  what it should do:
    1. extract project_id from params
    2. call state.subscriptions.unsubscribe(project_id)
    3. drop the broadcast::Receiver for this project
       on this connection
    4. return true

  params:
    {"project_id": "my-project"}  (required)

  response:
    true

  depends on:
    - e44 (subscribe must exist first)


h4s: query_events
------------------

  beads: ambient-fs-h4s
  file:  socket.rs:240-243
  stub:  returns []

  what it should do:
    1. extract params: project_id (optional), since (optional,
       seconds ago), source (optional), limit (optional)
    2. build EventFilter from params
    3. open store connection (spawn_blocking since rusqlite
       is sync)
    4. run store.query(filter)
    5. serialize Vec<FileEvent> as JSON array
    6. return the array

  params:
    {
      "project_id": "my-project",   // optional
      "since": 3600,                 // optional, seconds
      "source": "ai_agent",         // optional
      "limit": 100                   // optional, default 100
    }

  response:
    [<FileEvent>, <FileEvent>, ...]

  depends on:
    - shared state prerequisite (needs store_path)


68e: query_awareness
---------------------

  beads: ambient-fs-68e
  file:  socket.rs:244-247
  stub:  returns null

  what it should do:
    1. extract project_id and file_path from params
    2. query latest event for this file from store
    3. query analysis cache for this file (if exists)
    4. build FileAwareness from event + analysis + active
       agent state
    5. return serialized FileAwareness

  this is the most complex query -- it aggregates from
  multiple sources (store, analyzer cache, agent claims).

  params:
    {
      "project_id": "my-project",   // required
      "path": "src/main.rs"         // required
    }

  response:
    {
      "file_path": "src/main.rs",
      "project_id": "my-project",
      "last_modified": "2026-02-16T...",
      "modified_by": "ai_agent",
      "change_frequency": "hot",
      "todo_count": 3,
      "lint_hints": 0,
      "line_count": 142,
      "active_agent": null
    }

  depends on:
    - shared state prerequisite
    - active-agent-protocol.md (for active_agent field)
    - analyzer integration (for todo/lint/line counts)
    - partially works without analyzer (just event data)


pwu: watch_project
-------------------

  beads: ambient-fs-pwu
  file:  socket.rs:248-251
  stub:  returns {"watching": true}

  what it should do:
    1. extract path from params (required)
    2. validate path exists and is a directory
    3. call DaemonServer::watch_project(path) or equivalent
       - adds to watcher
       - generates project_id
       - persists to store
    4. return {"watching": true, "project_id": "..."}

  params:
    {"path": "/home/user/my-project"}  (required)

  response:
    {"watching": true, "project_id": "my-project"}

  error cases:
    - path doesn't exist: invalid_path error
    - path not a directory: invalid_path error
    - already watching: already_watching error

  depends on:
    - shared state prerequisite
    - 0d2 (store.add_project)


1hc: unwatch_project
---------------------

  beads: ambient-fs-1hc
  file:  socket.rs:252-255
  stub:  returns true

  what it should do:
    1. extract project_id from params (required)
    2. look up path from project_id mapping
    3. call watcher.unwatch(path)
    4. remove from store
    5. remove from projects map
    6. return true

  params:
    {"project_id": "my-project"}  (required)

  response:
    true

  error cases:
    - project_id not found: project_not_found error

  depends on:
    - shared state prerequisite
    - 6n0 (unwatch_project in DaemonServer)
    - pwu (watch must exist to unwatch)
