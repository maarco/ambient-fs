use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ambient_fs_core::{event::FileEvent, event::EventType, filter::PathFilter};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use crate::{ContentDedup, EventAttributor};

pub type EventReceiver = mpsc::UnboundedReceiver<FileEvent>;

/// Errors from the filesystem watcher
#[derive(Debug, Error)]
pub enum WatchError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),

    #[error("path is not absolute: {0}")]
    NotAbsolute(PathBuf),

    #[error("path does not exist: {0}")]
    NotExists(PathBuf),

    #[error("already watching: {0}")]
    AlreadyWatching(PathBuf),

    #[error("not watching: {0}")]
    NotWatching(PathBuf),
}

/// Tracked watch on a path
#[derive(Debug, Clone)]
struct WatchedPath {
    #[allow(dead_code)]
    path: PathBuf,
    #[allow(dead_code)]
    recursive: bool,
}

/// Filesystem watcher with debouncing and path filtering
///
/// Wraps notify 8 to watch filesystem changes, debounces rapid events
/// into single FileEvents, and applies PathFilter to ignore patterns.
pub struct FsWatcher {
    #[allow(dead_code)]
    debounce_ms: u64,
    tx: mpsc::UnboundedSender<FileEvent>,
    watcher: Option<RecommendedWatcher>,
    watched: HashMap<PathBuf, WatchedPath>,
    path_filter: PathFilter,
    project_id: String,
    machine_id: String,
    content_dedup: Option<ContentDedup>,
    attributor: Option<Arc<Mutex<EventAttributor>>>,
    project_root: Option<PathBuf>,
}

impl FsWatcher {
    /// Create a new FsWatcher with the given debounce duration
    ///
    /// # Arguments
    /// * `debounce_ms` - Milliseconds to wait before emitting events for a path
    /// * `project_id` - Project identifier for emitted events
    /// * `machine_id` - Machine identifier for emitted events
    pub fn new(debounce_ms: u64, project_id: impl Into<String>, machine_id: impl Into<String>) -> Self {
        let (tx, _rx) = mpsc::unbounded_channel();
        let project_id = project_id.into();
        let machine_id = machine_id.into();

        Self {
            debounce_ms,
            tx,
            watcher: None,
            watched: HashMap::new(),
            path_filter: PathFilter::default(),
            project_id,
            machine_id,
            content_dedup: None,
            attributor: None,
            project_root: None,
        }
    }

    /// Set a custom PathFilter for ignoring paths
    pub fn with_path_filter(mut self, filter: PathFilter) -> Self {
        self.path_filter = filter;
        self
    }

    /// Replace the current PathFilter
    pub fn set_path_filter(&mut self, filter: PathFilter) {
        self.path_filter = filter;
    }

    /// Set the content deduplicator for hashing file contents
    pub fn with_content_dedup(mut self, dedup: ContentDedup) -> Self {
        self.content_dedup = Some(dedup);
        self
    }

    /// Set the event attributor for detecting event sources
    ///
    /// The project_root is used for git detection (checking .git/index mtime).
    pub fn with_attributor(mut self, attributor: EventAttributor, project_root: PathBuf) -> Self {
        self.attributor = Some(Arc::new(Mutex::new(attributor)));
        self.project_root = Some(project_root);
        self
    }

    /// Start watching and return the event receiver
    ///
    /// Must be called before watch/unwatch. The receiver should be polled
    /// in a task to process incoming FileEvents.
    pub fn start(&mut self) -> Result<EventReceiver, WatchError> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.tx = tx;

        let watcher = self.create_watcher()?;
        self.watcher = Some(watcher);

