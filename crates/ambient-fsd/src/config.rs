//! Daemon config persistence to toml.
//! TDD: tests first, then implementation.

use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use anyhow::Result;

/// Daemon configuration persisted to toml file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Unique machine identifier (uuid v4)
    pub machine_id: String,
    /// Path to the sqlite database
    pub db_path: PathBuf,
    /// Path to the unix socket
    pub socket_path: PathBuf,
    /// Debounce delay in milliseconds
    pub debounce_ms: u64,
    /// Max file size for content analysis in bytes
    pub max_file_size_bytes: u64,
    /// Log level (trace, debug, info, warn, error)
    pub log_level: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            machine_id: uuid::Uuid::new_v4().to_string(),
            db_path: default_db_path(),
            socket_path: PathBuf::from("/tmp/ambient-fs.sock"),
            debounce_ms: 100,
            max_file_size_bytes: 10 * 1024 * 1024, // 10MB
            log_level: "info".to_string(),
        }
    }
}

/// Get default db path: ~/.local/share/ambient-fs/events.db
fn default_db_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("ambient-fs")
        .join("events.db")
}

/// Get config path: ~/.config/ambient-fs/config.toml
/// or $AMBIENT_FS_CONFIG env var override.
pub fn config_path() -> PathBuf {
    if let Ok(custom) = std::env::var("AMBIENT_FS_CONFIG") {
        return PathBuf::from(custom);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("ambient-fs")
        .join("config.toml")
}

/// Load config from file, or create default if missing.
pub fn load() -> Result<DaemonConfig> {
    let path = config_path();

    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        let config: DaemonConfig = toml::from_str(&content)?;
        Ok(config)
    } else {
        // Create default config with new uuid
        let config = DaemonConfig::default();
        config.save()?;
        Ok(config)
    }
}

impl DaemonConfig {
    /// Save config to toml file.
    pub fn save(&self) -> Result<()> {
        let path = config_path();

        // Create parent dir if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }
}

// Convert DaemonConfig to ServerConfig (from server module)
pub fn to_server_config(config: &DaemonConfig) -> crate::server::ServerConfig {
    crate::server::ServerConfig {
        db_path: config.db_path.clone(),
        socket_path: config.socket_path.clone(),
        debounce_ms: config.debounce_ms,
        max_file_size_bytes: config.max_file_size_bytes,
        machine_id: config.machine_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::sync::Mutex;

    // Mutex to serialize env var access in tests
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    // ========== default_db_path ==========

    #[test]
    fn default_db_path_returns_ambient_fs_subdir() {
        let path = default_db_path();
        // Should end with ambient-fs/events.db
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("ambient-fs"));
        assert!(path_str.ends_with("events.db"));
    }

    // ========== config_path ==========

    #[test]
    fn config_path_default_ends_with_config_toml() {
        let _lock = ENV_MUTEX.lock().unwrap();
        // Clear env var for default behavior
        std::env::remove_var("AMBIENT_FS_CONFIG");
        let path = config_path();
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("ambient-fs"), "path should contain ambient-fs: {}", path_str);
        assert!(path_str.ends_with("config.toml"), "path should end with config.toml: {}", path_str);
    }

