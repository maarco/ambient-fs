client reconnect logic
======================

status: design
created: 2026-02-17
affects: crates/ambient-fs-client/src/client.rs,
         crates/ambient-fs-client/src/builder.rs

overview
--------

current client fails permanently when the unix socket drops.
daemon restarts, network blips, or laptop sleep all cause
permanent client failure.

this spec adds auto-reconnect with exponential backoff,
subscription recovery, and connection state tracking.


current architecture (post-streaming rewrite)
----------------------------------------------

the client has split-stream architecture:

  struct AmbientFsClient {
      socket_path: PathBuf,
      writer: OwnedWriteHalf,
      pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<JsonValue>>>>>,
      notification_rx: Option<mpsc::Receiver<ClientNotification>>,
      reader_handle: JoinHandle<()>,
      next_id: AtomicU64,
  }

reconnect must rebuild the ENTIRE pipeline:
  1. create new UnixStream
  2. split into new read/write halves
  3. create new mpsc channel for notifications
  4. spawn new reader task
  5. swap writer
  6. re-subscribe to all previously subscribed projects

this is more complex than swapping a single stream field.


reconnect strategy
------------------

exponential backoff (no external deps):

  attempt 1:  100ms
  attempt 2:  200ms
  attempt 3:  400ms
  attempt 4:  800ms
  attempt 5:  1.6s
  attempt 6:  3.2s
  attempt 7:  6.4s
  attempt 8:  12.8s
  attempt 9+: 30s (capped)

formula:
  delay = min(base * 2^attempt, max_backoff)

no jitter for v1 (avoids adding rand dependency).
can add later if thundering herd becomes a real problem.

defaults:
  base_backoff: 100ms
  max_backoff: 30s
  max_retries: None (infinite)


when to reconnect
-----------------

reconnect triggered by:

  1. reader task receives EOF (read_line returns 0 bytes)
  2. reader task receives io::Error (connection reset, broken pipe)
  3. write fails with io::Error in send_request

not reconnecting:

  - explicit close() call
  - auto_reconnect=false in builder
  - max retries exceeded


ClientState enum
----------------

public enum for tracking connection status:

  #[derive(Debug, Clone, PartialEq)]
  pub enum ClientState {
      Connected,
      Reconnecting { attempt: u32 },
      Disconnected,
  }

  impl ClientState {
      pub fn is_connected(&self) -> bool {
          matches!(self, Self::Connected)
      }
  }

exposed via:
  client.state() -> ClientState


internal reconnect state
------------------------

new fields on AmbientFsClient:

  struct AmbientFsClient {
      // ... existing fields ...

      // reconnect state
      subscriptions: HashSet<String>,          // project IDs to re-subscribe
      auto_reconnect: bool,
      backoff: Backoff,
      max_retries: Option<u32>,
      state: Arc<RwLock<ClientState>>,
      state_tx: tokio::sync::watch::Sender<ClientState>,
  }

subscribe() adds to HashSet before sending RPC.
unsubscribe() removes from HashSet after success.


state change channel
--------------------

use tokio::sync::watch for state observation:

  let (state_tx, state_rx) = tokio::sync::watch::channel(ClientState::Connected);

caller gets state_rx from builder:

  let (client, mut state_rx) = AmbientFsClientBuilder::new()
      .auto_reconnect(true)
      .build()
      .await?;

  tokio::spawn(async move {
      while state_rx.changed().await.is_ok() {
          let state = state_rx.borrow().clone();
          println!("connection state: {:?}", state);
      }
  });

watch channel is better than callback because:
  - no Arc<dyn Fn> complexity
  - naturally async
  - caller decides when to poll
  - no panic handling needed


reconnect implementation
------------------------

triggered from reader task when connection drops:

  async fn reconnect_loop(
      socket_path: PathBuf,
      backoff: Backoff,
      max_retries: Option<u32>,
      subscriptions: Arc<Mutex<HashSet<String>>>,
      pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<JsonValue>>>>>,
      notification_tx: mpsc::Sender<ClientNotification>,
      writer: Arc<Mutex<OwnedWriteHalf>>,
      state_tx: watch::Sender<ClientState>,
      next_id: Arc<AtomicU64>,
  ) {
      // 1. fail all pending requests immediately
      {
          let mut map = pending.lock().await;
          for (_, tx) in map.drain() {
              let _ = tx.send(Err(ClientError::ConnectionClosed));
          }
      }

      // 2. attempt reconnect with backoff
      for attempt in 0.. {
          if let Some(max) = max_retries {
              if attempt >= max {
                  let _ = state_tx.send(ClientState::Disconnected);
                  return;
              }
          }

          let _ = state_tx.send(ClientState::Reconnecting { attempt });

          let delay = backoff.delay(attempt);
          tokio::time::sleep(delay).await;

          match UnixStream::connect(&socket_path).await {
              Ok(stream) => {
                  let (read_half, write_half) = stream.into_split();

                  // swap writer
                  *writer.lock().await = write_half;

                  // re-subscribe
                  let subs = subscriptions.lock().await.clone();
                  for project_id in &subs {
                      // send subscribe request through new writer
                      // (simplified - actual impl reuses send_request)
                  }

                  let _ = state_tx.send(ClientState::Connected);

                  // restart reader loop on new read_half
                  // (this function is called FROM the reader task,
                  //  so we just return and the caller loops)
                  return;
              }
              Err(_) => continue,
          }
      }
  }

