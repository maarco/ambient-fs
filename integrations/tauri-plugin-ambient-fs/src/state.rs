// managed plugin state: holds client connection, watches projects,
// forwards daemon events to tauri frontend

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::Mutex as TokioMutex;
use ambient_fs_client::AmbientFsClient;
use thiserror::Error;

/// errors from plugin state operations
#[derive(Debug, Error)]
pub enum StateError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("client error: {0}")]
    Client(#[from] ambient_fs_client::ClientError),

    #[error("not connected to daemon")]
    NotConnected,
}

pub type Result<T> = std::result::Result<T, StateError>;

/// managed plugin state
///
/// holds the ambient-fs client connection, tracks connection status,
/// and spawns the connection loop task.
pub struct PluginState {
    /// client is pub(crate) so commands can access it
    pub(crate) client: Arc<TokioMutex<Option<AmbientFsClient>>>,
    connected: Arc<AtomicBool>,
    socket_path: String,
    app_handle: AppHandle,
    auto_launch: bool,
}

impl PluginState {
    /// create new plugin state and spawn connection loop
    pub fn new(app_handle: AppHandle) -> Self {
        let socket_path = std::env::var("AMBIENT_FS_SOCKET")
            .unwrap_or_else(|_| "/tmp/ambient-fs.sock".to_string());
        let auto_launch = cfg!(feature = "auto-launch");

        let state = Self {
            client: Arc::new(TokioMutex::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
            socket_path,
            app_handle,
            auto_launch,
        };

        // spawn connection task
        let state_clone = state.clone_for_task();
        tokio::spawn(async move {
            state_clone.connection_loop().await;
        });

        state
    }

    /// main connection loop: retry until connected
    async fn connection_loop(&self) {
        loop {
            match self.try_connect().await {
                Ok(_) => {
                    self.connected.store(true, Ordering::SeqCst);
                    crate::events::emit_connected(&self.app_handle, true);
                    self.run_event_loop().await;
                    // event loop ended = connection lost
                    self.connected.store(false, Ordering::SeqCst);
                    crate::events::emit_connected(&self.app_handle, false);
                }
                Err(_) => {
                    self.connected.store(false, Ordering::SeqCst);
                    if self.auto_launch {
                        self.try_launch_daemon().await;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// attempt to connect to the daemon
    async fn try_connect(&self) -> Result<()> {
        let client = AmbientFsClient::connect(&self.socket_path).await?;
        *self.client.lock().await = Some(client);
        Ok(())
    }

    /// attempt to spawn the ambient-fsd daemon
    #[cfg(feature = "auto-launch")]
    async fn try_launch_daemon(&self) {
        let _ = tokio::process::Command::new("ambient-fsd")
            .arg("daemon")
            .spawn();
    }

    #[cfg(not(feature = "auto-launch"))]
    async fn try_launch_daemon(&self) {
        // auto-launch disabled, do nothing
    }

    /// main event loop: receive notifications from daemon, emit to tauri
    ///
    /// note: the current client doesn't support notification streaming.
    /// this is a placeholder that polls for awareness updates.
    async fn run_event_loop(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(_) = self.poll_awareness_updates().await {
                break; // connection lost
            }
        }
    }

    /// poll for awareness updates (simplified version)
    async fn poll_awareness_updates(&self) -> Result<()> {
        // query awareness for files in watched projects
        // emit events if changed
        // real implementation would use daemon push notifications
        Ok(())
    }

    /// check if currently connected to the daemon
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// clone for task spawning (arc inner fields)
    fn clone_for_task(&self) -> Self {
        Self {
            client: Arc::clone(&self.client),
            connected: Arc::clone(&self.connected),
            socket_path: self.socket_path.clone(),
            app_handle: self.app_handle.clone(),
            auto_launch: self.auto_launch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_error_display() {
        let err = StateError::NotConnected;
        assert_eq!(err.to_string(), "not connected to daemon");
    }

    #[test]
    fn atomic_connected_default() {
        let connected = Arc::new(AtomicBool::new(false));
        assert!(!connected.load(Ordering::SeqCst));
        connected.store(true, Ordering::SeqCst);
        assert!(connected.load(Ordering::SeqCst));
    }
}
