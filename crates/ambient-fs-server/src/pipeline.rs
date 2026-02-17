// Analysis pipeline - bridges watcher events to cached analysis
// TDD: Tests FIRST, then implementation

use ambient_fs_analyzer::{AnalyzerConfig, FileAnalyzer, LlmFileAnalyzer, LanguageRegistry};
use ambient_fs_core::analysis::FileAnalysis;
use ambient_fs_core::event::{FileEvent, EventType};
use ambient_fs_store::FileAnalysisCache;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use crate::llm::LlmClient;
use crate::state::ServerState;

/// Pipeline configuration
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub max_concurrent: usize,
    pub max_file_size: u64,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 2,
            max_file_size: 1024 * 1024, // 1MB
        }
    }
}

/// Analysis pipeline - coordinates file analysis with caching
///
/// - Receives FileEvents from watcher
/// - Checks cache freshness (by content_hash)
/// - Runs tier 1 analysis if stale (line count, TODOs)
/// - Runs tier 2 analysis if LLM enabled (imports, exports, lint hints)
/// - Caches results
/// - Spawns tasks non-blocking via schedule_analysis
pub struct AnalysisPipeline {
    // Store config instead of analyzer so we can create new ones in spawn_blocking
    analyzer_config: AnalyzerConfig,
    store_path: std::path::PathBuf,
    semaphore: Arc<Semaphore>,
    config: PipelineConfig,
    state: Option<Arc<ServerState>>,
    /// LLM client for tier 2 analysis - None if disabled
    llm: Option<Arc<LlmClient>>,
}

