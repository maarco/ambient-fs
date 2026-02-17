use std::collections::HashMap;
use std::path::Path;
use std::sync::{OnceLock, RwLock};

/// Language-specific analysis configuration
#[derive(Debug, Clone, PartialEq)]
pub struct LanguageConfig {
    /// Display name (e.g. "TypeScript", "Rust")
    pub name: &'static str,
    /// File extensions this applies to (e.g. ["ts", "tsx"])
    pub extensions: &'static [&'static str],
    /// Feature flags for what to extract
    pub features: LanguageFeatures,
}

/// Which analysis features to enable for a language
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LanguageFeatures {
    /// Extract export statements (export foo, export function bar() {})
    pub extract_exports: bool,
    /// Extract import statements (import foo from 'bar')
    pub extract_imports: bool,
    /// Count TODO/FIXME/HACK/XXX comments
    pub count_todos: bool,
    /// Count total lines
    pub count_lines: bool,
}

impl LanguageFeatures {
    /// All features enabled
    pub const fn all() -> Self {
        Self {
            extract_exports: true,
            extract_imports: true,
            count_todos: true,
            count_lines: true,
        }
    }

    /// All features disabled
    pub const fn none() -> Self {
        Self {
            extract_exports: false,
            extract_imports: false,
            count_todos: false,
            count_lines: false,
        }
    }
}

/// Owned language config for dynamic registration
///
/// Like LanguageConfig but with owned String instead of &'static str
/// to support runtime-registered languages.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedLanguageConfig {
    /// Display name (e.g. "TypeScript", "Rust")
    pub name: String,
    /// File extensions this applies to (e.g. ["ts", "tsx"])
    pub extensions: Vec<String>,
    /// Feature flags for what to extract
    pub features: LanguageFeatures,
}

impl From<OwnedLanguageConfig> for LanguageConfig {
    fn from(owned: OwnedLanguageConfig) -> Self {
        Self {
            name: "owned",
            extensions: &[],
            features: owned.features,
        }
    }
}

/// Global language registry
///
/// Maps extensions to LanguageConfig. Two layers:
/// - Built-in languages via OnceLock (zero-cost reads)
/// - Custom languages via RwLock (dynamic registration)
pub struct LanguageRegistry {
    /// Extension -> LanguageConfig mapping for builtins (e.g. "ts" -> typescript config)
    by_extension: HashMap<&'static str, &'static LanguageConfig>,
    /// Custom languages registered at runtime (extension -> owned config)
    custom: RwLock<HashMap<String, OwnedLanguageConfig>>,
}

impl LanguageRegistry {
    /// Get the global singleton registry
    fn global() -> &'static Self {
        static REGISTRY: OnceLock<LanguageRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| Self::with_builtin_languages())
    }

    /// Create registry with all built-in language configs
    fn with_builtin_languages() -> Self {
        let configs = builtin_configs();
        let mut by_extension = HashMap::new();

        for &config in configs {
            for &ext in config.extensions {
                by_extension.insert(ext, config);
            }
        }

        Self {
            by_extension,
            custom: RwLock::default(),
        }
    }

    /// Look up language config by file path
    ///
    /// Checks custom registry first, then builtins.
    /// Returns None if the extension is not recognized.
    pub fn get_for_path(path: impl AsRef<Path>) -> Option<LanguageConfig> {
        let ext = path.as_ref()
            .extension()
            .and_then(|ext| ext.to_str())?;

        let registry = Self::global();

        // Check custom languages first (allows override)
        if let Ok(custom) = registry.custom.try_read() {
            if let Some(owned) = custom.get(ext) {
                return Some(LanguageConfig {
                    name: "custom",
                    extensions: &[],
                    features: owned.features,
                });
            }
        }

        // Fall back to builtins
        registry.by_extension.get(ext)
            .map(|cfg| (**cfg).clone())
    }

    /// Register a custom language config
    ///
    /// Custom languages override builtins for matching extensions.
    /// Thread-safe: can be called from multiple threads.
    pub fn register(config: OwnedLanguageConfig) {
        let registry = Self::global();
        if let Ok(mut custom) = registry.custom.write() {
            for ext in &config.extensions {
                custom.insert(ext.clone(), config.clone());
            }
        }
    }

    /// Clear all custom registrations (test-only)
    ///
    /// # Panics
    /// Panics if the RwLock is poisoned
    #[cfg(test)]
    fn reset_custom() {
        let registry = Self::global();
        if let Ok(mut custom) = registry.custom.write() {
            custom.clear();
        }
    }
}

/// Built-in language definitions
static TYPESCRIPT: LanguageConfig = LanguageConfig {
    name: "TypeScript",
    extensions: &["ts", "tsx"],
    features: LanguageFeatures::all(),
};

static JAVASCRIPT: LanguageConfig = LanguageConfig {
    name: "JavaScript",
    extensions: &["js", "jsx", "mjs", "cjs"],
    features: LanguageFeatures::all(),
};