        Ok(rx)
    }

    /// Watch a path for filesystem events
    ///
    /// # Arguments
    /// * `path` - Absolute path to watch (file or directory)
    ///
    /// # Errors
    /// * `NotAbsolute` - Path is not absolute
    /// * `NotExists` - Path does not exist
    /// * `AlreadyWatching` - Already watching this path
    pub fn watch(&mut self, path: PathBuf) -> Result<(), WatchError> {
        if !path.is_absolute() {
            return Err(WatchError::NotAbsolute(path));
        }
        if !path.exists() {
            return Err(WatchError::NotExists(path));
        }
        if self.watched.contains_key(&path) {
            return Err(WatchError::AlreadyWatching(path));
        }

        let is_dir = path.is_dir();
        let recursive = is_dir;

        if let Some(watcher) = &mut self.watcher {
            let mode = if recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            watcher.watch(&path, mode)?;
        }

        self.watched.insert(
            path.clone(),
            WatchedPath {
                path: path.clone(),
                recursive,
            },
        );

        debug!(path = %path.display(), recursive, "added watch");
        Ok(())
    }

    /// Stop watching a path
    ///
    /// # Errors
    /// * `NotWatching` - Path is not being watched
    pub fn unwatch(&mut self, path: PathBuf) -> Result<(), WatchError> {
        if !self.watched.contains_key(&path) {
            return Err(WatchError::NotWatching(path));
        }

        if let Some(watcher) = &mut self.watcher {
            watcher.unwatch(&path)?;
        }

        self.watched.remove(&path);
        debug!(path = %path.display(), "removed watch");
        Ok(())
    }

    /// Get list of currently watched paths
    pub fn watched_paths(&self) -> Vec<PathBuf> {
        self.watched.keys().cloned().collect()
    }

    /// Stop the watcher and release all resources
    ///
    /// Drops the internal notify watcher, closes event callbacks,
    /// and clears all watched paths. Idempotent - calling multiple
    /// times is safe.
    pub fn stop(&mut self) {
        self.watcher = None;
        self.watched.clear();
    }

    fn create_watcher(&self) -> Result<RecommendedWatcher, WatchError> {
        let tx = self.tx.clone();
        let path_filter = self.path_filter.clone();
        let project_id = self.project_id.clone();
        let machine_id = self.machine_id.clone();
        let content_dedup = self.content_dedup; // ContentDedup is Copy, can move

        // Clone Arc/Mutex for shared state in callback
        let attributor = self.attributor.clone();
        let project_root = self.project_root.clone();

        let handle_events = move |res: Result<notify::Event, notify::Error>| {
            let event = match res {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "watcher error");
                    return;
                }
            };

            for path in event.paths {
                let path_str = path.to_string_lossy().to_string();

                if path_filter.should_ignore(&path_str) {
                    trace!(path = %path_str, "ignored by filter");
                    continue;
                }

                let event_type = match event.kind {
                    EventKind::Create(_) => EventType::Created,
                    EventKind::Modify(_) => EventType::Modified,
                    EventKind::Remove(_) => EventType::Deleted,
                    _ => continue,
                };

                let mut file_event = FileEvent::new(
                    event_type,
                    path_str.clone(),
                    project_id.clone(),
                    machine_id.clone(),
                );

                // Apply source attribution if configured
                if let (Some(ref attr_mutex), Some(ref proj_root)) = (&attributor, &project_root) {
                    if let Ok(mut attr) = attr_mutex.lock() {
                        let source = attr.detect_source(&path, proj_root);
                        file_event = file_event.with_source(source);
                    }
                }

                // Apply content hash if configured and event is not Delete
                if let Some(dedup) = content_dedup {
                    if matches!(event_type, EventType::Created | EventType::Modified) {
                        if let Ok(hash) = dedup.hash_file(&path) {
                            file_event = file_event.with_content_hash(hash);
                        }
                        // If hash fails (too large, permissions), we just emit without hash
                    }
                    // Deleted events get no hash (file is gone)
                }

                let _ = tx.send(file_event);
            }
        };

        Ok(notify::recommended_watcher(handle_events)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use std::time::Duration;
    use tempfile::TempDir;
    use ambient_fs_core::event::Source;

    #[test]
    fn new_watcher_has_debounce() {
        let watcher = FsWatcher::new(100, "proj", "machine");
        assert_eq!(watcher.debounce_ms, 100);
    }

    #[test]
    fn new_watcher_has_default_filter() {
        let watcher = FsWatcher::new(100, "proj", "machine");
        assert!(watcher.path_filter.should_ignore("node_modules/foo.js"));
        assert!(watcher.path_filter.should_ignore(".git/config"));
    }

    #[test]
    fn new_watcher_is_empty() {
        let watcher = FsWatcher::new(100, "proj", "machine");
        assert!(watcher.watched_paths().is_empty());
    }

    #[test]
    fn with_path_filter_sets_filter() {
        let filter = PathFilter::new(vec!["build".to_string()], 1024);
        let watcher = FsWatcher::new(100, "proj", "machine").with_path_filter(filter);
        assert!(watcher.path_filter.should_ignore("build/out.js"));
    }

    #[test]
    fn set_path_filter_replaces_filter() {
        let mut watcher = FsWatcher::new(100, "proj", "machine");
        let filter = PathFilter::new(vec!["custom".to_string()], 512);
        watcher.set_path_filter(filter);
        assert!(watcher.path_filter.should_ignore("custom/file.txt"));
    }

    #[test]
    fn watch_rejects_relative_path() {
        let mut watcher = FsWatcher::new(100, "proj", "machine");
        watcher.start().unwrap();
        let result = watcher.watch(PathBuf::from("relative/path"));
        assert!(matches!(result, Err(WatchError::NotAbsolute(_))));
    }

    #[test]
    fn watch_rejects_nonexistent_path() {
        let mut watcher = FsWatcher::new(100, "proj", "machine");
        watcher.start().unwrap();
        let result = watcher.watch(PathBuf::from("/does/not/exist"));
        assert!(matches!(result, Err(WatchError::NotExists(_))));
    }

    #[test]
    fn watch_same_path_twice_errors() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher = FsWatcher::new(100, "proj", "machine");
        watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();
        let result = watcher.watch(temp_dir.path().to_path_buf());
        assert!(matches!(result, Err(WatchError::AlreadyWatching(_))));
    }

    #[test]
    fn unwatch_non_watched_path_errors() {
        let mut watcher = FsWatcher::new(100, "proj", "machine");
        watcher.start().unwrap();
        let result = watcher.unwatch(PathBuf::from("/not/watched"));
        assert!(matches!(result, Err(WatchError::NotWatching(_))));
    }

    #[test]
    fn watched_paths_returns_all_watched() {
        let temp1 = TempDir::new().unwrap();
        let temp2 = TempDir::new().unwrap();
        let mut watcher = FsWatcher::new(100, "proj", "machine");
        watcher.start().unwrap();

        watcher.watch(temp1.path().to_path_buf()).unwrap();
        watcher.watch(temp2.path().to_path_buf()).unwrap();

        let paths = watcher.watched_paths();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn path_filter_ignores_node_modules() {
        let filter = PathFilter::default();
        assert!(filter.should_ignore("src/node_modules/pkg/index.js"));
    }

    #[test]
    fn path_filter_ignores_git() {
        let filter = PathFilter::default();
        assert!(filter.should_ignore(".git/objects/abc123"));
    }

    #[test]
    fn path_filter_allows_normal_files() {
        let filter = PathFilter::default();
        assert!(!filter.should_ignore("src/main.rs"));
        assert!(!filter.should_ignore("README.md"));
    }

    #[test]
    fn watch_absolute_existing_path() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher = FsWatcher::new(100, "proj", "machine");
        watcher.start().unwrap();
        let result = watcher.watch(temp_dir.path().to_path_buf());
        assert!(result.is_ok());
        assert_eq!(watcher.watched_paths().len(), 1);
    }

    #[test]
    fn unwatch_removes_watched_path() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher = FsWatcher::new(100, "proj", "machine");
        watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();
        let result = watcher.unwatch(temp_dir.path().to_path_buf());
        assert!(result.is_ok());
        assert!(watcher.watched_paths().is_empty());
    }

    #[test]
    fn stop_clears_watched_paths() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher = FsWatcher::new(100, "proj", "machine");
        watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();
        assert_eq!(watcher.watched_paths().len(), 1);

        watcher.stop();
        assert!(watcher.watched_paths().is_empty());
    }

    #[test]
    fn stop_is_idempotent() {
        let mut watcher = FsWatcher::new(100, "proj", "machine");
        watcher.start().unwrap();

        watcher.stop();
        watcher.stop(); // should not panic
    }

    #[test]
    fn stop_after_start_succeeds() {
        let mut watcher = FsWatcher::new(100, "proj", "machine");
        watcher.start().unwrap();

        watcher.stop(); // should not panic
        assert!(watcher.watched_paths().is_empty());
    }

    #[test]
    fn stop_without_start_is_safe() {
        let mut watcher = FsWatcher::new(100, "proj", "machine");

        watcher.stop(); // should not panic, watcher is None
        assert!(watcher.watched_paths().is_empty());
    }

    // ContentDedup integration tests

    #[test]
    fn with_content_dedup_stores_dedup() {
        let dedup = ContentDedup::new(1024);
        let watcher = FsWatcher::new(100, "proj", "machine").with_content_dedup(dedup);
        assert!(watcher.content_dedup.is_some());
    }

    #[test]
    fn watcher_without_dedup_has_none() {
        let watcher = FsWatcher::new(100, "proj", "machine");
        assert!(watcher.content_dedup.is_none());
    }

    #[test]
    fn watcher_with_dedup_emits_hash_on_create() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher = FsWatcher::new(10, "proj", "machine")
            .with_content_dedup(ContentDedup::new(1024));
        let mut rx = watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();

        // Create a file
        let file_path = temp_dir.path().join("test.txt");
        File::create(&file_path).unwrap().write_all(b"hello world").unwrap();

        // Wait for event
        std::thread::sleep(Duration::from_millis(50));

        let event = rx.try_recv().unwrap();
        assert_eq!(event.event_type, EventType::Created);
        assert!(event.content_hash.is_some());
        assert_eq!(event.content_hash.unwrap().len(), 64); // blake3 hex
    }

    #[test]
    fn watcher_with_dedup_emits_hash_on_modify() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        File::create(&file_path).unwrap().write_all(b"initial").unwrap();

        let mut watcher = FsWatcher::new(10, "proj", "machine")
            .with_content_dedup(ContentDedup::new(1024));
        let mut rx = watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();

        // Drain initial Create event
        std::thread::sleep(Duration::from_millis(50));
        let _ = rx.try_recv();

        // Modify the file
        fs::write(&file_path, b"modified content").unwrap();

        std::thread::sleep(Duration::from_millis(50));

        // May get multiple events, find the Modify one
        let mut found_modify = false;
        for _ in 0..10 {
            if let Ok(event) = rx.try_recv() {
                if event.event_type == EventType::Modified {
                    assert!(event.content_hash.is_some());
                    found_modify = true;
                    break;
                }
            } else {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        assert!(found_modify, "No Modify event found");
    }

    #[test]
    fn watcher_with_dedup_skips_hash_on_delete() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        File::create(&file_path).unwrap().write_all(b"content").unwrap();

        let mut watcher = FsWatcher::new(10, "proj", "machine")
            .with_content_dedup(ContentDedup::new(1024));
        let mut rx = watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();

        // Drain initial Create event
        std::thread::sleep(Duration::from_millis(50));
        let _ = rx.try_recv();

        // Delete the file
        fs::remove_file(&file_path).unwrap();

        std::thread::sleep(Duration::from_millis(50));

        // Find the Delete event
        let mut found_delete = false;
        for _ in 0..10 {
            if let Ok(event) = rx.try_recv() {
                if event.event_type == EventType::Deleted {
                    assert!(event.content_hash.is_none());
                    found_delete = true;
                    break;
                }
            } else {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        assert!(found_delete, "No Delete event found");
    }

    #[test]
    fn watcher_with_dedup_skips_hash_for_large_file() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher = FsWatcher::new(10, "proj", "machine")
            .with_content_dedup(ContentDedup::new(100)); // max 100 bytes
        let mut rx = watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();

        // Create a file larger than max_size
        let file_path = temp_dir.path().join("large.txt");
        let large_content = vec![b'x'; 200];
        File::create(&file_path).unwrap().write_all(&large_content).unwrap();

        std::thread::sleep(Duration::from_millis(50));

        let event = rx.try_recv().unwrap();
        assert_eq!(event.event_type, EventType::Created);
        assert!(event.content_hash.is_none()); // Too large, no hash
    }

    #[test]
    fn watcher_without_dedup_emits_no_hash() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher = FsWatcher::new(10, "proj", "machine");
        let mut rx = watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();

        let file_path = temp_dir.path().join("test.txt");
        File::create(&file_path).unwrap().write_all(b"hello").unwrap();

        std::thread::sleep(Duration::from_millis(50));

        let event = rx.try_recv().unwrap();
        assert_eq!(event.event_type, EventType::Created);
        assert!(event.content_hash.is_none());
    }

    // EventAttributor integration tests

    #[test]
    fn with_attributor_stores_attributor() {
        let attributor = EventAttributor::new();
        let temp = TempDir::new().unwrap();
        let watcher = FsWatcher::new(100, "proj", "machine")
            .with_attributor(attributor, temp.path().to_path_buf());
        assert!(watcher.attributor.is_some());
        assert!(watcher.project_root.is_some());
    }

    #[test]
    fn watcher_without_attributor_defaults_to_user() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher = FsWatcher::new(10, "proj", "machine");
        let mut rx = watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();

        let file_path = temp_dir.path().join("src.txt");
        File::create(&file_path).unwrap().write_all(b"content").unwrap();

        std::thread::sleep(Duration::from_millis(50));

        let event = rx.try_recv().unwrap();
        assert_eq!(event.source, Source::User);
    }

    #[test]
    fn watcher_with_attributor_detects_build_source() {
        let temp_dir = TempDir::new().unwrap();
        // Use custom filter that allows build directories for testing
        let filter = PathFilter::new(vec!["node_modules".to_string(), ".git".to_string()], 10 * 1024 * 1024);
        let mut watcher = FsWatcher::new(10, "proj", "machine")
            .with_path_filter(filter)
            .with_attributor(EventAttributor::new(), temp_dir.path().to_path_buf());
        let mut rx = watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();

        // Create a build artifact
        fs::create_dir_all(temp_dir.path().join("dist")).unwrap();
        let file_path = temp_dir.path().join("dist").join("app.js");
        File::create(&file_path).unwrap().write_all(b"bundle").unwrap();

        // Wait for file event (skip directory events)
        let mut found = false;
        for _ in 0..30 {
            if let Ok(event) = rx.try_recv() {
                // Only care about file events, not directory events
                if !event.file_path.ends_with("dist") && event.file_path.contains("dist") {
                    assert_eq!(event.source, Source::Build);
                    found = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(found, "No file event received");
    }

    #[test]
    fn watcher_with_attributor_detects_user_source() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher = FsWatcher::new(10, "proj", "machine")
            .with_attributor(EventAttributor::new(), temp_dir.path().to_path_buf());
        let mut rx = watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();

        fs::create_dir_all(temp_dir.path().join("src")).unwrap();
        let file_path = temp_dir.path().join("src").join("main.rs");
        File::create(&file_path).unwrap().write_all(b"fn main() {}").unwrap();

        std::thread::sleep(Duration::from_millis(50));

        let event = rx.try_recv().unwrap();
        assert_eq!(event.source, Source::User);
    }

    #[test]
    fn watcher_with_explicit_source_overrides() {
        let temp_dir = TempDir::new().unwrap();
        let attributor = EventAttributor::new().with_explicit_source(Source::AiAgent);
        let mut watcher = FsWatcher::new(10, "proj", "machine")
            .with_attributor(attributor, temp_dir.path().to_path_buf());
        let mut rx = watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();

        let file_path = temp_dir.path().join("any.txt");
        File::create(&file_path).unwrap().write_all(b"content").unwrap();

        std::thread::sleep(Duration::from_millis(50));

        let event = rx.try_recv().unwrap();
        assert_eq!(event.source, Source::AiAgent);
    }

    // Combined dedup + attributor tests

    #[test]
    fn watcher_with_both_dedup_and_attributor() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher = FsWatcher::new(10, "proj", "machine")
            .with_content_dedup(ContentDedup::new(1024))
            .with_attributor(EventAttributor::new(), temp_dir.path().to_path_buf());
        let mut rx = watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();

        let file_path = temp_dir.path().join("code").join("lib.rs");
        fs::create_dir_all(temp_dir.path().join("code")).unwrap();
        File::create(&file_path).unwrap().write_all(b"pub fn hello() {}").unwrap();
        fs::File::create(&file_path).unwrap().sync_all().unwrap(); // Ensure content is flushed

        // Wait for event with retry
        let mut found = false;
        for _ in 0..30 {
            if let Ok(event) = rx.try_recv() {
                // Skip directory events
                if event.file_path.ends_with("code") {
                    continue;
                }
                assert_eq!(event.event_type, EventType::Created);
                assert_eq!(event.source, Source::User);
                assert!(event.content_hash.is_some());
                found = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(found, "No event received");
    }

    #[test]
    fn watcher_with_both_detects_build_and_hashes() {
        let temp_dir = TempDir::new().unwrap();
        // Use custom filter that allows build directories for testing
        let filter = PathFilter::new(vec!["node_modules".to_string(), ".git".to_string()], 10 * 1024 * 1024);
        let mut watcher = FsWatcher::new(10, "proj", "machine")
            .with_path_filter(filter)
            .with_content_dedup(ContentDedup::new(1024))
            .with_attributor(EventAttributor::new(), temp_dir.path().to_path_buf());
        let mut rx = watcher.start().unwrap();
        watcher.watch(temp_dir.path().to_path_buf()).unwrap();

        let file_path = temp_dir.path().join("dist").join("bundle.o");
        fs::create_dir_all(temp_dir.path().join("dist")).unwrap();
        File::create(&file_path).unwrap().write_all(b"object bytes").unwrap();

        // Wait for file event (skip directory events)
        let mut found = false;
        for _ in 0..30 {
            if let Ok(event) = rx.try_recv() {
                // Only care about file events
                if event.file_path.ends_with("bundle.o") {
                    assert_eq!(event.source, Source::Build);
                    assert!(event.content_hash.is_some());
                    found = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(found, "No file event received");
    }
}