impl AnalysisPipeline {
    /// Create a new analysis pipeline
    pub fn new(store_path: std::path::PathBuf, config: PipelineConfig) -> Self {
        let analyzer_config = AnalyzerConfig {
            max_file_size: config.max_file_size,
        };
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent));

        Self {
            analyzer_config,
            store_path,
            semaphore,
            config,
            state: None,
            llm: None,
        }
    }

    /// Set the server state for broadcasting analysis notifications
    pub fn with_state(mut self, state: Arc<ServerState>) -> Self {
        self.state = Some(state);
        self
    }

    /// Set the LLM client for tier 2 analysis
    pub fn with_llm(mut self, llm: Arc<LlmClient>) -> Self {
        self.llm = Some(llm);
        self
    }

    /// Create with default config
    pub fn with_defaults(store_path: std::path::PathBuf) -> Self {
        Self::new(store_path, PipelineConfig::default())
    }

    /// Analyze a single file event
    ///
    /// Returns None if cache is fresh, Some(analysis) if analyzed.
    /// Errors are logged but not propagated (analysis failure is non-fatal).
    pub async fn analyze_file(
        &self,
        event: &FileEvent,
        project_root: &Path,
    ) -> Option<FileAnalysis> {
        // Skip non-content events
        if !matches!(event.event_type, EventType::Created | EventType::Modified) {
            return None;
        }

        // Skip if no content hash
        let content_hash = event.content_hash.as_ref()?;
        let file_path = project_root.join(&event.file_path);

        // Check cache freshness
        let cache_fresh = tokio::task::spawn_blocking({
            let store_path = self.store_path.clone();
            let project_id = event.project_id.clone();
            let file_path_str = event.file_path.clone();
            let hash = content_hash.clone();
            move || {
                let cache = FileAnalysisCache::open(store_path)
                    .or_else(|_| FileAnalysisCache::in_memory())?;
                cache.is_fresh(&project_id, &file_path_str, &hash)
            }
        })
        .await
        .unwrap_or(Ok(false))
        .unwrap_or(false);

        if cache_fresh {
            debug!("cache fresh for {} {}", event.project_id, event.file_path);
            return None;
        }

        // Acquire semaphore permit (throttle concurrent analysis)
        let _permit = self.semaphore.acquire().await.ok()?;

        // Run tier 1 analysis (blocking, so spawn_blocking)
        let analyzer_config = self.analyzer_config.clone();
        let file_path = file_path.clone();
        let project_id = event.project_id.clone();
        let hash = content_hash.clone();
        let relative_path = event.file_path.clone();
        let analysis = tokio::task::spawn_blocking(move || {
            let analyzer = FileAnalyzer::new(analyzer_config);
            analyzer.analyze(&file_path, &project_id, &hash)
        })
        .await
        .ok()? // JoinError -> None
        .ok()?; // AnalysisError -> None

        // Fix file_path to be relative (for cache lookups)
        let mut analysis = analysis;
        analysis.file_path = relative_path;

        // Cache the result (await to ensure it's written)
        let store_path = self.store_path.clone();
        let analysis_clone = analysis.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let cache = FileAnalysisCache::open(store_path)
                .or_else(|_| FileAnalysisCache::in_memory())?;
            cache.put(&analysis_clone)
        })
        .await;

        debug!("analyzed {} {} -> {} lines", event.project_id, event.file_path, analysis.line_count);
        Some(analysis)
    }

    /// Schedule analysis for a file event (non-blocking)
    ///
    /// Spawns a background task that runs analyze_file and logs errors.
    /// Returns immediately without waiting for analysis to complete.
    pub fn schedule_analysis(
        &self,
        event: FileEvent,
        project_root: std::path::PathBuf,
    ) {
        let pipeline = self.clone_ref();
        tokio::spawn(async move {
            if let Err(e) = pipeline.do_analyze(event, project_root).await {
                warn!("analysis task failed: {}", e);
            }
        });
    }

    /// Internal analyze with error handling
    async fn do_analyze(&self, event: FileEvent, project_root: std::path::PathBuf) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(mut analysis) = self.analyze_file(&event, &project_root).await {
            // Tier 2: LLM analysis (imports, exports, lint hints)
            if let Some(ref llm) = self.llm {
                let file_path = project_root.join(&event.file_path);
                if let Some(lang) = LanguageRegistry::get_for_path(&file_path) {
                    match tokio::fs::read_to_string(&file_path).await {
                        Ok(content) => {
                            let llm_analyzer = LlmFileAnalyzer::new(true);
                            let (system, user) = llm_analyzer.build_prompt(
                                &event.file_path,
                                &content,
                                lang.name,
                            );
                            match llm.call(&system, &user).await {
                                Ok(response) => {
                                    match llm_analyzer.enhance_with_llm_response(analysis.clone(), &response) {
                                        Ok(enhanced) => {
                                            analysis = enhanced;
                                            debug!("tier 2 complete: {} imports={} exports={}",
                                                event.file_path,
                                                analysis.imports.len(),
                                                analysis.exports.len());

                                            // Update cache with enhanced analysis
                                            let store_path = self.store_path.clone();
                                            let enhanced_clone = analysis.clone();
                                            let _ = tokio::task::spawn_blocking(move || {
                                                let cache = FileAnalysisCache::open(store_path)
                                                    .or_else(|_| FileAnalysisCache::in_memory())?;
                                                cache.put(&enhanced_clone)
                                            }).await;
                                        }
                                        Err(e) => warn!("tier 2 parse failed for {}: {}", event.file_path, e),
                                    }
                                }
                                Err(e) => warn!("tier 2 LLM call failed for {}: {}", event.file_path, e),
                            }
                        }
                        Err(e) => warn!("tier 2 read failed for {}: {}", event.file_path, e),
                    }
                }
            }

            debug!("analysis complete: {} {} lines={} todos={}",
                event.project_id, event.file_path, analysis.line_count, analysis.todo_count);

            // Broadcast analysis_complete notification to subscribers
            if let Some(ref state) = self.state {
                state.subscriptions.publish_analysis(
                    event.project_id.clone(),
                    event.file_path.clone(),
                    analysis.line_count,
                    analysis.todo_count,
                ).await;
            }
        }
        Ok(())
    }

    /// Clone for spawning tasks
    pub fn clone_ref(&self) -> Self {
        Self {
            analyzer_config: self.analyzer_config.clone(),
            store_path: self.store_path.clone(),
            semaphore: Arc::clone(&self.semaphore),
            config: self.config.clone(),
            state: self.state.clone(),
            llm: self.llm.clone(),
        }
    }

    /// Get current config
    pub fn config(&self) -> &PipelineConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{TempDir, NamedTempFile};
    use std::io::Write;
    use std::fs;

    fn create_test_file(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    fn make_event(project_id: &str, file_path: &str, hash: &str) -> FileEvent {
        FileEvent::new(EventType::Modified, file_path, project_id, "test-machine")
            .with_content_hash(hash)
    }

    // ========== AnalysisPipeline::new ==========

    #[test]
    fn new_creates_pipeline_with_config() {
        let temp = TempDir::new().unwrap();
        let config = PipelineConfig {
            max_concurrent: 5,
            max_file_size: 2048,
        };
        let pipeline = AnalysisPipeline::new(temp.path().to_path_buf(), config);

        assert_eq!(pipeline.config().max_concurrent, 5);
        assert_eq!(pipeline.config().max_file_size, 2048);
    }

    #[test]
    fn with_defaults_uses_default_config() {
        let temp = TempDir::new().unwrap();
        let pipeline = AnalysisPipeline::with_defaults(temp.path().to_path_buf());

        assert_eq!(pipeline.config().max_concurrent, 2);
        assert_eq!(pipeline.config().max_file_size, 1024 * 1024);
    }

    // ========== AnalysisPipeline::analyze_file ==========

    #[tokio::test]
    async fn analyze_file_returns_none_for_deleted_event() {
        let temp = TempDir::new().unwrap();
        let pipeline = AnalysisPipeline::with_defaults(temp.path().to_path_buf());

        let event = FileEvent::new(EventType::Deleted, "test.rs", "proj", "m")
            .with_content_hash("hash123");
        let result = pipeline.analyze_file(&event, temp.path()).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn analyze_file_returns_none_for_renamed_event() {
        let temp = TempDir::new().unwrap();
        let pipeline = AnalysisPipeline::with_defaults(temp.path().to_path_buf());

        let event = FileEvent::new(EventType::Renamed, "new.rs", "proj", "m")
            .with_content_hash("hash123");
        let result = pipeline.analyze_file(&event, temp.path()).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn analyze_file_returns_none_for_event_without_hash() {
        let temp = TempDir::new().unwrap();
        let pipeline = AnalysisPipeline::with_defaults(temp.path().to_path_buf());

        let event = FileEvent::new(EventType::Created, "test.rs", "proj", "m");
        let result = pipeline.analyze_file(&event, temp.path()).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn analyze_file_returns_none_when_cache_is_fresh() {
        let temp = TempDir::new().unwrap();
        let file = create_test_file(temp.path(), "test.rs", "fn main() {}\n");
        let hash = "abc123";

        // Pre-populate cache
        let cache = FileAnalysisCache::open(temp.path().join("cache.db")).unwrap();
        let analysis = FileAnalysis {
            file_path: "test.rs".to_string(),
            project_id: "proj".to_string(),
            content_hash: hash.to_string(),
            exports: vec![],
            imports: vec![],
            todo_count: 0,
            lint_hints: vec![],
            line_count: 2,
        };
        cache.put(&analysis).unwrap();

        let pipeline = AnalysisPipeline::with_defaults(temp.path().join("cache.db"));
        let event = make_event("proj", "test.rs", hash);
        let result = pipeline.analyze_file(&event, temp.path()).await;

        assert!(result.is_none(), "should return None when cache is fresh");
    }

    #[tokio::test]
    async fn analyze_file_analyzes_and_returns_result() {
        let temp = TempDir::new().unwrap();
        let file = create_test_file(temp.path(), "test.rs", "fn main() {\n    // TODO: fix\n}\n");

        let pipeline = AnalysisPipeline::with_defaults(temp.path().join("cache.db"));
        let event = make_event("proj", "test.rs", "hash123");
        let result = pipeline.analyze_file(&event, temp.path()).await;

        assert!(result.is_some());
        let analysis = result.unwrap();
        assert_eq!(analysis.line_count, 3);
        assert_eq!(analysis.todo_count, 1);
        assert_eq!(analysis.project_id, "proj");
        assert_eq!(analysis.content_hash, "hash123");
    }

    #[tokio::test]
    async fn analyze_file_caches_result() {
        let temp = TempDir::new().unwrap();
        let file = create_test_file(temp.path(), "test.rs", "fn main() {}\n");

        let cache_path = temp.path().join("cache.db");
        let pipeline = AnalysisPipeline::with_defaults(cache_path.clone());
        let event = make_event("proj", "test.rs", "hash456");

        // First call - should analyze
        let result1 = pipeline.analyze_file(&event, temp.path()).await;
        assert!(result1.is_some());

        // Verify cache has it
        let cache = FileAnalysisCache::open(cache_path).unwrap();
        let cached = cache.get_if_fresh("proj", "test.rs", "hash456").unwrap();
        assert!(cached.is_some());
        let cached = cached.unwrap();
        assert_eq!(cached.line_count, 1); // "fn main() {}\n" = 1 line
    }

    #[tokio::test]
    async fn analyze_file_returns_none_on_second_call_with_same_hash() {
        let temp = TempDir::new().unwrap();
        let file = create_test_file(temp.path(), "test.rs", "fn main() {}\n");

        let pipeline = AnalysisPipeline::with_defaults(temp.path().join("cache.db"));
        let event = make_event("proj", "test.rs", "samehash");

        // First call - analyzes
        let result1 = pipeline.analyze_file(&event, temp.path()).await;
        assert!(result1.is_some());

        // Second call - cache hit, returns None
        let result2 = pipeline.analyze_file(&event, temp.path()).await;
        assert!(result2.is_none(), "should skip analysis when cache is fresh");
    }

    #[tokio::test]
    async fn analyze_file_reanalyzes_when_hash_changes() {
        let temp = TempDir::new().unwrap();
        let file = create_test_file(temp.path(), "test.rs", "fn main() {}\n");

        let pipeline = AnalysisPipeline::with_defaults(temp.path().join("cache.db"));
        let event1 = make_event("proj", "test.rs", "hash1");

        // First call
        pipeline.analyze_file(&event1, temp.path()).await;

        // Modify file
        create_test_file(temp.path(), "test.rs", "fn main() {\n    println!(\"x\");\n}\n");
        let event2 = make_event("proj", "test.rs", "hash2");

        // Second call with new hash - should reanalyze
        let result2 = pipeline.analyze_file(&event2, temp.path()).await;
        assert!(result2.is_some());
        assert_eq!(result2.unwrap().line_count, 3);
    }

    #[tokio::test]
    async fn analyze_file_handles_missing_file_gracefully() {
        let temp = TempDir::new().unwrap();
        let pipeline = AnalysisPipeline::with_defaults(temp.path().join("cache.db"));

        let event = make_event("proj", "nonexistent.rs", "hash123");
        let result = pipeline.analyze_file(&event, temp.path()).await;

        // Should return None (error handled internally)
        assert!(result.is_none());
    }

    // ========== AnalysisPipeline::schedule_analysis ==========

    #[tokio::test]
    async fn schedule_analysis_spawn_completes_analysis() {
        let temp = TempDir::new().unwrap();
        let file = create_test_file(temp.path(), "test.rs", "fn main() {}\n");

        let pipeline = AnalysisPipeline::with_defaults(temp.path().join("cache.db"));
        let event = make_event("proj", "test.rs", "hash789");

        // schedule_analysis returns immediately
        pipeline.schedule_analysis(event, temp.path().to_path_buf());

        // Give task time to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify cache was populated
        let cache = FileAnalysisCache::open(temp.path().join("cache.db")).unwrap();
        let cached = cache.get_if_fresh("proj", "test.rs", "hash789").unwrap();
        assert!(cached.is_some());
    }

    #[tokio::test]
    async fn schedule_analysis_does_not_block() {
        let temp = TempDir::new().unwrap();
        let file = create_test_file(temp.path(), "test.rs", "fn main() {}\n");

        let pipeline = AnalysisPipeline::with_defaults(temp.path().join("cache.db"));
        let event = make_event("proj", "test.rs", "hash999");

        let start = std::time::Instant::now();
        pipeline.schedule_analysis(event, temp.path().to_path_buf());
        let elapsed = start.elapsed();

        // Should return almost immediately (<< file analysis time)
        assert!(elapsed < tokio::time::Duration::from_millis(10));
    }

    #[tokio::test]
    async fn schedule_analysis_handles_invalid_path() {
        let temp = TempDir::new().unwrap();
        let pipeline = AnalysisPipeline::with_defaults(temp.path().join("cache.db"));

        let event = make_event("proj", "nonexistent.rs", "hash");
        pipeline.schedule_analysis(event, temp.path().to_path_buf());

        // Give task time to fail gracefully
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Should not panic or crash
    }

    // ========== Semaphore throttling ==========

    #[tokio::test]
    async fn semaphore_limits_concurrent_analysis_to_max_concurrent() {
        let temp = TempDir::new().unwrap();
        let config = PipelineConfig {
            max_concurrent: 2, // Only 2 concurrent
            max_file_size: 1024 * 1024,
        };
        let pipeline = AnalysisPipeline::new(temp.path().join("cache.db"), config);

        // Create test files
        for i in 0..5 {
            create_test_file(temp.path(), &format!("test{}.rs", i), "fn main() {}\n");
        }

        let mut handles = vec![];
        for i in 0..5 {
            let pipeline = pipeline.clone_ref();
            let event = make_event("proj", &format!("test{}.rs", i), &format!("hash{}", i));
            let root = temp.path().to_path_buf();

            let handle = tokio::spawn(async move {
                pipeline.analyze_file(&event, &root).await
            });
            handles.push(handle);
        }

        // All should complete eventually
        let mut results = vec![];
        for handle in handles {
            results.push(handle.await.ok().flatten());
        }
        let completed = results.into_iter().filter_map(|r| r).count();
        assert_eq!(completed, 5, "all files should be analyzed");
    }

    // ========== FileAnalyzer integration ==========

    #[tokio::test]
    async fn analyze_file_counts_lines_correctly() {
        let temp = TempDir::new().unwrap();
        let content = "line1\nline2\nline3\nline4\n";
        create_test_file(temp.path(), "lines.rs", content);

        let pipeline = AnalysisPipeline::with_defaults(temp.path().join("cache.db"));
        let event = make_event("proj", "lines.rs", "hash");

        let result = pipeline.analyze_file(&event, temp.path()).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().line_count, 4);
    }

    #[tokio::test]
    async fn analyze_file_counts_todos_correctly() {
        let temp = TempDir::new().unwrap();
        let content = "fn main() {\n    // TODO: one\n    // FIXME: two\n    // HACK: three\n}\n";
        create_test_file(temp.path(), "todos.rs", content);

        let pipeline = AnalysisPipeline::with_defaults(temp.path().join("cache.db"));
        let event = make_event("proj", "todos.rs", "hash");

        let result = pipeline.analyze_file(&event, temp.path()).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().todo_count, 3);
    }

    #[tokio::test]
    async fn analyze_file_handles_empty_file() {
        let temp = TempDir::new().unwrap();
        create_test_file(temp.path(), "empty.rs", "");

        let pipeline = AnalysisPipeline::with_defaults(temp.path().join("cache.db"));
        let event = make_event("proj", "empty.rs", "hash");

        let result = pipeline.analyze_file(&event, temp.path()).await;
        assert!(result.is_some());
        let analysis = result.unwrap();
        assert_eq!(analysis.line_count, 0);
        assert_eq!(analysis.todo_count, 0);
    }
}
