event pruning schedule
=======================

status: design
created: 2026-02-16
affects: crates/ambient-fsd/src/server.rs


overview
--------

prune.rs has EventPruner with prune_events_before(),
prune_analysis_before(), vacuum(). PruneConfig has
retention_days (default 90) and cutoff_timestamp().

nothing in the daemon calls these. the DB grows forever.


implementation
--------------

simple periodic task in DaemonServer::run().

  PruneScheduler:
    - config: PruneConfig
    - store_path: PathBuf
    - interval: Duration (default: 24 hours)

  async fn run(&self):
    loop {
      tokio::time::sleep(self.interval).await;
      self.prune().await;
    }

  async fn prune(&self):
    let store_path = self.store_path.clone();
    let cutoff = self.config.cutoff_timestamp();

    // rusqlite is sync, use spawn_blocking
    let result = tokio::task::spawn_blocking(move || {
      let conn = rusqlite::Connection::open(&store_path)?;
      let events_pruned = EventPruner::prune_events_before(&conn, cutoff)?;
      let analysis_pruned = EventPruner::prune_analysis_before(&conn, cutoff)?;

      if events_pruned > 0 || analysis_pruned > 0 {
        EventPruner::vacuum(&conn)?;
      }

      Ok::<_, Box<dyn std::error::Error + Send + Sync>>((events_pruned, analysis_pruned))
    }).await??;

    info!(
      "prune complete: {} events, {} analysis records removed",
      result.0, result.1
    );


daemon integration
-------------------

  DaemonServer::run():
    let prune_scheduler = PruneScheduler::new(
      PruneConfig::new(config.retention_days),
      config.db_path.clone(),
    );
    tokio::spawn(prune_scheduler.run());

  also run once at startup (catch up if daemon was down):
    prune_scheduler.prune().await;


config
------

  from config.toml [store] section:
    retention_days = 90

  from DaemonConfig:
    - add prune_interval_hours: u64 (default 24)

  or just hardcode 24h. it's not something users need
  to configure. once a day is fine.


manual prune CLI
-----------------

  ambient-fsd prune
    immediately runs prune + vacuum.
    useful for maintenance.

  main.rs:
    Command::Prune -> cmd_prune():
      let config = PruneConfig::new(retention_days);
      let conn = Connection::open(db_path)?;
      let events = EventPruner::prune_events_before(&conn, config.cutoff_timestamp())?;
      let analysis = EventPruner::prune_analysis_before(&conn, config.cutoff_timestamp())?;
      EventPruner::vacuum(&conn)?;
      println!("pruned {} events, {} analysis", events, analysis);


test strategy
-------------

unit tests:
  - PruneScheduler::prune() deletes old records
  - PruneScheduler::prune() skips vacuum when nothing deleted
  - PruneScheduler interval defaults to 24h

integration tests:
  - start daemon, insert old events, wait for prune cycle,
    verify old events gone
  - cmd_prune works from CLI


depends on
----------

  - EventPruner (done, fully implemented with tests)
  - PruneConfig (done)
  - DaemonConfig (in progress)