key insight: reconnect runs INSIDE the reader task. when the reader
detects EOF, it runs reconnect_loop, which creates a new stream and
swaps the writer. then the reader task continues its loop with the
new read half.

this means the writer must be behind Arc<Mutex<>> so both the reader
task (during reconnect) and send_request can access it. this is a
structural change from the current &mut self pattern.


architectural change: Arc<Mutex> writer
-----------------------------------------

current: writer is owned by client, send_request takes &mut self
after:   writer behind Arc<Mutex>, send_request can take &self

  struct AmbientFsClient {
      writer: Arc<Mutex<OwnedWriteHalf>>,
      // ... rest unchanged
  }

  async fn send_request<T: Serialize>(&self, ...) -> Result<JsonValue> {
      let mut writer = self.writer.lock().await;
      writer.write_all(request_json.as_bytes()).await?;
      // ...
  }

this also enables concurrent requests without &mut self.
win-win.


pending requests during reconnect
----------------------------------

pending requests fail immediately with ConnectionClosed.
no request queuing. caller must retry or handle failure.

rationale: queuing introduces unbounded memory growth and
complex timeout semantics. fail-fast is simpler and safer.

new requests during reconnect also fail immediately:

  if !self.state().is_connected() {
      return Err(ClientError::NotConnected);
  }


backoff implementation
----------------------

no external deps (no rand):

  struct Backoff {
      base: Duration,
      max: Duration,
  }

  impl Backoff {
      fn delay(&self, attempt: u32) -> Duration {
          let multiplier = 1u64.checked_shl(attempt.min(30)).unwrap_or(u64::MAX);
          let exponential = self.base.saturating_mul(multiplier as u32);
          exponential.min(self.max)
      }
  }


builder changes
---------------

  impl AmbientFsClientBuilder {
      pub fn auto_reconnect(mut self, enabled: bool) -> Self
      pub fn max_retries(mut self, n: u32) -> Self
      pub fn reconnect_backoff(mut self, base: Duration, max: Duration) -> Self
      pub async fn build(self) -> Result<(AmbientFsClient, watch::Receiver<ClientState>)>
  }

defaults:
  auto_reconnect: true
  max_retries: None (infinite)
  reconnect_backoff: (100ms, 30s)

build() returns tuple of (client, state_receiver).


error handling
--------------

new ClientError variant:

  #[error("max reconnect attempts exceeded")]
  MaxRetriesExceeded,


test plan
---------

unit tests (no real socket):

  - Backoff::delay follows exponential curve
  - Backoff::delay caps at max_backoff
  - Backoff::delay handles attempt=30+ without panic
  - ClientState equality and is_connected()
  - builder stores reconnect config

integration tests (UnixListener mock):

  test: reconnect_after_server_drops
    1. create UnixListener, accept connection
    2. connect client with auto_reconnect=true
    3. subscribe to "project-1"
    4. drop server connection
    5. accept new connection on listener
    6. verify client re-subscribes to "project-1"
    7. verify client.state() == Connected

  test: max_retries_exceeded
    1. connect, then drop listener entirely
    2. configure max_retries=3
    3. verify client gives up after 3 attempts
    4. verify state == Disconnected

  test: pending_requests_fail_on_disconnect
    1. connect, drop server
    2. send request
    3. verify ConnectionClosed error (not hang)

  test: state_watch_receives_transitions
    1. connect -> verify Connected
    2. drop server -> verify Reconnecting{0}
    3. accept -> verify Connected again

  test: requests_fail_during_reconnect
    1. connect, drop server
    2. while reconnecting, call query_events
    3. verify immediate NotConnected error


implementation order
--------------------

1. add Backoff struct + tests
2. add ClientState enum + tests
3. change writer to Arc<Mutex<OwnedWriteHalf>>
4. change send_request to &self
5. add subscriptions HashSet tracking
6. add watch channel for state
7. extend builder with reconnect options
8. implement reconnect_loop in reader task
9. integration tests with UnixListener


dependencies
------------

no new crate dependencies. uses only:
  - tokio::sync::watch (already available)
  - tokio::sync::Mutex (already used)
  - std::collections::HashSet (stdlib)
