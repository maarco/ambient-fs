gRPC server (ambient-fs-server)
================================

status: design
created: 2026-02-16
affects: crates/ambient-fs-server/src/grpc.rs (new),
         proto/ambient_fs.proto (new),
         crates/ambient-fs-server/Cargo.toml


beads: ambient-fs-zwg
----------------------

overview
--------

the daemon currently only exposes a Unix socket JSON-RPC API.
for cross-machine sync and remote clients, a gRPC server is
needed. this is the transport layer -- the actual sync logic
(8ow) sits on top of it.


proto definition
-----------------

  proto/ambient_fs.proto:

  syntax = "proto3";
  package ambient_fs;

  service AmbientFs {
    // unary RPCs (same as JSON-RPC methods)
    rpc WatchProject(WatchProjectRequest) returns (WatchProjectResponse);
    rpc UnwatchProject(UnwatchProjectRequest) returns (UnwatchProjectResponse);
    rpc QueryEvents(QueryEventsRequest) returns (QueryEventsResponse);
    rpc QueryAwareness(QueryAwarenessRequest) returns (AwarenessResponse);
    rpc Attribute(AttributeRequest) returns (AttributeResponse);

    // streaming RPCs (new, not in JSON-RPC)
    rpc Subscribe(SubscribeRequest) returns (stream FileEventMessage);
    rpc SyncEvents(SyncEventsRequest) returns (stream FileEventMessage);
  }

  message FileEventMessage {
    int64 timestamp = 1;
    string event_type = 2;
    string file_path = 3;
    string project_id = 4;
    string source = 5;
    optional string source_id = 6;
    string machine_id = 7;
    optional string content_hash = 8;
    optional string old_path = 9;
  }

  message WatchProjectRequest { string path = 1; }
  message WatchProjectResponse { bool watching = 1; string project_id = 2; }

  message UnwatchProjectRequest { string project_id = 1; }
  message UnwatchProjectResponse { bool success = 1; }

  message QueryEventsRequest {
    optional string project_id = 1;
    optional int64 since = 2;
    optional string source = 3;
    optional int32 limit = 4;
  }
  message QueryEventsResponse { repeated FileEventMessage events = 1; }

  message QueryAwarenessRequest {
    string project_id = 1;
    string path = 2;
  }
  message AwarenessResponse {
    string file_path = 1;
    string project_id = 2;
    optional string last_modified = 3;
    optional string modified_by = 4;
    string change_frequency = 5;
    int32 todo_count = 6;
    int32 lint_hints = 7;
    int32 line_count = 8;
    optional string active_agent = 9;
  }

  message AttributeRequest {
    string file_path = 1;
    string project_id = 2;
    string source = 3;
    optional string source_id = 4;
  }
  message AttributeResponse { bool attributed = 1; }

  message SubscribeRequest { string project_id = 1; }

  message SyncEventsRequest {
    string project_id = 1;
    string machine_id = 2;
    int64 since_timestamp = 3;
  }


implementation
--------------

  crates/ambient-fs-server/src/grpc.rs (new):

  dependencies:
    tonic = "0.12"
    prost = "0.13"
    tonic-build = "0.12"  (build dep)

  build.rs:
    tonic_build::compile_protos("../../proto/ambient_fs.proto")

  GrpcServer struct:
    - state: Arc<ServerState>
    - addr: SocketAddr (default 0.0.0.0:50051)
    - run() starts tonic server

  AmbientFsService impl:
    - each RPC method delegates to the same logic as JSON-RPC handlers
    - convert protobuf request -> internal types
    - call existing store/watcher/state methods
    - convert internal types -> protobuf response

  Subscribe streaming:
    - same as JSON-RPC subscribe but uses tonic::Streaming
    - calls state.subscriptions.subscribe()
    - wraps broadcast::Receiver in a ReceiverStream
    - converts FileEvent -> FileEventMessage per item

  the handlers should be thin wrappers around the same
  business logic used by socket.rs handlers. extract shared
  logic into functions if not already done.


daemon integration
-------------------

  crates/ambient-fsd/src/server.rs:
    - add optional grpc_addr: Option<SocketAddr> to ServerConfig
    - in DaemonServer::run(), if grpc_addr is Some:
      tokio::spawn(grpc_server.run())
    - grpc server shares the same Arc<ServerState>

  crates/ambient-fsd/src/config.rs:
    - add grpc_port: Option<u16> to DaemonConfig
    - default: None (gRPC disabled by default)


test strategy
-------------

unit tests:
  - proto compiles without errors
  - request/response type conversions correct
  - FileEvent -> FileEventMessage roundtrip preserves data

integration tests:
  - start gRPC server, call WatchProject, verify response
  - start gRPC server, Subscribe, trigger event, verify stream
  - needs #[cfg(feature = "grpc-tests")]


depends on
----------

  - ServerState (done)
  - all socket handlers (done, share logic)
  - tonic/prost workspace dependencies (new)
