// Filesystem watcher with debounce and attribution

mod attribution;
mod dedup;
mod watcher;

pub use attribution::{BuildPatterns, EventAttributor};
pub use dedup::{ContentDedup, HashError};
pub use watcher::{EventReceiver, FsWatcher, WatchError};
