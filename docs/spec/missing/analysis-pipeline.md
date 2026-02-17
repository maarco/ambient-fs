analysis pipeline (P2-3)
=========================

status: design
created: 2026-02-16
affects: crates/ambient-fsd/src/server.rs,
         crates/ambient-fs-server/src/pipeline.rs (new),
         crates/ambient-fs-server/src/state.rs


overview
--------

the analyzer exists (FileAnalyzer with tier 1 + LLM tier 2).
the cache exists (FileAnalysisCache with get/put/invalidate).
the watcher emits events. nothing connects them.

when a file is created or modified, the daemon should:
  1. check if analysis cache is fresh (content_hash match)
  2. if stale: run tier 1 analysis (sync, fast)
  3. cache the tier 1 result
  4. if LLM enabled: schedule tier 2 enhancement (async)
  5. when tier 2 completes: update cache, broadcast

this is the most critical missing piece. without it,
query_awareness always returns zero for todo_count,
lint_hints, and line_count.


architecture
------------

  AnalysisPipeline struct:
    - analyzer: FileAnalyzer
    - llm_client: Option<LlmClient>   (from ambient-fs-server)
    - store_path: PathBuf              (for cache access)
    - max_concurrent: usize            (from config, default 2)
    - semaphore: Arc<Semaphore>        (throttle concurrent analysis)
    - batch_tx: mpsc::Sender<BatchItem> (for LLM batching)

  lives in: crates/ambient-fs-server/src/pipeline.rs


event-driven trigger
---------------------

  in DaemonServer::run(), after watcher.start():

  the watcher event loop already exists. currently it:
    1. receives FileEvent from watcher rx
    2. inserts into store
    3. broadcasts to subscribers

  add step 2.5:
    if event.event_type is Created or Modified:
      pipeline.schedule_analysis(event.clone())

  schedule_analysis is non-blocking. it spawns a task:
    tokio::spawn(async move {
      pipeline.analyze_file(event).await;
    })

  the semaphore limits concurrent tasks to max_concurrent.


analysis flow
--------------

  async fn analyze_file(&self, event: FileEvent):

  1. acquire semaphore permit (blocks if at capacity)
  2. resolve full file path from project root + event.file_path
  3. check cache freshness:
     let cache = FileAnalysisCache::new(store_path)?;
     if cache.is_fresh(project_id, file_path, content_hash):
       return (already analyzed, skip)
  4. run tier 1 analysis (spawn_blocking, FileAnalyzer is sync):
     let analysis = spawn_blocking(move ||
       analyzer.analyze(&full_path, project_id, content_hash)
     ).await??;
  5. cache tier 1 result:
     cache.put(&analysis)?;
  6. broadcast analysis-complete notification
  7. if LLM enabled:
     a. detect language from file extension
     b. build LLM prompt (language + file content)
     c. send to batch_tx for batching
     d. batch processor collects items for batch_delay_ms
     e. calls LlmClient::call_json() per item (or batched)
     f. on response: analyzer.enhance_with_llm_response()
     g. update cache with enhanced analysis
     h. broadcast analysis-complete again (with tier 2 data)


LLM batching
-------------

  the LlmAnalyzer spec describes batch_delay_ms (default 2000ms).
  collect files that changed within a window, then send
  individual LLM calls (haiku is fast enough per-file).

  BatchProcessor:
    - receives BatchItem from channel
    - waits batch_delay_ms for more items
    - processes each item individually (not one mega-call)
    - the delay just deduplicates rapid changes to same file

  BatchItem:
    - file_path: String
    - project_id: String
    - content: String
    - language: String
    - content_hash: String
    - base_analysis: FileAnalysis (tier 1 result)


state integration
------------------

  ServerState additions:
    - pipeline: Option<AnalysisPipeline>

  or keep pipeline separate in DaemonServer, not in ServerState.
  the pipeline is a background processor, not queryable state.
  ServerState stays focused on shared query state.

  DaemonServer::run():
    let pipeline = AnalysisPipeline::new(config);
    // in event loop:
    pipeline.schedule_analysis(event);


config
------

  from config.toml [analyzer] section:
    enabled: bool (default true)
    max_concurrent: usize (default 2)
    max_file_size_bytes: u64 (default 1MB)
    languages: Vec<String>

  from config.toml [llm] section:
    enabled: bool (default false)
    api_key, model, batch_delay_ms, etc.


notifications
--------------

  when analysis completes (tier 1 or tier 2), broadcast:

  {
    "jsonrpc": "2.0",
    "method": "analysis_complete",
    "params": {
      "project_id": "my-project",
      "file_path": "src/auth.rs",
      "content_hash": "abc123",
      "line_count": 142,
      "todo_count": 3,
      "lint_hints": 0,
      "tier": 1
    }
  }

  tier 2 sends same notification with populated imports/exports/lint
  and tier: 2. clients can distinguish and update incrementally.

  uses existing SubscriptionManager. add a new broadcast method
  or reuse existing with a different event type marker.


test strategy
-------------

unit tests:
  - schedule_analysis spawns task
  - semaphore limits concurrent analysis to max_concurrent
  - cache check skips already-fresh files
  - tier 1 analysis produces correct FileAnalysis
  - tier 1 result gets cached
  - LLM disabled: only tier 1 runs
  - LLM enabled: tier 2 enhancement runs after tier 1
  - batch dedup: rapid changes to same file = one analysis
  - file too large: skipped gracefully
  - analysis error: logged, not propagated

integration tests:
  - create file in watched dir -> analysis cached
  - modify file -> analysis re-run with new hash
  - query_awareness returns non-zero counts after analysis


depends on
----------

  - FileAnalyzer (done)
  - LlmFileAnalyzer (done)
  - FileAnalysisCache (done, freshness check in progress)
  - LlmClient (done)
  - SubscriptionManager (done)
  - ContentDedup in watcher (in progress, provides content_hash)
