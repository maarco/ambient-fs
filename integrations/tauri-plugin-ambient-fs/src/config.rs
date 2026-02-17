// plugin configuration options

/// plugin configuration for ambient-fs
#[derive(Debug, Clone)]
pub struct AmbientFsConfig {
    /// path to ambient-fsd socket (default: /tmp/ambient-fs.sock or AMBIENT_FS_SOCKET env)
    pub socket_path: Option<String>,
    /// whether to auto-launch the daemon if not running
    pub auto_launch: bool,
    /// connection timeout in seconds
    pub connect_timeout_secs: u64,
}

impl Default for AmbientFsConfig {
    fn default() -> Self {
        Self {
            socket_path: None,
            auto_launch: true,
            connect_timeout_secs: 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = AmbientFsConfig::default();
        assert!(config.socket_path.is_none());
        assert!(config.auto_launch);
        assert_eq!(config.connect_timeout_secs, 5);
    }

    #[test]
    fn config_clone() {
        let config = AmbientFsConfig {
            socket_path: Some("/tmp/test.sock".to_string()),
            auto_launch: false,
            connect_timeout_secs: 10,
        };
        let cloned = config.clone();
        assert_eq!(cloned.socket_path, config.socket_path);
        assert_eq!(cloned.auto_launch, config.auto_launch);
        assert_eq!(cloned.connect_timeout_secs, config.connect_timeout_secs);
    }
}
