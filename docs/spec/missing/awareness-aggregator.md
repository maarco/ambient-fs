awareness aggregator
====================

status: design
created: 2026-02-16
affects: crates/ambient-fs-server/src/awareness.rs (new),
         crates/ambient-fs-server/src/state.rs


overview
--------

the query_awareness handler (68e) needs to combine data from
multiple sources into a single FileAwareness response:

  1. event store  -> last_modified, modified_by, change_frequency
  2. analysis cache -> todo_count, lint_hints, line_count,
                       imports, exports
  3. agent tracker -> active_agent

no component currently does this aggregation. each source
exists independently.


architecture
------------

one function, not a struct. takes references to state and
returns FileAwareness. pure aggregation logic.

  crates/ambient-fs-server/src/awareness.rs:

  pub async fn build_awareness(
      state: &ServerState,
      project_id: &str,
      file_path: &str,
  ) -> Result<FileAwareness>

  steps:
    1. open store (spawn_blocking, rusqlite is sync)
    2. store.get_latest(project_id, file_path) -> Option<FileEvent>
    3. open cache (spawn_blocking)
    4. cache.get(project_id, file_path) -> Option<FileAnalysis>
    5. state.get_active_agent(file_path) -> Option<String>
    6. combine into FileAwareness:

  from event (or defaults if None):
    file_path:       file_path param
    project_id:      project_id param
    last_modified:   event.timestamp (or None)
    modified_by:     event.source.as_str()
    modified_by_label: event.source_id (if AI) or source label
    change_frequency: ChangeFrequency::from_age(event.timestamp)

  from analysis (or defaults if None):
    todo_count:      analysis.todo_count (or 0)
    lint_hints:      analysis.lint_hints.len() (or 0)
    line_count:      analysis.line_count (or 0)

  from agent tracker:
    active_agent:    active_agent (or None)

  chat_references stays 0 for now (ynk is deferred).


query_awareness handler integration
-------------------------------------

the Phase2-QueryAwareness agent is building the socket handler.
it should call build_awareness() instead of building the
response inline. this keeps the handler thin.

  socket.rs handle_request for QueryAwareness:
    1. extract project_id, path from params
    2. call build_awareness(state, project_id, path).await
    3. serialize to JSON, return

  error cases:
    - missing project_id param -> invalid_params error
    - missing path param -> invalid_params error
    - store/cache errors -> internal_error (log, don't expose)


project-level awareness (future)
----------------------------------

  build_project_awareness(state, project_id) -> Vec<FileAwareness>

  queries all recent events for a project, builds awareness
  for each unique file_path. expensive, should be cached or
  paginated. not implementing now but the function signature
  should support it.


test strategy
-------------

unit tests (mock data, no real store):
  - build_awareness with event + analysis + agent returns
    fully populated FileAwareness
  - build_awareness with event only (no analysis, no agent)
    returns partial FileAwareness with zeros
  - build_awareness with no event returns default/empty
  - change_frequency calculated correctly from event age
  - active_agent populated from tracker

integration tests:
  - create store, insert event, build awareness, verify fields
  - create store + cache, insert both, verify combined result
