server implementations status (ambient-fs-server)
===================================================

status: ALL CORE METHODS IMPLEMENTED
updated: 2026-02-16
affects: crates/ambient-fs-server/src/socket.rs, crates/ambient-fs-server/src/state.rs


shared prerequisite: server state - DONE
-----------------------------------------

  COMPLETED: ServerState struct created in state.rs
    - store_path: PathBuf
    - subscriptions: SubscriptionManager
    - projects: Arc<RwLock<HashMap<String, PathBuf>>>

  COMPLETED: SocketServer refactored
    - state: Option<Arc<ServerState>> field added
    - set_state(&mut self, state: Arc<ServerState>) method added
    - handle_connection(stream, state: Arc<ServerState>) signature updated
    - handle_request async fn handle_request(req, state: &ServerState, conn_state: &mut ConnectionState)

  COMPLETED: DaemonServer updated (crates/ambient-fsd/src/server.rs)
    - creates ServerState with db_path
    - calls socket.set_state() before run()


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
  file:  socket.rs:409-411 (dispatch), 445-486 (implementation)
  status: DONE (implemented 2026-02-16)

  implementation:
    1. extract project_id from params (required)
    2. validate project_id is a string
    3. subscribe via state.subscriptions.subscribe(project_id)
    4. store receiver in conn_state.subscriptions
    5. return {"subscribed": true, "project_id": "..."}

  ConnectionState added:
    - tracks HashMap<project_id, broadcast::Receiver<FileEvent>>
    - methods: new(), add_subscription(), remove_subscription(), has_subscriptions()

  note: event forwarding NOT yet implemented - requires select! loop refactoring
    the receiver is stored but events are not yet forwarded to client

  params:
    {"project_id": "my-project"}  (required)

  response:
    {"subscribed": true, "project_id": "my-project"}

  notifications: NOT YET IMPLEMENTED
    will be: {"jsonrpc":"2.0","method":"event","params":{<FileEvent>}}

  error cases:
    - missing project_id: invalid_params error (-32602)

  implementation location:
    - handler: socket.rs:handle_subscribe (~line 445)
    - dispatch: socket.rs:handle_request -> Method::Subscribe (~line 409)


kx2: unsubscribe
-----------------

  beads: ambient-fs-kx2
  file:  socket.rs:412-414 (dispatch), 493-530 (implementation)
  status: DONE (implemented 2026-02-16)

  implementation:
    1. extract project_id from params (required)
    2. remove from conn_state.subscriptions (drops receiver)
    3. call state.subscriptions.unsubscribe(project_id)
    4. return true

  params:
    {"project_id": "my-project"}  (required)

  response:
    true

  error cases:
    - missing project_id: invalid_params error (-32602)
    - not subscribed to this project: still returns success (idempotent)

  implementation location:
    - handler: socket.rs:handle_unsubscribe (~line 493)
    - dispatch: socket.rs:handle_request -> Method::Unsubscribe (~line 412)


h4s: query_events
------------------

  beads: ambient-fs-h4s
  file:  socket.rs:415-417
  status: DONE (implemented 2026-02-16)

  implementation:
    1. extract params: project_id (optional), since (optional,
       seconds ago), source (optional), limit (optional)
    2. build EventFilter from params
    3. query store via spawn_blocking (rusqlite is sync)
    4. serialize Vec<FileEvent> as JSON array
    5. return the array

  params:
    {
      "project_id": "my-project",   // optional
      "since": 3600,                 // optional, seconds
      "source": "ai_agent",         // optional
      "limit": 100                   // optional
    }

  response:
    [<FileEvent>, <FileEvent>, ...]

  error cases:
    - invalid source string: invalid_params error (-32602)
    - params is array (not object): invalid_params error

  implementation location:
    - handler: socket.rs:handle_query_events (~line 733)
    - dispatch: socket.rs:handle_request -> Method::QueryEvents (~line 415)


68e: query_awareness
---------------------

  beads: ambient-fs-68e
  file:  socket.rs:244-247
  status: DONE (implemented 2026-02-16)

  implementation:
    1. extract project_id and file_path from params (both required)
    2. query latest event for this file from store (via spawn_blocking)
    3. build FileAwareness::from_event_minimal() from event
    4. return serialized FileAwareness or null if no event

  current limitations (partial implementation):
    - analysis cache not wired yet (todo_count, lint_hints, line_count are 0)
    - active_agent always null (AgentTracker integration pending)
    - these fields will be populated when analyzer is integrated

  params:
    {
      "project_id": "my-project",   // required
      "path": "src/main.rs"         // required
    }

  response (with event found):
    {
      "file_path": "src/main.rs",
      "project_id": "my-project",
      "last_modified": "2026-02-16T...",
      "modified_by": "user",        // or "ai_agent", "git", etc.
      "modified_by_label": null,
      "active_agent": null,         // TODO: populate from AgentTracker
      "chat_references": 0,         // TODO: populate from analyzer
      "todo_count": 0,              // TODO: populate from analyzer
      "lint_hints": 0,              // TODO: populate from analyzer
      "line_count": 0,              // TODO: populate from analyzer
      "change_frequency": "hot"     // calculated from last_modified
    }

  response (no event found):
    null

  error cases:
    - missing project_id: invalid_params error (-32602)
    - missing path: invalid_params error (-32602)
    - params not an object: invalid_params error

  implementation location:
    - handler: socket.rs:handle_query_awareness (~line 811)
    - dispatch: socket.rs:handle_request -> Method::QueryAwareness (~line 419)

  depends on:
    - shared state prerequisite (done - ServerState.store_path)
    - active-agent-protocol.md (for active_agent field - pending)
    - analyzer integration (for todo/lint/line counts - pending)


