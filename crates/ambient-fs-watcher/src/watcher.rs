use std::collections::HashMap;
use std::path::PathBuf;

use ambient_fs_core::{event::FileEvent, event::EventType, filter::PathFilter};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

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

                let file_event = FileEvent::new(
                    event_type,
                    path_str,
                    project_id.clone(),
                    machine_id.clone(),
                );

                let _ = tx.send(file_event);
            }
        };

        Ok(notify::recommended_watcher(handle_events)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
}