    #[test]
    fn config_path_env_override() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("AMBIENT_FS_CONFIG", "/custom/path/config.toml");
        let path = config_path();
        assert_eq!(path, PathBuf::from("/custom/path/config.toml"));
        std::env::remove_var("AMBIENT_FS_CONFIG");
    }

    // ========== DaemonConfig::default ==========

    #[test]
    fn default_generates_uuid_machine_id() {
        let config = DaemonConfig::default();
        // Should parse as uuid
        let uuid = uuid::Uuid::parse_str(&config.machine_id);
        assert!(uuid.is_ok());
    }

    #[test]
    fn default_machine_id_is_unique() {
        let c1 = DaemonConfig::default();
        let c2 = DaemonConfig::default();
        assert_ne!(c1.machine_id, c2.machine_id);
    }

    #[test]
    fn default_has_expected_db_path() {
        let config = DaemonConfig::default();
        let path_str = config.db_path.to_string_lossy();
        assert!(path_str.contains("ambient-fs"));
        assert!(path_str.ends_with("events.db"));
    }

    #[test]
    fn default_has_expected_socket_path() {
        let config = DaemonConfig::default();
        assert_eq!(config.socket_path, PathBuf::from("/tmp/ambient-fs.sock"));
    }

    #[test]
    fn default_debounce_is_100ms() {
        let config = DaemonConfig::default();
        assert_eq!(config.debounce_ms, 100);
    }

    #[test]
    fn default_max_file_size_is_10mb() {
        let config = DaemonConfig::default();
        assert_eq!(config.max_file_size_bytes, 10 * 1024 * 1024);
    }

    #[test]
    fn default_log_level_is_info() {
        let config = DaemonConfig::default();
        assert_eq!(config.log_level, "info");
    }

    // ========== DaemonConfig::save ==========

    #[test]
    fn save_creates_config_file() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        // Override config path for test
        std::env::set_var("AMBIENT_FS_CONFIG", config_path.to_str().unwrap());

        let config = DaemonConfig {
            machine_id: "test-machine".to_string(),
            db_path: PathBuf::from("/tmp/test.db"),
            socket_path: PathBuf::from("/tmp/test.sock"),
            debounce_ms: 50,
            max_file_size_bytes: 12345,
            log_level: "debug".to_string(),
        };

        config.save().unwrap();

        assert!(config_path.exists());

        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("test-machine"));
        assert!(content.contains("debug"));

        std::env::remove_var("AMBIENT_FS_CONFIG");
    }

    #[test]
    fn save_creates_parent_dir() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let nested_dir = temp_dir.path().join("nested").join("dir");
        let config_path = nested_dir.join("config.toml");

        std::env::set_var("AMBIENT_FS_CONFIG", config_path.to_str().unwrap());

        let config = DaemonConfig::default();
        config.save().unwrap();

        assert!(nested_dir.exists());
        assert!(config_path.exists());

        std::env::remove_var("AMBIENT_FS_CONFIG");
    }

    #[test]
    fn save_writes_valid_toml() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::env::set_var("AMBIENT_FS_CONFIG", config_path.to_str().unwrap());

        let config = DaemonConfig::default();
        config.save().unwrap();

        let content = std::fs::read_to_string(&config_path).unwrap();
        let parsed: DaemonConfig = toml::from_str(&content).unwrap();

        assert_eq!(parsed.machine_id, config.machine_id);
        assert_eq!(parsed.debounce_ms, config.debounce_ms);

        std::env::remove_var("AMBIENT_FS_CONFIG");
    }

    // ========== load ==========

    #[test]
    fn load_creates_default_when_missing() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::env::set_var("AMBIENT_FS_CONFIG", config_path.to_str().unwrap());

        // Ensure file doesn't exist
        assert!(!config_path.exists());

        let config = load().unwrap();

        // Should create file
        assert!(config_path.exists());

        // Should have valid uuid
        let uuid = uuid::Uuid::parse_str(&config.machine_id);
        assert!(uuid.is_ok());

        std::env::remove_var("AMBIENT_FS_CONFIG");
    }

    #[test]
    fn load_reads_existing_config() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::env::set_var("AMBIENT_FS_CONFIG", config_path.to_str().unwrap());

        let original = DaemonConfig {
            machine_id: "existing-uuid-123".to_string(),
            db_path: PathBuf::from("/custom/db.db"),
            socket_path: PathBuf::from("/custom/sock"),
            debounce_ms: 200,
            max_file_size_bytes: 9999,
            log_level: "warn".to_string(),
        };
        original.save().unwrap();

        let loaded = load().unwrap();

        assert_eq!(loaded.machine_id, "existing-uuid-123");
        assert_eq!(loaded.debounce_ms, 200);
        assert_eq!(loaded.max_file_size_bytes, 9999);
        assert_eq!(loaded.log_level, "warn");

        std::env::remove_var("AMBIENT_FS_CONFIG");
    }

    #[test]
    fn load_preserves_machine_id() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::env::set_var("AMBIENT_FS_CONFIG", config_path.to_str().unwrap());

        let first = load().unwrap();
        let saved_id = first.machine_id.clone();

        let second = load().unwrap();

        assert_eq!(second.machine_id, saved_id);

        std::env::remove_var("AMBIENT_FS_CONFIG");
    }

    // ========== to_server_config ==========

    #[test]
    fn to_server_config_maps_fields() {
        let daemon_config = DaemonConfig {
            machine_id: "test-machine".to_string(),
            db_path: PathBuf::from("/tmp/test.db"),
            socket_path: PathBuf::from("/tmp/test.sock"),
            debounce_ms: 250,
            max_file_size_bytes: 5000,
            log_level: "trace".to_string(),
        };

        let server_config = to_server_config(&daemon_config);

        assert_eq!(server_config.machine_id, "test-machine");
        assert_eq!(server_config.db_path, PathBuf::from("/tmp/test.db"));
        assert_eq!(server_config.socket_path, PathBuf::from("/tmp/test.sock"));
        assert_eq!(server_config.debounce_ms, 250);
        assert_eq!(server_config.max_file_size_bytes, 5000);
    }
}