static RUST: LanguageConfig = LanguageConfig {
    name: "Rust",
    extensions: &["rs"],
    features: LanguageFeatures::all(),
};

static PYTHON: LanguageConfig = LanguageConfig {
    name: "Python",
    extensions: &["py", "pyi"],
    features: LanguageFeatures::all(),
};

static VUE: LanguageConfig = LanguageConfig {
    name: "Vue",
    extensions: &["vue"],
    features: LanguageFeatures::all(),
};

static MARKDOWN: LanguageConfig = LanguageConfig {
    name: "Markdown",
    extensions: &["md", "markdown"],
    features: LanguageFeatures {
        extract_exports: false,
        extract_imports: false,
        count_todos: true,
        count_lines: true,
    },
};

static BUILTIN_CONFIGS: &[&LanguageConfig] = &[
    &TYPESCRIPT,
    &JAVASCRIPT,
    &RUST,
    &PYTHON,
    &VUE,
    &MARKDOWN,
];

fn builtin_configs() -> &'static [&'static LanguageConfig] {
    BUILTIN_CONFIGS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_features_all() {
        let f = LanguageFeatures::all();
        assert!(f.extract_exports);
        assert!(f.extract_imports);
        assert!(f.count_todos);
        assert!(f.count_lines);
    }

    #[test]
    fn language_features_none() {
        let f = LanguageFeatures::none();
        assert!(!f.extract_exports);
        assert!(!f.extract_imports);
        assert!(!f.count_todos);
        assert!(!f.count_lines);
    }

    #[test]
    fn get_for_path_typescript() {
        LanguageRegistry::reset_custom();

        let config = LanguageRegistry::get_for_path("src/components/Button.tsx").unwrap();
        assert_eq!(config.name, "TypeScript");
        assert!(config.features.extract_exports);
        assert!(config.features.extract_imports);
    }

    #[test]
    fn get_for_path_javascript() {
        LanguageRegistry::reset_custom();

        let config = LanguageRegistry::get_for_path("utils.js").unwrap();
        assert_eq!(config.name, "JavaScript");
        // mjs extension
        let config2 = LanguageRegistry::get_for_path("app.mjs").unwrap();
        assert_eq!(config2.name, "JavaScript");
    }

    #[test]
    fn get_for_path_rust() {
        LanguageRegistry::reset_custom();

        let config = LanguageRegistry::get_for_path("main.rs").unwrap();
        assert_eq!(config.name, "Rust");
    }

    #[test]
    fn get_for_path_python() {
        LanguageRegistry::reset_custom();

        let config = LanguageRegistry::get_for_path("app.py").unwrap();
        assert_eq!(config.name, "Python");
        // pyi extension (type stubs)
        let config2 = LanguageRegistry::get_for_path("types.pyi").unwrap();
        assert_eq!(config2.name, "Python");
    }

    #[test]
    fn get_for_path_vue() {
        LanguageRegistry::reset_custom();

        let config = LanguageRegistry::get_for_path("App.vue").unwrap();
        assert_eq!(config.name, "Vue");
    }

    #[test]
    fn get_for_path_markdown() {
        LanguageRegistry::reset_custom();  // ensure clean state

        let config = LanguageRegistry::get_for_path("README.md").unwrap();
        assert_eq!(config.name, "Markdown");
        // markdown should not extract imports/exports
        assert!(!config.features.extract_imports);
        assert!(!config.features.extract_exports);
        // but should still count todos and lines
        assert!(config.features.count_todos);
        assert!(config.features.count_lines);
    }

    #[test]
    fn get_for_path_unknown_extension() {
        let config = LanguageRegistry::get_for_path("data.xml");
        assert!(config.is_none());
        // no extension
        let config2 = LanguageRegistry::get_for_path("Makefile");
        assert!(config2.is_none());
    }

    #[test]
    fn get_for_path_dotfile() {
        // .gitignore has no extension
        let config = LanguageRegistry::get_for_path(".gitignore");
        assert!(config.is_none());
    }

    #[test]
    fn builtin_configs_all_have_extensions() {
        for config in builtin_configs() {
            assert!(!config.extensions.is_empty(), "{} has no extensions", config.name);
        }
    }

    #[test]
    fn language_config_clone_and_eq() {
        let cfg = builtin_configs()[0];
        assert_eq!(cfg.name, "TypeScript");
    }

    #[test]
    fn multiple_extensions_same_language() {
        LanguageRegistry::reset_custom();

        // ts and tsx both map to TypeScript
        let cfg1 = LanguageRegistry::get_for_path("file.ts").unwrap();
        let cfg2 = LanguageRegistry::get_for_path("file.tsx").unwrap();
        assert_eq!(cfg1.name, cfg2.name);
        assert_eq!(cfg1.features, cfg2.features);
    }

    #[test]
    fn case_sensitive_extensions() {
        LanguageRegistry::reset_custom();

        // .TS is different from .ts on unix
        let cfg1 = LanguageRegistry::get_for_path("file.ts");
        let cfg2 = LanguageRegistry::get_for_path("file.TS");
        // lowercase should work
        assert!(cfg1.is_some());
        // uppercase might not (depends on filesystem)
        // but our registry is lowercase-only
        assert!(cfg2.is_none());
    }

    // === dynamic registration tests ===

    #[test]
    fn register_custom_language() {
        LanguageRegistry::reset_custom();  // isolate from other tests

        // Register Svelte with custom features
        let svelte = OwnedLanguageConfig {
            name: "Svelte".to_string(),
            extensions: vec!["svelte".to_string()],
            features: LanguageFeatures {
                extract_exports: true,
                extract_imports: true,
                count_todos: false,  // custom: don't count todos
                count_lines: true,
            },
        };

        LanguageRegistry::register(svelte);

        // Now .svelte files should be recognized
        let config = LanguageRegistry::get_for_path("App.svelte").unwrap();
        assert!(config.features.extract_exports);
        assert!(config.features.extract_imports);
        assert!(!config.features.count_todos);  // custom setting
        assert!(config.features.count_lines);
    }

    #[test]
    fn custom_language_multiple_extensions() {
        LanguageRegistry::reset_custom();  // isolate from other tests

        // Register a language with multiple extensions
        let custom = OwnedLanguageConfig {
            name: "Elm".to_string(),
            extensions: vec!["elm".to_string(), "elmi".to_string()],
            features: LanguageFeatures::all(),
        };

        LanguageRegistry::register(custom);

        let cfg1 = LanguageRegistry::get_for_path("Main.elm").unwrap();
        let cfg2 = LanguageRegistry::get_for_path("Types.elmi").unwrap();

        assert!(cfg1.features.extract_exports);
        assert!(cfg2.features.extract_exports);
    }

    #[test]
    fn custom_overrides_builtin() {
        LanguageRegistry::reset_custom();  // isolate from other tests

        // Verify builtin .md behavior first
        let builtin_cfg = LanguageRegistry::get_for_path("README.md").unwrap();
        assert!(!builtin_cfg.features.extract_imports);
        assert!(!builtin_cfg.features.extract_exports);

        // Register custom Markdown with all features enabled
        let custom_md = OwnedLanguageConfig {
            name: "CustomMarkdown".to_string(),
            extensions: vec!["md".to_string()],
            features: LanguageFeatures::all(),
        };

        LanguageRegistry::register(custom_md);

        // Now .md should have different features
        let override_cfg = LanguageRegistry::get_for_path("README.md").unwrap();
        assert!(override_cfg.features.extract_imports);  // now true
        assert!(override_cfg.features.extract_exports);  // now true
    }

    #[test]
    fn builtin_still_works_after_custom_registration() {
        LanguageRegistry::reset_custom();  // isolate from other tests

        // Register a custom language
        let svelte = OwnedLanguageConfig {
            name: "Svelte".to_string(),
            extensions: vec!["svelte".to_string()],
            features: LanguageFeatures::all(),
        };
        LanguageRegistry::register(svelte);

        // Builtins should still work
        let rust_cfg = LanguageRegistry::get_for_path("main.rs").unwrap();
        assert_eq!(rust_cfg.name, "Rust");

        let ts_cfg = LanguageRegistry::get_for_path("file.ts").unwrap();
        assert_eq!(ts_cfg.name, "TypeScript");
    }

    #[test]
    fn register_is_thread_safe() {
        LanguageRegistry::reset_custom();  // isolate from other tests

        use std::thread;

        // Register same language from multiple threads
        let custom = OwnedLanguageConfig {
            name: "ThreadSafe".to_string(),
            extensions: vec!["custom".to_string()],
            features: LanguageFeatures::all(),
        };

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let config = custom.clone();
                thread::spawn(move || {
                    LanguageRegistry::register(config);
                })
            })
            .collect();

        // All threads should complete without panic
        for handle in handles {
            handle.join().unwrap();
        }

        // Language should still be registered
        let cfg = LanguageRegistry::get_for_path("file.custom").unwrap();
        assert!(cfg.features.extract_exports);
    }

    #[test]
    fn owned_language_config_clone() {
        let config = OwnedLanguageConfig {
            name: "Test".to_string(),
            extensions: vec!["test".to_string()],
            features: LanguageFeatures::none(),
        };

        let cloned = config.clone();
        assert_eq!(config, cloned);
    }

    #[test]
    fn owned_to_language_config_conversion() {
        let owned = OwnedLanguageConfig {
            name: "Test".to_string(),
            extensions: vec!["test".to_string()],
            features: LanguageFeatures {
                extract_exports: true,
                extract_imports: false,
                count_todos: true,
                count_lines: false,
            },
        };

        let converted: LanguageConfig = owned.into();
        assert_eq!(converted.name, "owned");
        assert!(converted.extensions.is_empty());
        assert!(converted.features.extract_exports);
        assert!(!converted.features.extract_imports);
    }
}
