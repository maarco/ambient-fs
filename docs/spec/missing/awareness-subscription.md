awareness change subscription (P3-3)
======================================

status: design
created: 2026-02-16
affects: crates/ambient-fs-server/src/subscriptions.rs,
         crates/ambient-fs-server/src/socket.rs,
         crates/ambient-fs-server/src/protocol.rs


overview
--------

current subscribe sends raw FileEvents to clients.
the spec says clients should also receive FileAwareness
changes. a client rendering a file tree needs to know when
a file's awareness state changes (modified_by changed,
todo_count changed, active_agent appeared, etc), not just
that a raw filesystem event occurred.


design: two notification types on same channel
-----------------------------------------------

don't create a separate subscription system. extend the
existing one to carry different notification types.

current: subscribe("my-project") -> stream of FileEvent
new:     subscribe("my-project") -> stream of Notification

  Notification enum:
    FileEvent(FileEvent)                    existing
    AwarenessChanged(AwarenessChange)       new
    AnalysisComplete(AnalysisNotification)  new
    TreePatch(TreePatch)                    future (7t4)

clients receive all notification types on one stream.
they filter by what they care about. JSON-RPC notifications
use "method" field to distinguish:

  {"jsonrpc":"2.0","method":"event","params":{<FileEvent>}}
  {"jsonrpc":"2.0","method":"awareness_changed","params":{<AwarenessChange>}}
  {"jsonrpc":"2.0","method":"analysis_complete","params":{<AnalysisNotification>}}


AwarenessChange struct
-----------------------

  {
    "project_id": "my-project",
    "file_path": "src/auth.rs",
    "changes": {
      "modified_by": "ai_agent",
      "change_frequency": "hot",
      "active_agent": "opus-42"
    },
    "awareness": { <full FileAwareness> }
  }

  "changes" is a sparse object -- only fields that changed.
  "awareness" is the full current state for convenience.
  clients can use either: changes for incremental updates,
  awareness for full replacement.


when to emit awareness_changed
-------------------------------

  1. after a FileEvent is stored:
     build_awareness() for the file, compare with last known.
     if different: emit AwarenessChange.

  2. after analysis completes:
     todo_count, lint_hints, line_count may have changed.
     build_awareness(), compare, emit if different.

  3. after agent state changes:
     active_agent appeared or disappeared.
     build_awareness(), compare, emit if different.


implementation
--------------

  approach: awareness snapshot cache in ServerState.

  ServerState additions:
    awareness_cache: Arc<RwLock<HashMap<(String, String), FileAwareness>>>
    (key: (project_id, file_path))

  AwarenessNotifier (new, in pipeline.rs or awareness.rs):

    async fn check_and_notify(
      state: &ServerState,
      project_id: &str,
      file_path: &str,
    ):
      1. build_awareness(state, project_id, file_path).await
      2. get previous from awareness_cache
      3. if different or not in cache:
         a. compute changes (diff the two FileAwareness)
         b. update cache
         c. broadcast AwarenessChange to subscribers
      4. if same: skip

  diff_awareness(old: &FileAwareness, new: &FileAwareness) -> Option<Changes>:
    compare each field, return None if identical.
    return Some(Changes) with only the changed fields.


subscription manager changes
-------------------------------

  SubscriptionManager currently uses:
    broadcast::Sender<FileEvent>

  change to:
    broadcast::Sender<Notification>

  where Notification is an enum wrapping FileEvent,
  AwarenessChange, AnalysisNotification, etc.

  this is a breaking change to the internal API but
  doesn't affect the JSON-RPC wire protocol (that already
  uses generic JSON objects with "method" field).

  alternatively: keep SubscriptionManager as-is for FileEvents,
  add a second AwarenessSubscriptionManager for awareness.
  simpler change, less elegant.

  recommended: extend SubscriptionManager to be generic over
  notification type, or use Notification enum. the enum
  approach is simpler:

  enum Notification {
    Event(FileEvent),
    AwarenessChanged(AwarenessChange),
    AnalysisComplete(AnalysisNotification),
  }

  SubscriptionManager changes:
    broadcast::Sender<Notification>
    broadcast() takes Notification instead of FileEvent
    add publish_event(event) shorthand
    add publish_awareness(change) shorthand


socket.rs changes
------------------

  the notification forwarding task (spawned after subscribe)
  reads from the broadcast receiver and writes JSON-RPC
  notifications to the client stream.

  currently it only handles FileEvent.
  change to match on Notification enum:

  match notification {
    Notification::Event(event) => {
      write json: {"method":"event","params":{event}}
    }
    Notification::AwarenessChanged(change) => {
      write json: {"method":"awareness_changed","params":{change}}
    }
    Notification::AnalysisComplete(notif) => {
      write json: {"method":"analysis_complete","params":{notif}}
    }
  }


client.rs changes
------------------

  EventStream (from client-api spec) needs to yield
  Notification instead of just FileEvent.

  or: provide separate filtered streams:
    subscribe_events(project_id) -> Stream<FileEvent>
    subscribe_awareness(project_id) -> Stream<AwarenessChange>
    subscribe_all(project_id) -> Stream<Notification>

  the filtered variants just wrap subscribe_all + filter.


call sites
-----------

  1. DaemonServer event loop (after storing event):
     pipeline.check_and_notify(state, event.project_id, event.file_path)

  2. AnalysisPipeline (after caching analysis):
     check_and_notify(state, project_id, file_path)

  3. AgentTracker (after updating agent state):
     for each file the agent touches:
       check_and_notify(state, project_id, file_path)


test strategy
-------------

unit tests:
  - diff_awareness returns None for identical
  - diff_awareness returns changes for different modified_by
  - diff_awareness returns changes for different todo_count
  - diff_awareness returns changes for different active_agent
  - check_and_notify emits on first call (no cache)
  - check_and_notify skips when awareness unchanged
  - check_and_notify emits when field changes
  - Notification enum serializes correctly for each variant

integration tests:
  - subscribe, trigger file event, receive awareness_changed
  - subscribe, trigger analysis, receive awareness_changed
    with updated todo_count


depends on
----------

  - build_awareness() (hra, in progress)
  - SubscriptionManager (done, needs modification)
  - analysis pipeline (P2-3, this spec)
