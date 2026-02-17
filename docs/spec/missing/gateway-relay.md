gateway relay (P6-3)
=====================

status: design
created: 2026-02-16
affects: new crate or binary: ambient-fs-gateway


overview
--------

when two machines can't connect peer-to-peer (NAT, firewalls,
different networks), a relay server bridges them. each daemon
connects outbound to the gateway, and the gateway routes
events between them.

from the main spec:

  machine A  <--gRPC-->  gateway  <--gRPC-->  machine B

this is optional. peer-to-peer sync (8ow) works without it
when machines can reach each other directly.


architecture
------------

the gateway is a standalone binary (or optional mode of
ambient-fsd). it does NOT store events long-term. it's a
stateless relay with short-term buffering.

  ambient-fs-gateway:
    - accepts gRPC connections from daemons
    - each daemon registers with project_id + machine_id
    - gateway maintains a routing table:
      project_id -> [connected daemons]
    - when daemon A sends events, gateway forwards to all
      other daemons registered for the same project
    - short-term buffer (last 5 min) for daemons that
      reconnect after brief disconnect


components
-----------

  crate: ambient-fs-gateway (new, or subcommand of ambient-fsd)

  GatewayServer:
    - addr: SocketAddr (default 0.0.0.0:50052)
    - connections: HashMap<String, Vec<DaemonConnection>>
      (project_id -> connected daemons)
    - buffer: HashMap<String, VecDeque<FileEvent>>
      (project_id -> recent events, capped)

  DaemonConnection:
    - machine_id: String
    - stream: tonic streaming sender
    - connected_at: Instant
    - last_seen: Instant

  gRPC service:

    service AmbientFsGateway {
      rpc Register(RegisterRequest) returns (RegisterResponse);
      rpc Relay(stream FileEventMessage) returns (stream FileEventMessage);
    }

    Register: daemon tells gateway which projects it wants
    Relay: bidirectional stream, daemon sends local events,
           receives remote events from other daemons


relay flow
-----------

  1. daemon A connects, calls Register(project="my-project", machine="A")
  2. daemon B connects, calls Register(project="my-project", machine="B")
  3. gateway knows: my-project -> [A, B]
  4. A sends event via Relay stream
  5. gateway receives, forwards to B's Relay stream
  6. B sends event via Relay stream
  7. gateway receives, forwards to A's Relay stream

  events already carry machine_id, so daemons can dedup
  (skip events with their own machine_id).


daemon-side changes
--------------------

  SyncManager (from grpc-sync.md) needs a gateway mode:

  SyncConfig:
    mode: "peer" | "gateway"
    gateway_addr: Option<String>   (for gateway mode)
    peers: Vec<PeerConfig>         (for peer mode)

  in gateway mode:
    - connect to gateway instead of peers
    - call Register for each watched project
    - open Relay stream
    - send local events on stream
    - receive remote events from stream
    - insert remote events into store (same dedup logic)


config
------

  config.toml:

  [sync]
  enabled = true
  mode = "gateway"
  gateway_addr = "sync.example.com:50052"

  or for self-hosted:

  [sync]
  enabled = true
  mode = "gateway"
  gateway_addr = "192.168.1.100:50052"


gateway config (separate file or CLI flags):

  ambient-fs-gateway --port 50052 --buffer-minutes 5


authentication
--------------

  v1: no auth (trusted network)
  v2: shared secret token in config
  v3: mTLS or JWT

  for now, no auth. the gateway is meant for trusted
  environments (home network, team VPN, etc).


test strategy
-------------

unit tests:
  - routing table: register adds daemon, deregister removes
  - relay: event from A forwarded to B but not back to A
  - buffer: stores last N minutes, oldest evicted
  - disconnect: daemon removed from routing table

integration tests:
  - two daemons + gateway, event flows A -> gateway -> B
  - daemon disconnects, reconnects, gets buffered events
  - three daemons, event from A reaches B and C


depends on
----------

  - gRPC server (zwg)
  - cross-machine sync (8ow)
  - SyncManager gateway mode
  - proto definitions for Register + Relay RPCs
