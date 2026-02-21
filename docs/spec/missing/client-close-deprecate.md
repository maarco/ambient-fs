client close() and deprecation cleanup
=======================================

status: pending
created: 2026-02-17
affects: crates/ambient-fs-client/src/client.rs

current architecture (post-streaming rewrite)
----------------------------------------------

the client already has:
  - split stream: writer (OwnedWriteHalf) + background reader task
  - pending map: Arc<Mutex<HashMap<u64, oneshot::Sender>>>
  - notification channel: mpsc::channel(256) for ClientNotification
  - Drop impl that aborts reader_handle
  - is_connected() that checks reader_handle.is_finished()

what's missing:
  - explicit close() method (currently relies on Drop)
  - deprecated watch()/events() methods still present


part 1: close() method
-----------------------

consumes self, gracefully shuts down. different from Drop because
it can do async cleanup (drain pending, flush writer).

```rust
/// Gracefully close the client connection.
///
/// Aborts the background reader task, fails all pending requests with
/// ConnectionClosed, and drops the write half (closing the socket).
///
/// Prefer this over just dropping the client when you want to ensure
/// clean shutdown ordering.
pub async fn close(self) {
    // Abort reader task first (stops routing)
    self.reader_handle.abort();

    // Fail all pending requests
    let mut map = self.pending.lock().await;
    for (_, tx) in map.drain() {
        let _ = tx.send(Err(ClientError::ConnectionClosed));
    }

    // writer + notification_rx dropped here (implicit)
}
```

returns nothing (not Result). closing can't fail in a meaningful way --
the socket close and task abort are infallible. keeping the signature
simple.

existing Drop impl stays unchanged (abort reader_handle). close() is
the polite version that also drains pending requests.


part 2: remove deprecated methods
----------------------------------

methods to remove from client.rs:

  1. watch(&mut self, path: &str) -> Result<()>
     replaced by: watch_project(path) -> Result<String>

  2. events(&mut self, filter: EventFilter) -> Result<Vec<FileEvent>>
     replaced by: query_events(filter) -> Result<Vec<FileEvent>>

these are the old API from before the protocol was finalized.
watch_project and query_events are the correct server method names.

tests to remove/update:

  - any test calling client.watch() should use watch_project()
  - any test calling client.events() should use query_events()
  - check: `grep -n "\.watch(" crates/ambient-fs-client/src/client.rs`
  - check: `grep -n "\.events(" crates/ambient-fs-client/src/client.rs`

integration packages already use the new names:
  - node: watchProject(), queryEvents()
  - python: watch_project(), query_events()


tests
-----

```rust
#[tokio::test]
async fn close_aborts_reader_and_drains_pending() {
    let (client, _server) = mock_client();

    // Insert a pending request that will never get a response
    let (tx, rx) = oneshot::channel();
    client.pending.lock().await.insert(99, tx);

    client.close().await;

    // The pending request should have received ConnectionClosed
    let result = rx.await.unwrap();
    assert!(matches!(result, Err(ClientError::ConnectionClosed)));
}

#[tokio::test]
async fn close_makes_notification_stream_end() {
    let (mut client, _server) = mock_client();
    let mut rx = client.take_notification_stream().unwrap();

    client.close().await;

    // Notification channel should be closed
    assert!(rx.recv().await.is_none());
}

#[tokio::test]
async fn drop_still_works_without_close() {
    let (client, _server) = mock_client();
    assert!(client.is_connected());
    drop(client);
    // No panic, reader aborted via Drop
}
```


files to modify
----------------

1. crates/ambient-fs-client/src/client.rs
   - add close() method to impl AmbientFsClient
   - remove watch() method
   - remove events() method
   - remove/update tests that reference deprecated methods

2. no changes to lib.rs (no new types)
3. no changes to builder.rs


migration guide
---------------

```rust
// old:
client.watch("/path").await?;
let events = client.events(filter).await?;

// new:
let project_id = client.watch_project("/path").await?;
let events = client.query_events(filter).await?;

// when done:
client.close().await;
```
