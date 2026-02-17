# ambient-fs-watcher

Filesystem watcher with debouncing and source attribution. Wraps notify 8.

## Features

- **FsWatcher** - Watch directories for changes
  - Configurable debounce (default 100ms)
  - Path filtering (ignore patterns)
  - Returns `Receiver<FileEvent>` from `start()`

- **ContentDedup** - Blake3 content hashing
  - `hash_file(path)` - Compute blake3 hash
  - `hash_matches(path, expected)` - Compare without re-reading
  - Max file size enforcement

- **EventAttributor** - Source detection
  - Git detection (via .git/index mtime)
  - Build detection (path patterns like dist/, target/)
  - Explicit attribution (passed in)
  - Default to user

## Usage

```rust
use ambient_fs_watcher::FsWatcher;

let mut watcher = FsWatcher::new(100, "my-project", "machine-1");
let mut events = watcher.start()?;

watcher.watch("/path/to/project")?;

while let Some(event) = events.recv().await {
    println!("{:?} {}", event.event_type, event.file_path);
}
```

## Attribution Priority

1. Explicit (client-provided)
2. Git (.git/index mtime changed recently)
3. Build (path matches dist/, target/, .next/)
4. User (default)

## License

MIT
