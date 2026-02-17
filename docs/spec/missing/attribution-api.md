attribution API
================

status: design
created: 2026-02-16
affects: crates/ambient-fs-server/src/protocol.rs,
         crates/ambient-fs-server/src/socket.rs,
         crates/ambient-fs-client/src/client.rs


overview
--------

external tools (Kollabor, Claude Code hooks, etc) need to
explicitly tell the daemon "this file was modified by X".
the watcher's EventAttributor does heuristic detection
(git, build, user), but AI agent attribution requires an
explicit API call.


9ie: attribution protocol method
-----------------------------------

  beads: ambient-fs-9ie

  problem:
    no protocol method to attribute a file change to a
    specific source. the only way events get source info
    is through EventAttributor heuristics (which can't
    detect AI agent vs user).

  implementation:

  protocol.rs additions:
    - add Method::Attribute to the Method enum

  socket.rs handler:
    Method::Attribute:
      1. extract params:
         file_path: String (required)
         project_id: String (required)
         source: String (required, one of: user, ai_agent, git, build, voice)
         source_id: Option<String> (chat id, agent name, etc)
      2. parse source string into Source enum
      3. create FileEvent with the given source:
         FileEvent::new(Modified, file_path, project_id, machine_id)
           .with_source(source)
           .with_source_id(source_id)
      4. insert into store via spawn_blocking
      5. broadcast via subscriptions
      6. return { "attributed": true }

  this is essentially "insert a synthetic event with
  explicit source". the daemon records it just like any
  other event, but the source comes from the caller
  instead of heuristic detection.

  JSON-RPC call:
    {
      "jsonrpc": "2.0",
      "method": "attribute",
      "params": {
        "file_path": "src/auth.rs",
        "project_id": "my-project",
        "source": "ai_agent",
        "source_id": "claude-opus-session-42"
      },
      "id": 5
    }

  response:
    { "attributed": true }

  error cases:
    - missing file_path -> invalid_params
    - missing project_id -> invalid_params
    - invalid source string -> invalid_params
    - store write failure -> internal_error


client.rs additions:
    pub async fn attribute(
        &mut self,
        project_id: &str,
        file_path: &str,
        source: &str,
        source_id: Option<&str>,
    ) -> Result<()>


integration with agent activity protocol
-------------------------------------------

when the AgentTracker detects an agent editing a file from
JSONL, it could optionally call the attribution method to
also record the event in the store. this creates a
permanent record beyond the transient agent tracker state.

not required for initial implementation. the attribution API
is useful standalone for external callers.


test strategy
-------------

unit tests:
  - attribute call inserts event with correct source
  - attribute call with source_id populates it
  - attribute call broadcasts to subscribers
  - invalid source string returns error
  - missing required params return error

integration tests:
  - attribute + query_events returns the attributed event
  - attribute + query_awareness shows correct modified_by
