// Builder API for AmbientFsClient

use crate::{client::AmbientFsClient, ClientError, Result};
use std::path::PathBuf;
use std::time::Duration;

#[cfg(unix)]
use crate::DEFAULT_SOCKET_PATH;
#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(windows)]
use crate::DEFAULT_ADDR;
#[cfg(windows)]
use tokio::net::TcpStream;

/// Default notification channel buffer size
const DEFAULT_NOTIFICATION_BUFFER: usize = 256;

/// Fluent builder for constructing AmbientFsClient
#[derive(Debug, Clone)]
pub struct AmbientFsClientBuilder {
    socket_path: PathBuf,
    connect_timeout: Option<Duration>,
    notification_buffer_size: usize,
}

impl Default for AmbientFsClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AmbientFsClientBuilder {
    /// Create a new builder with default settings
    pub fn new() -> Self {
        #[cfg(unix)]
        let socket_path = PathBuf::from(DEFAULT_SOCKET_PATH);
        #[cfg(windows)]
        let socket_path = PathBuf::from(DEFAULT_ADDR);

        Self {
            socket_path,
            connect_timeout: None,
            notification_buffer_size: DEFAULT_NOTIFICATION_BUFFER,
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

    /// Set the notification channel buffer size (default: 256).
    ///
    /// Controls how many server-pushed notifications can be buffered before
    /// new ones are dropped. Increase if your application processes
    /// notifications slower than they arrive.
    pub fn notification_buffer_size(mut self, size: usize) -> Self {
        self.notification_buffer_size = size;
        self
    }

    /// Build and connect the client.
    ///
    /// On Unix, connects via Unix socket. On Windows, connects via TCP.
    pub async fn build(self) -> Result<AmbientFsClient> {
        #[cfg(unix)]
        let connect_fut = UnixStream::connect(&self.socket_path);
        #[cfg(windows)]
        let addr = self.socket_path.to_string_lossy().into_owned();
        #[cfg(windows)]
        let connect_fut = TcpStream::connect(&*addr);

        let stream = if let Some(timeout) = self.connect_timeout {
            tokio::time::timeout(timeout, connect_fut)
                .await
                .map_err(|_| ClientError::DaemonError("connection timeout".to_string()))?
        } else {
            connect_fut.await
        }?;
        Ok(AmbientFsClient::from_stream(
            stream,
            self.socket_path,
            self.notification_buffer_size,
        ))
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
    fn builder_default_notification_buffer() {
        let builder = AmbientFsClientBuilder::new();
        assert_eq!(builder.notification_buffer_size, 256);
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
    fn builder_notification_buffer_size_override() {
        let builder = AmbientFsClientBuilder::new()
            .notification_buffer_size(1024);
        assert_eq!(builder.notification_buffer_size, 1024);
    }

    #[test]
    fn builder_chaining_works() {
        let builder = AmbientFsClientBuilder::new()
            .socket_path("/tmp/test.sock")
            .connect_timeout(Duration::from_secs(5))
            .notification_buffer_size(512);
        assert_eq!(builder.socket_path, PathBuf::from("/tmp/test.sock"));
        assert_eq!(builder.connect_timeout, Some(Duration::from_secs(5)));
        assert_eq!(builder.notification_buffer_size, 512);
    }

    #[test]
    fn builder_default_trait() {
        let builder = AmbientFsClientBuilder::default();
        assert_eq!(builder.socket_path, PathBuf::from(DEFAULT_SOCKET_PATH));
        assert!(builder.connect_timeout.is_none());
        assert_eq!(builder.notification_buffer_size, 256);
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
