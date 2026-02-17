daemon stubs (ambient-fsd)
=========================

status: stub implementations found by agents
found: 2026-02-16
affects: crates/ambient-fsd/src/daemon.rs, crates/ambient-fsd/src/server.rs


f2q: stdio redirection in daemon
---------------------------------

  beads: ambient-fs-f2q
  file:  daemon.rs:328-370
  status: DONE (2026-02-16)

  what was there:
    comment block showing what dup2 calls WOULD do.
    log file was opened for append but never wired up.
    stdout/stderr still went to the terminal.

  what was implemented:
    - dup2(log.as_raw_fd(), 1) for stdout
    - dup2(log.as_raw_fd(), 2) for stderr
    - open /dev/null and dup2 to fd 0 for stdin
    - --foreground / -F flag to skip stdio redirection and forking
    - Daemon::with_foreground() builder method
    - Daemon::is_foreground() getter
    - start() skips fork() and redirect_stdio() when foreground=true

  tests:
    - cli_start_foreground_short/long: flag parsing works
    - test_daemon_with_foreground: builder method works
    - test_daemon_default_background: default is background mode
    - test_daemon_builder_chain: builder chaining works
    - integration tests needed for actual log file contents

  notes:
    - nix crate already had fs feature enabled
    - foreground mode useful for debugging, docker, systemd
    - main.rs does manual PID file management (daemon.start() not called yet)


yf2: watcher stop() for clean shutdown
---------------------------------------

  beads: ambient-fs-yf2
  file:  server.rs:162
  status: DONE (2026-02-16)

  what was there:
    shutdown() sets running=false, shuts down socket server,
    but had a comment: "watcher doesn't have stop() in the
    current API"

  what was implemented:
    FsWatcher::stop(&mut self) that:
    - drops the notify::RecommendedWatcher (sets self.watcher = None)
    - clears the watched paths HashMap
    - is idempotent (safe to call multiple times)
    - works before or after start()

    server.rs:162:
    - calls watcher.lock().await.stop() during shutdown

  tests:
    - stop_clears_watched_paths: verifies watched HashMap is cleared
    - stop_is_idempotent: calls stop() twice, no panic
    - stop_after_start_succeeds: stop() works after start()
    - stop_without_start_is_safe: stop() works on unstarted watcher
    - all 59 watcher tests passing


6n0: unwatch_project implementation
------------------------------------

  beads: ambient-fs-6n0
  file:  server.rs:183-204
  status: DONE (2026-02-16)

  what was there:
    takes project_id, logs it, returns Ok(()). doesn't
    actually unwatch anything.

  what was implemented:
    - uses projects table to map project_id -> path
    - calls get_project_path() to retrieve the path
    - calls remove_project() to delete from store
    - calls watcher.unwatch(path) to stop filesystem events
    - all calls wrapped in spawn_blocking for sql access

  depends on:
    - 0d2 (projects table in store) - complete


0d2: wire store.add_project in watch_project
---------------------------------------------

  beads: ambient-fs-0d2
  file:  server.rs:169-180
  status: DONE (2026-02-16)

  what was there:
    spawn_blocking that logged the project but skipped the
    actual store call. comment: "add_project doesn't exist
    yet"

  what was implemented:
    crates/ambient-fs-store/src/store.rs:
    - added projects table: (project_id TEXT PK, path TEXT,
      created_at TEXT)
    - add_project(id, path) inserts into projects table
    - remove_project(id) deletes from projects table
    - get_project_path(id) -> Option<PathBuf>
    - list_projects() -> Vec<(String, PathBuf)>

    crates/ambient-fsd/src/server.rs:169-180:
    - calls store.add_project(project_id, path) in spawn_blocking
    - also implemented unwatch_project() using get_project_path
      and remove_project

  tests:
    - test_add_project: verifies insert and retrieval
    - test_add_duplicate_project_errors: pk constraint works
    - test_remove_project: delete works
    - test_remove_nonexistent_project_succeeds: idempotent
    - test_get_project_path: lookup works
    - test_list_projects: returns all projects
    - test_list_projects_empty: empty result
    - all 62 store tests passing
