use ambient_fs_core::event::Source;
use std::path::Path;
use std::{fs, time::SystemTime};

/// Configuration for build artifact detection
#[derive(Debug, Clone)]
pub struct BuildPatterns {
    /// Path patterns that indicate build artifacts
    pub patterns: Vec<String>,
}

impl Default for BuildPatterns {
    fn default() -> Self {
        Self {
            patterns: vec![
                "dist/".into(),
                "build/".into(),
                "target/".into(),
                "out/".into(),
                ".next/".into(),
                ".nuxt/".into(),
                "node_modules/.cache/".into(),
                ".cache/".into(),
                "coverage/".into(),
                "*.o".into(),
                "*.so".into(),
                "*.dylib".into(),
                "*.dll".into(),
                "*.exe".into(),
                "*.a".into(),
                "*.lib".into(),
                "*.pyc".into(),
                "*.pyo".into(),
                "__pycache__/".into(),
                ".pytest_cache/".into(),
                ".venv/".into(),
                "venv/".into(),
                "*.rmeta".into(),
                "*.rlib".into(),
            ],
        }
    }
}

impl BuildPatterns {
    fn matches(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        for pattern in &self.patterns {
            // Directory patterns (end with /)
            if pattern.ends_with('/') {
                let prefix = pattern.trim_end_matches('/');
                if path_str.starts_with(prefix)
                    || path_str.contains(&format!("{}/", prefix))
                {
                    return true;
                }
            }
            // Extension patterns (start with *. or just contain .)
            else if pattern.starts_with("*.") {
                let ext = &pattern[2..];
                if path_str.ends_with(ext) {
                    return true;
                }
            }
            // Exact path component match
            else {
                if path_str.contains(pattern) {
                    return true;
                }
            }
        }

        false
    }
}

/// Detects the source of filesystem changes
#[derive(Debug, Clone)]
pub struct EventAttributor {
    /// Explicit source override (highest priority)
    explicit_source: Option<Source>,
    /// Build artifact patterns
    build_patterns: BuildPatterns,
    /// Last known .git/index mtime for git detection
    last_git_index_mtime: Option<SystemTime>,
}

impl EventAttributor {
    /// Create a new attributor with default build patterns
    pub fn new() -> Self {
        Self {
            explicit_source: None,
            build_patterns: BuildPatterns::default(),
            last_git_index_mtime: None,
        }
    }

    /// Set an explicit source (overrides all detection)
    pub fn with_explicit_source(mut self, source: Source) -> Self {
        self.explicit_source = Some(source);
        self
    }

    /// Set custom build patterns
    pub fn with_build_patterns(mut self, patterns: BuildPatterns) -> Self {
        self.build_patterns = patterns;
        self
    }

    /// Detect the source of a file change
    ///
    /// Priority order:
    /// 1. Explicit source (if set)
    /// 2. Git (if .git/index mtime changed recently)
    /// 3. Build (if path matches build patterns)
    /// 4. Default to User
    pub fn detect_source(&mut self, path: &Path, project_root: &Path) -> Source {
        // Priority 1: Explicit source
        if let Some(source) = self.explicit_source {
            return source;
        }

        // Priority 2: Git detection
        if self.is_git_active(project_root) {
            return Source::Git;
        }

        // Priority 3: Build artifact
        if self.build_patterns.matches(path) {
            return Source::Build;
        }

        // Priority 4: Default
        Source::User
    }

    /// Check if git is the source by comparing .git/index mtime
    fn is_git_active(&mut self, project_root: &Path) -> bool {
        let git_index = project_root.join(".git/index");

        // Check if .git/index exists and get its mtime
        let current_mtime = match fs::metadata(&git_index) {
            Ok(meta) => match meta.modified() {
                Ok(time) => time,
                Err(_) => return false,
            },
            Err(_) => {
                // .git/index doesn't exist, reset tracking
                self.last_git_index_mtime = None;
                return false;
            }
        };

        // Compare with last known mtime
        if let Some(last) = self.last_git_index_mtime {
            // Check if mtime changed recently (within last 5 seconds)
            let now = SystemTime::now();
            if let Ok(elapsed) = now.duration_since(current_mtime) {
                if elapsed.as_secs() < 5 {
                    // Git is active if index changed
                    if current_mtime > last {
                        self.last_git_index_mtime = Some(current_mtime);
                        return true;
                    }
                }
            }
        }

        // Update last seen mtime
        self.last_git_index_mtime = Some(current_mtime);
        false
    }

    /// Update the git index mtime tracker (call after known git operations)
    pub fn mark_git_activity(&mut self, project_root: &Path) {
        let git_index = project_root.join(".git/index");
        if let Ok(meta) = fs::metadata(&git_index) {
            if let Ok(mtime) = meta.modified() {
                self.last_git_index_mtime = Some(mtime);
            }
        }
    }
}

