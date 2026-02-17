daemon stubs (ambient-fsd)
=========================

status: stub implementations found by agents
found: 2026-02-16
affects: crates/ambient-fsd/src/daemon.rs, crates/ambient-fsd/src/server.rs


f2q: stdio redirection in daemon
---------------------------------

  beads: ambient-fs-f2q
  file:  daemon.rs:326-335

  what's there:
    comment block showing what dup2 calls WOULD do.
    log file is opened for append but never wired up.
    stdout/stderr still go to the terminal.

  what's needed:
    use nix::unistd::dup2 to redirect fd 1 (stdout) and
    fd 2 (stderr) to the log file descriptor.

  implementation:
    - add nix crate dependency (unistd feature)
    - use std::os::fd::AsRawFd to get log fd
    - dup2(log.as_raw_fd(), 1) for stdout
    - dup2(log.as_raw_fd(), 2) for stderr
    - handle errors (dup2 returns Result)

  test strategy:
    tricky -- dup2 in tests redirects the test runner's
    output. options:
    - fork a child process and verify log file gets output
    - integration test that spawns the daemon binary and
      checks log file contents
    - skip unit test, rely on integration test only

  open questions:
    - should we also redirect stdin to /dev/null?
    - should we support --foreground mode that skips dup2?
      (useful for debugging, docker, systemd)


yf2: watcher stop() for clean shutdown
---------------------------------------

  beads: ambient-fs-yf2
  file:  server.rs:151

  what's there:
    shutdown() sets running=false, shuts down socket server,
    but has a comment: "watcher doesn't have stop() in the
    current API"

  what's needed:
    FsWatcher needs a stop() method that:
    - drops the notify::Watcher handle
    - closes the event channel
    - lets pending events drain (or not -- design choice)

  implementation:
    crates/ambient-fs-watcher/src/watcher.rs:
    - add stop(&mut self) that drops the internal watcher
    - close the mpsc sender so receiver gets None
    - set an internal stopped flag

    crates/ambient-fsd/src/server.rs:151:
    - call watcher.lock().await.stop() during shutdown

  test strategy:
    - unit test: start watcher, call stop(), verify channel
      closes (recv returns None)
    - unit test: stop is idempotent (calling twice is ok)


6n0: unwatch_project implementation
------------------------------------

  beads: ambient-fs-6n0
  file:  server.rs:176-181

  what's there:
    takes project_id, logs it, returns Ok(()). doesn't
    actually unwatch anything.

  what's needed:
    - track project_id -> watched path mapping (currently
      only path -> project_id exists via generate_project_id)
    - call watcher.unwatch(path) to stop filesystem events
    - remove project from store tracking

  implementation:
    server.rs needs a HashMap<String, PathBuf> to map
    project_id back to its path:
    - add field: projects: Arc<Mutex<HashMap<String, PathBuf>>>
    - populate in watch_project()
    - look up in unwatch_project() to get path
    - call watcher.unwatch(path)
    - remove from projects map

    also need FsWatcher::unwatch(path) method -- check if
    notify crate's Watcher trait has unwatch/remove_path.

  depends on:
    - yf2 (watcher API changes)


0d2: wire store.add_project in watch_project
---------------------------------------------

  beads: ambient-fs-0d2
  file:  server.rs:166-170

  what's there:
    spawn_blocking that logs the project but skips the
    actual store call. comment: "add_project doesn't exist
    yet"

  what's needed:
    EventStore needs an add_project method that persists
    the project_id <-> path mapping in SQLite.

  implementation:
    crates/ambient-fs-store/src/store.rs:
    - add projects table: (project_id TEXT PK, path TEXT,
      created_at INTEGER)
    - add_project(id, path) inserts into projects table
    - remove_project(id) deletes from projects table
    - get_project_path(id) -> Option<PathBuf>
    - list_projects() -> Vec<(String, PathBuf)>

    crates/ambient-fsd/src/server.rs:166:
    - replace the no-op with store.add_project(project_id, path)

  depends on:
    - ambient-fs-store migration for projects table
