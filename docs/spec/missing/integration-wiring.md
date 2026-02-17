integration wiring (cross-crate connections)
=============================================

status: design
created: 2026-02-16
affects: crates/ambient-fs-watcher/src/watcher.rs,
         crates/ambient-fs-store/src/cache.rs,
         crates/ambient-fs-store/src/migrations.rs


overview
--------

several crates have fully implemented components that are
never wired into the actual event flow. each component works
in isolation (has passing tests) but nothing connects them.


6sx: integrate ContentDedup into watcher
-----------------------------------------

  beads: ambient-fs-6sx
  files: watcher.rs, dedup.rs

  problem:
    watcher.rs emits FileEvent with content_hash = None.
    dedup.rs has ContentDedup::hash_file() that produces
    blake3 hashes. they're never connected.

  implementation:
    watcher.rs changes:
    - add field: content_dedup: Option<ContentDedup>
    - add builder: with_content_dedup(dedup: ContentDedup)
    - in create_watcher() callback, after building FileEvent:
      if content_dedup is Some AND event is Created/Modified:
        let hash = dedup.hash_file(&path)
        if Ok(hash): event = event.with_content_hash(hash)
    - skip hash for Deleted events (file gone)
    - skip if hash_file returns Err (file too large, permission)

  the callback runs in notify's thread, so hash_file needs
  to be sync (it already is -- std::fs::read + blake3::hash).

  ContentDedup needs to be cloneable or Arc'd since the
  callback closure needs ownership. currently it is:
    pub struct ContentDedup { max_size_bytes: u64 }
  this is Copy, so clone into the closure is fine.

  test strategy:
    - unit test: create watcher with dedup, write file to
      temp dir, verify emitted event has content_hash set
    - unit test: large file (over max_size) emits None hash
    - unit test: deleted file emits None hash
    - unit test: watcher without dedup still works (backward compat)


9bc: fix cache validation with content_hash
---------------------------------------------

  beads: ambient-fs-9bc
  files: cache.rs, analysis.rs

  problem:
    cache.get() returns analysis without checking content_hash.
    FileAnalysis has a content_hash field but cache.get()
    doesn't verify it matches current file. stale analysis
    returned for modified files.

  implementation:
    cache.rs changes:
    - add get_if_fresh(project_id, file_path, current_hash) method:
      SELECT ... WHERE project_id = ? AND file_path = ?
        AND content_hash = ?
      returns None if hash doesn't match (stale)
    - keep existing get() for backward compat (no hash check)
    - add is_fresh(project_id, file_path, hash) -> bool
      quick check without full deserialization

  analysis.rs already has is_valid_for() but it's unused.
  that stays in core (no IO). the new cache method does
  the check at the DB level which is more efficient.

  test strategy:
    - test: put analysis with hash "abc", get_if_fresh with
      "abc" returns it
    - test: put analysis with hash "abc", get_if_fresh with
      "xyz" returns None
    - test: is_fresh returns true/false correctly
    - test: existing get() still works without hash check


r10: add idx_analysis_hash to migrations
------------------------------------------

  beads: ambient-fs-r10
  files: migrations.rs

  problem:
    cache.rs creates idx_analysis_hash in its own init_schema().
    migrations.rs create_tables_v1() creates the file_analysis
    table but NOT this index. fresh DB from migrations lacks
    the hash lookup index.

  implementation:
    migrations.rs create_tables_v1():
    - add after file_analysis table creation:
      CREATE INDEX IF NOT EXISTS idx_analysis_hash
        ON file_analysis(project_id, content_hash)

  this is a one-liner fix. the cache.rs init_schema is a
  separate code path (used when cache opens its own DB).
  but the canonical path through migrations should also
  create the index.

  test strategy:
    - test: ensure_schema creates idx_analysis_hash index
    - update existing test_create_tables_creates_indexes
      to check for idx_analysis_hash


watcher attribution integration
---------------------------------

  not a beads issue but closely related to 6sx.

  problem:
    EventAttributor exists with attribute_source() method.
    watcher.rs always sets source = Source::User (implicit
    from FileEvent::new default). attribution never runs.

  implementation:
    watcher.rs changes:
    - add field: attributor: Option<EventAttributor>
    - add builder: with_attributor(attr: EventAttributor)
    - in create_watcher() callback, after building FileEvent:
      if attributor is Some:
        let source = attributor.attribute_source(&path)
        event = event.with_source(source)

  EventAttributor checks .git/index mtime and build patterns.
  it's sync and cheap. clone into callback closure.

  this means events automatically get source = Git when
  .git/index was recently modified, source = Build for
  build artifacts, and source = User for everything else.

  test strategy:
    - test: watcher with attributor sets correct source
    - test: watcher without attributor defaults to User
    - integration: modify file while git index is recent,
      verify source = Git on event