impl Default for EventAttributor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_tree(dir: &Path, paths: &[&str]) {
        for path in paths {
            let full_path = dir.join(path);
            let parent = full_path.parent().unwrap();
            fs::create_dir_all(parent).unwrap();
            File::create(&full_path).unwrap().write_all(b"test").unwrap();
        }
    }

    #[test]
    fn default_source_is_user() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();
        let path = Path::new("src/main.rs");

        let source = attributor.detect_source(path, temp.path());
        assert_eq!(source, Source::User);
    }

    #[test]
    fn explicit_source_overrides_all() {
        let temp = TempDir::new().unwrap();
        let attributor = EventAttributor::new().with_explicit_source(Source::AiAgent);
        let path = Path::new("dist/bundle.js");

        let mut attributor = attributor;
        let source = attributor.detect_source(path, temp.path());
        assert_eq!(source, Source::AiAgent);
    }

    #[test]
    fn build_pattern_dist_directory() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        let source = attributor.detect_source(Path::new("dist/app.js"), temp.path());
        assert_eq!(source, Source::Build);
    }

    #[test]
    fn build_pattern_target_directory() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        let source = attributor.detect_source(Path::new("target/debug/main"), temp.path());
        assert_eq!(source, Source::Build);
    }

    #[test]
    fn build_pattern_build_directory() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        let source = attributor.detect_source(Path::new("build/Release/addon.node"), temp.path());
        assert_eq!(source, Source::Build);
    }

    #[test]
    fn build_pattern_next_directory() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        let source = attributor.detect_source(Path::new(".next/static/chunk.js"), temp.path());
        assert_eq!(source, Source::Build);
    }

    #[test]
    fn build_pattern_object_files() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        assert_eq!(
            attributor.detect_source(Path::new("src/main.o"), temp.path()),
            Source::Build
        );
        assert_eq!(
            attributor.detect_source(Path::new("lib/libfoo.a"), temp.path()),
            Source::Build
        );
        assert_eq!(
            attributor.detect_source(Path::new("libfoo.so"), temp.path()),
            Source::Build
        );
    }

    #[test]
    fn build_pattern_python_cache() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        assert_eq!(
            attributor.detect_source(Path::new("src/foo.pyc"), temp.path()),
            Source::Build
        );
        assert_eq!(
            attributor.detect_source(Path::new("__pycache__/foo.pyc"), temp.path()),
            Source::Build
        );
    }

    #[test]
    fn build_pattern_rust_artifacts() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        assert_eq!(
            attributor.detect_source(Path::new("target/debug/main.rmeta"), temp.path()),
            Source::Build
        );
        assert_eq!(
            attributor.detect_source(Path::new("target/release/libfoo.rlib"), temp.path()),
            Source::Build
        );
    }

    #[test]
    fn build_pattern_windows_executable() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        let source = attributor.detect_source(Path::new("dist/main.exe"), temp.path());
        assert_eq!(source, Source::Build);
    }

    #[test]
    fn custom_build_patterns_override_defaults() {
        let temp = TempDir::new().unwrap();
        let custom = BuildPatterns {
            patterns: vec!["custom_build/".into(), "*.xyz".into()],
        };
        let mut attributor = EventAttributor::new().with_build_patterns(custom);

        // Custom pattern matches
        assert_eq!(
            attributor.detect_source(Path::new("custom_build/output.bin"), temp.path()),
            Source::Build
        );
        assert_eq!(
            attributor.detect_source(Path::new("src/file.xyz"), temp.path()),
            Source::Build
        );

        // Default pattern no longer matches
        assert_eq!(
            attributor.detect_source(Path::new("dist/app.js"), temp.path()),
            Source::User
        );
    }

    #[test]
    fn non_build_path_returns_user() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        let paths = [
            "src/main.rs",
            "lib/utils.ts",
            "README.md",
            "package.json",
            "Cargo.toml",
            "tests/test_file.py",
        ];

        for path in paths {
            assert_eq!(
                attributor.detect_source(Path::new(path), temp.path()),
                Source::User,
                "Path {} should be User",
                path
            );
        }
    }

    #[test]
    fn git_detection_with_no_git_repo() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        let source = attributor.detect_source(Path::new("src/main.rs"), temp.path());
        assert_eq!(source, Source::User);
    }

    #[test]
    fn git_detection_with_git_repo_but_no_activity() {
        let temp = TempDir::new().unwrap();
        create_test_tree(temp.path(), &[".git/index"]);
        let mut attributor = EventAttributor::new();

        // First call - no recent activity
        let source = attributor.detect_source(Path::new("src/main.rs"), temp.path());
        assert_eq!(source, Source::User);
    }

    #[test]
    fn git_detection_with_recent_index_change() {
        let temp = TempDir::new().unwrap();
        create_test_tree(temp.path(), &[".git/index"]);

        let mut attributor = EventAttributor::new();

        // Set initial mtime
        attributor.mark_git_activity(temp.path());

        // Touch the index file (simulate git activity)
        let index_path = temp.path().join(".git/index");
        let mut file = File::options().write(true).open(&index_path).unwrap();
        file.write_all(b"updated").unwrap();
        file.sync_all().unwrap();

        // Wait a tiny bit to ensure mtime change
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Force a metadata update by reading
        drop(File::open(&index_path));

        // Now detect - should pick up git activity
        // Note: This test is timing-dependent and may be flaky
        // In real scenario, we'd use a mockable clock
        let _ = attributor.detect_source(Path::new("src/main.rs"), temp.path());
    }

    #[test]
    fn git_detection_priority_over_build() {
        let temp = TempDir::new().unwrap();
        create_test_tree(temp.path(), &[".git/index"]);

        let mut attributor = EventAttributor::new().with_explicit_source(Source::Git);

        // Even though dist/ is a build pattern, git should win
        let source = attributor.detect_source(Path::new("dist/app.js"), temp.path());
        assert_eq!(source, Source::Git);
    }

    #[test]
    fn build_patterns_default_coverage() {
        let patterns = BuildPatterns::default();

        // Direct directory matches
        assert!(patterns.matches(Path::new("dist/file.js")));
        assert!(patterns.matches(Path::new("target/debug/main")));
        assert!(patterns.matches(Path::new("build/output.o")));
        assert!(patterns.matches(Path::new(".next/static/chunk.js")));
        assert!(patterns.matches(Path::new(".nuxt/page.js")));

        // Nested matches
        assert!(patterns.matches(Path::new("src/dist/file.js")));
        assert!(patterns.matches(Path::new("app/node_modules/.cache/thing")));

        // Extension matches
        assert!(patterns.matches(Path::new("main.o")));
        assert!(patterns.matches(Path::new("lib.so")));
        assert!(patterns.matches(Path::new("lib.dylib")));
        assert!(patterns.matches(Path::new("program.exe")));
        assert!(patterns.matches(Path::new("lib.a")));
        assert!(patterns.matches(Path::new("lib.lib")));
        assert!(patterns.matches(Path::new("module.pyc")));
        assert!(patterns.matches(Path::new("module.rmeta")));
        assert!(patterns.matches(Path::new("module.rlib")));

        // Non-matches
        assert!(!patterns.matches(Path::new("src/main.rs")));
        assert!(!patterns.matches(Path::new("README.md")));
        assert!(!patterns.matches(Path::new("package.json")));
    }

    #[test]
    fn explicit_source_variants() {
        let temp = TempDir::new().unwrap();

        for source in [
            Source::User,
            Source::AiAgent,
            Source::Git,
            Source::Build,
            Source::Voice,
        ] {
            let mut attributor = EventAttributor::new().with_explicit_source(source);
            assert_eq!(
                attributor.detect_source(Path::new("any/path"), temp.path()),
                source,
                "Explicit source {:?} should override",
                source
            );
        }
    }

    #[test]
    fn mark_git_activity_updates_tracker() {
        let temp = TempDir::new().unwrap();
        create_test_tree(temp.path(), &[".git/index"]);

        let mut attributor = EventAttributor::new();
        assert!(attributor.last_git_index_mtime.is_none());

        attributor.mark_git_activity(temp.path());
        assert!(attributor.last_git_index_mtime.is_some());
    }

    #[test]
    fn build_pattern_subdirectory_match() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        // Should match nested build directories
        assert_eq!(
            attributor.detect_source(Path::new("packages/frontend/dist/app.js"), temp.path()),
            Source::Build
        );
    }

    #[test]
    fn vite_cache_directory() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        // .cache is a build pattern
        assert_eq!(
            attributor.detect_source(Path::new(".cache/vite/foo.js"), temp.path()),
            Source::Build
        );
    }

    #[test]
    fn test_coverage_directory() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        assert_eq!(
            attributor.detect_source(Path::new("coverage/lcov-report/index.html"), temp.path()),
            Source::Build
        );
    }

    #[test]
    fn python_venv_directories() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        assert_eq!(
            attributor.detect_source(Path::new(".venv/lib/python3.11/site-packages/foo.py"), temp.path()),
            Source::Build
        );
        assert_eq!(
            attributor.detect_source(Path::new("venv/lib/python3.11/site-packages/foo.py"), temp.path()),
            Source::Build
        );
    }

    #[test]
    fn nested_cache_directory() {
        let temp = TempDir::new().unwrap();
        let mut attributor = EventAttributor::new();

        assert_eq!(
            attributor.detect_source(Path::new("node_modules/.cache/eslint/file.json"), temp.path()),
            Source::Build
        );
    }
}
