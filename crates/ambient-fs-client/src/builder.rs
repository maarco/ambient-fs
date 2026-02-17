// Builder API for AmbientFsClient

use crate::{client::AmbientFsClient, ClientError, DEFAULT_SOCKET_PATH, Result};
use std::path::PathBuf;
use std::time::Duration;

/// Fluent builder for constructing AmbientFsClient
#[derive(Debug, Clone)]
pub struct AmbientFsClientBuilder {
    socket_path: PathBuf,
    connect_timeout: Option<Duration>,
}

impl Default for AmbientFsClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AmbientFsClientBuilder {
    /// Create a new builder with default settings
    pub fn new() -> Self {
        Self {
            socket_path: PathBuf::from(DEFAULT_SOCKET_PATH),
            connect_timeout: None,
        }
    }

    /// Set the socket path for connecting to the daemon
    pub fn socket_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.socket_path = path.into();
        self
    }

    /// Set the connection timeout
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self
    }

    /// Build and connect the client
    pub async fn build(self) -> Result<AmbientFsClient> {
        if let Some(timeout) = self.connect_timeout {
            tokio::time::timeout(
                timeout,
                AmbientFsClient::connect(self.socket_path),
            )
            .await
            .map_err(|_| ClientError::DaemonError("connection timeout".to_string()))?
        } else {
            Ok(AmbientFsClient::connect(self.socket_path).await?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_default_has_correct_socket_path() {
        let builder = AmbientFsClientBuilder::new();
        assert_eq!(builder.socket_path, PathBuf::from(DEFAULT_SOCKET_PATH));
    }

    #[test]
    fn builder_default_has_no_timeout() {
        let builder = AmbientFsClientBuilder::new();
        assert!(builder.connect_timeout.is_none());
    }

    #[test]
    fn builder_socket_path_override() {
        let builder = AmbientFsClientBuilder::new()
            .socket_path("/custom/path.sock");
        assert_eq!(builder.socket_path, PathBuf::from("/custom/path.sock"));
    }

    #[test]
    fn builder_connect_timeout_override() {
        let builder = AmbientFsClientBuilder::new()
            .connect_timeout(Duration::from_secs(10));
        assert_eq!(builder.connect_timeout, Some(Duration::from_secs(10)));
    }

    #[test]
    fn builder_chaining_works() {
        let builder = AmbientFsClientBuilder::new()
            .socket_path("/tmp/test.sock")
            .connect_timeout(Duration::from_secs(5));
        assert_eq!(builder.socket_path, PathBuf::from("/tmp/test.sock"));
        assert_eq!(builder.connect_timeout, Some(Duration::from_secs(5)));
    }

    #[test]
    fn builder_default_trait() {
        let builder = AmbientFsClientBuilder::default();
        assert_eq!(builder.socket_path, PathBuf::from(DEFAULT_SOCKET_PATH));
        assert!(builder.connect_timeout.is_none());
    }

    #[test]
    fn builder_path_buf_acceptance() {
        // Should accept PathBuf, &str, String
        let path = PathBuf::from("/from/pathbuf.sock");
        let builder1 = AmbientFsClientBuilder::new().socket_path(&path);
        let builder2 = AmbientFsClientBuilder::new().socket_path("/from/str.sock");
        let builder3 = AmbientFsClientBuilder::new().socket_path(String::from("/from/string.sock"));
        assert_eq!(builder1.socket_path, PathBuf::from("/from/pathbuf.sock"));
        assert_eq!(builder2.socket_path, PathBuf::from("/from/str.sock"));
        assert_eq!(builder3.socket_path, PathBuf::from("/from/string.sock"));
    }
}
