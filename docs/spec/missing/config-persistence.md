config persistence
==================

status: design
created: 2026-02-16
affects: crates/ambient-fsd/src/config.rs (new),
         crates/ambient-fsd/src/server.rs,
         crates/ambient-fsd/src/main.rs


overview
--------

the daemon needs a config.toml file for persistent settings.
currently ServerConfig is hardcoded with defaults. two specific
gaps: machine_id isn't persisted (lsd), and watched projects
don't survive restarts (107).


lsd: persist machine_id to config
-----------------------------------

  beads: ambient-fs-lsd
  file:  server.rs, config.rs (new)

  problem:
    machine_id = hostname from env var. not stable across
    containers. no uuid generation on first run. not saved.

  implementation:
    crates/ambient-fsd/src/config.rs (new file):

    DaemonConfig struct:
      machine_id: String
      db_path: PathBuf         (default: ~/.local/share/ambient-fs/events.db)
      socket_path: PathBuf     (default: /tmp/ambient-fs.sock)
      debounce_ms: u64         (default: 100)
      max_file_size_bytes: u64 (default: 10MB)
      log_level: String        (default: "info")

    config file location:
      ~/.config/ambient-fs/config.toml
      or $AMBIENT_FS_CONFIG env var override

    load() -> DaemonConfig:
      1. check if config file exists
      2. if exists: parse toml, return
      3. if not exists:
         a. generate machine_id = uuid::Uuid::new_v4().to_string()
         b. create default config
         c. write to config file (create dirs)
         d. return

    save(&self) -> Result<()>:
      serialize to toml, write to config file

    config.toml format:
      machine_id = "a1b2c3d4-e5f6-..."
      db_path = "~/.local/share/ambient-fs/events.db"
      socket_path = "/tmp/ambient-fs.sock"
      debounce_ms = 100
      max_file_size_bytes = 10485760
      log_level = "info"

  server.rs changes:
    - ServerConfig::from_daemon_config(dc: &DaemonConfig)
    - or replace ServerConfig with DaemonConfig entirely

  main.rs changes:
    - load DaemonConfig at startup
    - pass to DaemonServer::new()


107: persist watch/unwatch to projects table
----------------------------------------------

  beads: ambient-fs-107
  file:  server.rs

  problem:
    watches don't survive daemon restart. watch_project()
    already calls store.add_project() (agents wired this),
    but on startup, the daemon doesn't reload projects.

  implementation:
    server.rs DaemonServer::new() or run():
    - after creating store, call store.list_projects()
    - for each (project_id, path):
      if path still exists:
        watcher.watch(path)
        state.add_project(project_id, path)
      else:
        log warning, remove stale project from store

    server.rs DaemonServer::run() addition:
    - add restore_projects() method called early in run():
      let store = EventStore::new(store_path)?;
      let projects = store.list_projects()?;
      for (id, path) in projects {
          if path.exists() {
              let mut w = watcher.lock().await;
              w.watch(path.clone())?;
              state.add_project(id.clone(), path).await;
              info!("restored watch: {} -> {}", id, path.display());
          } else {
              warn!("stale project removed: {} -> {}", id, path.display());
              store.remove_project(&id)?;
          }
      }

  this is a simple iteration on startup. the store already
  has the projects table and CRUD methods (agent implemented).

  test strategy:
    - unit: create store, add projects, create new server,
      verify projects restored on run()
    - unit: stale project (path deleted) removed on startup
    - integration: start daemon, watch project, restart daemon,
      verify project still watched
