// End-to-end test for ContentDedup and EventAttributor integration

use std::fs::{self, File};
use std::io::Write;
use std::time::Duration;
use tempfile::TempDir;

use ambient_fs_watcher::{ContentDedup, EventAttributor, FsWatcher};
use ambient_fs_core::{event::EventType, event::Source, filter::PathFilter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Integration Wiring E2E Test ===\n");

    // Create temp directory
    let temp_dir = TempDir::new()?;
    println!("temp dir: {:?}", temp_dir.path());

    // Custom filter that allows build directories for testing
    let filter = PathFilter::new(vec!["node_modules".to_string(), ".git".to_string()], 10 * 1024 * 1024);

    // Create watcher with both ContentDedup and EventAttributor
    let mut watcher = FsWatcher::new(10, "test-project", "test-machine")
        .with_path_filter(filter)
        .with_content_dedup(ContentDedup::new(10 * 1024 * 1024)) // 10MB
        .with_attributor(EventAttributor::new(), temp_dir.path().to_path_buf());

    let mut rx = watcher.start()?;
    watcher.watch(temp_dir.path().to_path_buf())?;

    println!("watcher started with ContentDedup and EventAttributor");

    // Test 1: Create a normal file (should have User source + hash)
    println!("\n--- Test 1: Normal file creation ---");
    let src_file = temp_dir.path().join("src").join("main.rs");
    fs::create_dir_all(temp_dir.path().join("src"))?;
    File::create(&src_file)?.write_all(b"fn main() { println!(\"hello\"); }")?;
    println!("created: {:?}", src_file);

    std::thread::sleep(Duration::from_millis(100));

    // Drain all events and find the file event (skip directory events)
    let event = loop {
        let e = rx.try_recv()?;
        if e.file_path.ends_with("main.rs") {
            break e;
        }
    };

    println!("event_type: {:?}", event.event_type);
    println!("source: {:?}", event.source);
    println!("content_hash: {} chars", event.content_hash.as_ref().map(|h| h.len()).unwrap_or(0));

    assert_eq!(event.event_type, EventType::Created);
    assert_eq!(event.source, Source::User);
    assert!(event.content_hash.is_some(), "should have hash");
    println!("✓ Normal file: User source + hash present");

    // Test 2: Create a build artifact (should have Build source + hash)
    println!("\n--- Test 2: Build artifact creation ---");
    let build_file = temp_dir.path().join("dist").join("bundle.js");
    fs::create_dir_all(temp_dir.path().join("dist"))?;
    File::create(&build_file)?.write_all(b"console.log('bundled');")?;
    println!("created: {:?}", build_file);

    // Find the actual file event (skip directory events) with retry
    let event = loop {
        match rx.try_recv() {
            Ok(e) if e.file_path.ends_with("bundle.js") => break e,
            Ok(_) => continue, // Skip other events
            Err(_) => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
        }
    };

    println!("event_type: {:?}", event.event_type);
    println!("source: {:?}", event.source);
    println!("content_hash: {} chars", event.content_hash.as_ref().map(|h| h.len()).unwrap_or(0));

    assert_eq!(event.source, Source::Build, "should detect Build source");
    assert!(event.content_hash.is_some(), "should have hash");
    println!("✓ Build artifact: Build source + hash present");

    // Test 3: Modify file (should update hash)
    println!("\n--- Test 3: File modification ---");
    let old_hash = event.content_hash.clone();
    fs::write(&src_file, b"fn main() { println!(\"modified\"); }")?;
    println!("modified: {:?}", src_file);

    // Find the modify event with retry
    let event = loop {
        match rx.try_recv() {
            Ok(e) if e.event_type == EventType::Modified && e.file_path.ends_with("main.rs") => break e,
            Ok(_) => continue,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
        }
    };

    println!("event_type: {:?}", event.event_type);
    println!("new hash: {} chars", event.content_hash.as_ref().map(|h| h.len()).unwrap_or(0));

    assert_eq!(event.event_type, EventType::Modified);
    assert!(event.content_hash.is_some());
    assert_ne!(event.content_hash, old_hash, "hash should change after modification");
    println!("✓ Modification: hash updated");

    // Test 4: Delete file (should have no hash)
    println!("\n--- Test 4: File deletion ---");
    fs::remove_file(&src_file)?;
    println!("deleted: {:?}", src_file);

    // Find the delete event with retry
    let event = loop {
        match rx.try_recv() {
            Ok(e) if e.event_type == EventType::Deleted && e.file_path.ends_with("main.rs") => break e,
            Ok(_) => continue,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
        }
    };

    println!("event_type: {:?}", event.event_type);
    println!("content_hash: {:?}", event.content_hash);

    assert_eq!(event.event_type, EventType::Deleted);
    assert!(event.content_hash.is_none(), "deleted files should have no hash");
    println!("✓ Deletion: no hash (file is gone)");

    // Test 5: Large file (should skip hashing)
    println!("\n--- Test 5: Large file (> max_size) ---");
    let large_file = temp_dir.path().join("large.bin");
    let large_content = vec![b'x'; 20 * 1024 * 1024]; // 20MB
    File::create(&large_file)?.write_all(&large_content)?;
    let actual_size = std::fs::metadata(&large_file)?.len();
    println!("created large file: {} MB ({} bytes)", actual_size / 1024 / 1024, actual_size);

    // Find the large file event with retry
    let event = loop {
        match rx.try_recv() {
            Ok(e) if e.file_path.ends_with("large.bin") => break e,
            Ok(_) => continue,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
        }
    };

    println!("event_type: {:?}", event.event_type);
    println!("content_hash: {:?}", event.content_hash);
    assert!(event.content_hash.is_none(), "large files should skip hashing");
    println!("✓ Large file: hash skipped (exceeds max_size)");

    println!("\n=== All E2E tests passed ===");
    Ok(())
}