pwu: watch_project
-------------------

  beads: ambient-fs-pwu
  file:  socket.rs:422-424
  status: DONE (implemented 2026-02-16)

  implementation:
    1. extract path from params (required)
    2. validate path exists and is a directory
    3. check if already watching (via state.has_project)
    4. generate project_id from directory name
    5. create FsWatcher and start watching
    6. add to state.projects and state.watchers
    7. spawn event handler task for this watcher
    8. return {"watching": true, "project_id": "...", "path": "..."}

  params:
    {"path": "/home/user/my-project"}  (required)

  response:
    {"watching": true, "project_id": "my-project", "path": "/home/user/my-project"}

  error cases:
    - missing path: invalid_params error (-32602)
    - path doesn't exist: invalid_path error (-1004)
    - path not a directory: invalid_path error (-1004)
    - already watching: already_watching error (-1002)

  implementation location:
    - handler: socket.rs:handle_watch_project (~line 542)
    - dispatch: socket.rs:handle_request -> Method::WatchProject (~line 422)


1hc: unwatch_project
---------------------

  beads: ambient-fs-1hc
  file:  socket.rs:426-428
  status: DONE (implemented 2026-02-16)

  implementation:
    1. extract project_id from params (required)
    2. look up path from state.projects
    3. remove from state.projects
    4. remove watcher from state.watchers
    5. return {"unwatched": true, "project_id": "..."}

  params:
    {"project_id": "my-project"}  (required)

  response:
    {"unwatched": true, "project_id": "my-project"}

  error cases:
    - missing project_id: invalid_params error (-32602)
    - project_id not found: project_not_found error (-1001)

  implementation location:
    - handler: socket.rs:handle_unwatch_project (~line 664)
    - dispatch: socket.rs:handle_request -> Method::UnwatchProject (~line 426)


agent tracking methods (implemented 2026-02-16)
--------------------------------------------------

watch_agents (NEW)
  file: socket.rs:handle_watch_agents (~line 721)
  status: DONE

  implementation:
    1. extract path from params (required)
    2. validate path exists and is directory
    3. store in state.projects with "agents:" prefix
    4. return {"watching": true, "path": "..."}

  params:
    {"path": "/path/to/.agents"}

  response:
    {"watching": true, "path": "/path/to/.agents"}

  error cases:
    - path doesn't exist: invalid_path error
    - path not a directory: invalid_path error

  note: currently just registers the directory. a future enhancement
        will spawn a task to tail .jsonl files and feed lines to AgentTracker.

unwatch_agents (NEW)
  file: socket.rs:handle_unwatch_agents (~line 767)
  status: DONE

  implementation:
    1. extract path from params (required)
    2. remove from state.projects (by "agents:" prefix key)
    3. return true

  params:
    {"path": "/path/to/.agents"}

  response:
    true

query_agents (NEW)
  file: socket.rs:handle_query_agents (~line 804)
  status: DONE

  implementation:
    1. extract optional file param for filtering
    2. get all non-stale agents from AgentTracker
    3. filter by file if specified
    4. return array of agent states as JSON

  params:
    {"file": "src/main.rs"}  // optional, filters to agents working on this file

  response:
    [{
      "agent_id": "claude-7",
      "files": ["src/auth.rs", "src/main.rs"],
      "last_seen": 1708099200,
      "intent": "fixing auth bypass",
      "tool": "claude-code",
      "session": "session-42"
    }]


agent tracking module (agents.rs)
---------------------------------

file: crates/ambient-fs-server/src/agents.rs
status: DONE (42 tests passing)

AgentActivity struct
  - parses JSONL lines with agent activity
  - required fields: ts, agent, action, file
  - optional fields: project, tool, session, intent, lines, confidence, done
  - implements Serialize, Deserialize, PartialEq

AgentState struct
  - per-agent tracking state
  - fields: agent_id, files (Vec<String>), last_seen (DateTime<Utc>),
           intent, tool, session (all Option<String>)

AgentTracker struct
  - manages agent activity from JSONL files
  - 5-minute default stale timeout
  - methods:
    - new(stale_timeout) -> Self
    - with_default_timeout() -> Self
    - process_line(line: &str) -> Option<AgentActivity>
    - update_from_activity(activity: &AgentActivity)
    - get_active_agent(file_path: &str) -> Option<String>
    - get_all_agents() -> Vec<AgentState>
    - prune_stale() -> usize
    - calculate_pass_rate(lines: &[&str]) -> f64
    - agent_count() -> usize
    - active_agent_count() -> usize


ServerState additions
--------------------

file: crates/ambient-fs-server/src/state.rs

ServerState now includes:
  - agent_tracker: AgentTracker (with 5-minute stale timeout)
  - get_active_agent(file_path: &str) -> Option<String>
  - update_agent_activity(activity: &AgentActivity)
  - get_all_agents() -> Vec<AgentState>
  - prune_stale_agents() -> usize


protocol.rs additions
---------------------

file: crates/ambient-fs-server/src/protocol.rs

Method enum now includes:
  - WatchAgents -> "watch_agents"
  - UnwatchAgents -> "unwatch_agents"
  - QueryAgents -> "query_agents"


integration notes
-----------------

1. QueryAwareness (68e) now has access to AgentTracker via state.
   The active_agent field can be populated by calling
   state.get_active_agent(file_path).

2. The watch_agents handler registers directories but doesn't yet
   spawn file watching tasks. This would be the next enhancement:
   - detect .jsonl files in the watched directory
   - tail each file (remember file position)
   - parse new lines with AgentTracker.process_line()
   - call AgentTracker.update_from_activity()

3. For low-compliance JSONL sources (pass rate < 0.8), the spec
   defines a Haiku LLM enhancement path. This is not yet implemented.
