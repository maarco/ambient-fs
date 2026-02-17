client API completion
=====================

status: design
created: 2026-02-16
affects: crates/ambient-fs-client/src/client.rs,
         crates/ambient-fs-client/src/builder.rs (new)


overview
--------

ambient-fs-client has basic connect + watch + events + subscribe.
missing: builder pattern, awareness queries, unsubscribe,
unwatch, attribute, agent queries. these match the server
protocol methods.


builder.rs (new file)
---------------------

fluent API for constructing the client:

  AmbientFsClientBuilder::new()
    .socket_path("/tmp/ambient-fs.sock")  // optional, has default
    .connect_timeout(Duration::from_secs(5))  // optional
    .build()
    .await?

  struct AmbientFsClientBuilder {
    socket_path: PathBuf,
    connect_timeout: Option<Duration>,
  }

  impl AmbientFsClientBuilder {
    pub fn new() -> Self
    pub fn socket_path(mut self, path: impl Into<PathBuf>) -> Self
    pub fn connect_timeout(mut self, timeout: Duration) -> Self
    pub async fn build(self) -> Result<AmbientFsClient>
  }

  build() calls AmbientFsClient::connect() with timeout wrap.


missing client methods
-----------------------

each maps 1:1 to a server protocol method:

  watch_project(path: &str) -> Result<String>
    calls "watch_project", returns project_id

  unwatch_project(project_id: &str) -> Result<()>
    calls "unwatch_project"

  unsubscribe(project_id: &str) -> Result<()>
    calls "unsubscribe"

  query_events(filter: EventFilter) -> Result<Vec<FileEvent>>
    calls "query_events" (rename from events())

  query_awareness(project_id: &str, path: &str) -> Result<FileAwareness>
    calls "query_awareness"

  attribute(project_id: &str, file_path: &str, source: &str,
            source_id: Option<&str>) -> Result<()>
    calls "attribute" (from attribution-api spec)

  query_agents() -> Result<Vec<AgentInfo>>
    calls "query_agents" (if agent tracker exposes this)

  existing watch() and events() should be kept as aliases
  for backward compat.


streaming subscription support
-------------------------------

current subscribe() sends the request but doesn't handle
incoming notifications. need:

  subscribe_stream(project_id: &str) -> Result<EventStream>

  EventStream wraps the UnixStream reader and yields
  FileEvent as they arrive. this requires splitting the
  stream (read/write halves) so requests and notifications
  can flow independently.

  implementation:
    use tokio::io::split() on the UnixStream
    store WriteHalf in client for requests
    EventStream holds ReadHalf + BufReader

    after subscribe_stream():
    - sends subscribe request via write half
    - reads response from read half (confirmation)
    - returns EventStream that reads from read half
    - EventStream::next() reads lines, parses notifications
    - regular request/response still works via write half

  this is the most complex part. defer to a follow-up if
  needed - the basic subscribe() that just sends the request
  is sufficient for v1.


test strategy
-------------

unit tests (mock/no daemon):
  - builder defaults are correct
  - builder custom path works
  - EventFilter serialization
  - all client methods construct correct JSON-RPC requests

integration tests (needs running daemon):
  - connect + watch_project + query_events roundtrip
  - connect + watch_project + query_awareness roundtrip
  - connect + subscribe + receive notification
  - attribute + query confirms source
