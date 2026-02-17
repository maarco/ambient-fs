cross-machine sync (ambient-fs-server)
=======================================

status: design
created: 2026-02-16
affects: crates/ambient-fs-server/src/sync.rs (new),
         crates/ambient-fs-server/src/grpc.rs


beads: ambient-fs-8ow
----------------------

overview
--------

ambient-fs needs to sync file events across machines so
awareness data reflects changes made on any machine in a
team. this builds on the gRPC server (zwg).


architecture
------------

peer-to-peer sync, not hub-and-spoke. each daemon is both
a gRPC client and server. events flow bidirectionally.

  machine A (daemon)  <--gRPC-->  machine B (daemon)

each machine has a unique machine_id. events already carry
machine_id, so deduplication is straightforward: skip events
where machine_id == local machine_id.


sync protocol
--------------

  1. machine A connects to machine B's gRPC endpoint
  2. sends SyncEventsRequest:
     project_id: shared project
     machine_id: A's machine_id
     since_timestamp: last sync time for B
  3. B streams all events for project since timestamp
     where machine_id != A's machine_id
  4. A inserts remote events into local store
  5. simultaneously, B connects to A and does the same

  this is eventually consistent. both machines converge
  to the same event log (minus ordering within the same
  second).


implementation
--------------

  crates/ambient-fs-server/src/sync.rs (new):

  SyncManager struct:
    - state: Arc<ServerState>
    - peers: Vec<PeerConfig>   (addr, project_id)
    - sync_interval: Duration  (default: 30s)
    - last_sync: HashMap<String, i64>  (peer_addr -> timestamp)

  PeerConfig:
    - addr: String            (host:port)
    - project_id: String

  SyncManager methods:
    - new(state, peers) -> Self
    - run() -> loops forever, syncs each peer on interval
    - sync_peer(peer) -> connects, calls SyncEvents, inserts
    - handle_incoming_sync(request) -> streams local events

  dedup rules:
    - skip events with machine_id == local machine_id
    - skip events already in store (check by timestamp + file_path + machine_id)
    - insert via store.insert_batch()

  conflict handling:
    - no conflict resolution. all events are appended.
    - awareness query always uses latest event per file,
      regardless of which machine generated it.
    - this means "last write wins" by timestamp.


config
------

  config.toml:

  [sync]
  enabled = false
  peers = [
    { addr = "192.168.1.50:50051", project = "my-project" }
  ]
  interval_secs = 30


daemon integration
-------------------

  DaemonServer::run():
    if sync config exists and enabled:
      let sync_mgr = SyncManager::new(state.clone(), peers)
      tokio::spawn(sync_mgr.run())

  the sync manager is a background task. it doesn't block
  the main server loop.


test strategy
-------------

unit tests:
  - dedup logic: skip events with local machine_id
  - dedup logic: skip already-present events
  - last_sync tracking per peer

integration tests:
  - two daemon instances in-process
  - insert event on A, wait sync interval, verify on B
  - needs #[cfg(feature = "sync-tests")]


depends on
----------

  - ambient-fs-zwg (gRPC server)
  - SyncEvents RPC (defined in proto)
  - machine_id persistence (lsd, in progress)
