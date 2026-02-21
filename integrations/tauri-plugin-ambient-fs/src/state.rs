// managed plugin state: holds client connection, watches projects,
// forwards daemon events to tauri frontend

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, async_runtime, Emitter};
use tokio::sync::Mutex as TokioMutex;
use ambient_fs_client::{AmbientFsClient, Notification};
use thiserror::Error;

/// errors from plugin state operations
#[derive(Debug, Error)]
pub enum StateError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("client error: {0}")]
    Client(#[from] ambient_fs_client::ClientError),
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
    /// projects we've subscribed to
    subscribed_projects: Arc<TokioMutex<Vec<String>>>,
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
            subscribed_projects: Arc::new(TokioMutex::new(Vec::new())),
        };

        // spawn connection task using tauri's async runtime
        let state_clone = state.clone_for_task();
        async_runtime::spawn(async move {
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

                    // resubscribe to previously watched projects
                    self.resubscribe_all().await;

                    // run notification loop
                    self.run_notification_loop().await;

                    // loop ended = connection lost
                    self.connected.store(false, Ordering::SeqCst);
                    crate::events::emit_connected(&self.app_handle, false);
                }
                Err(e) => {
                    tracing::debug!("connection failed: {}", e);
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
        tracing::info!("connected to ambient-fs daemon at {}", self.socket_path);
        Ok(())
    }

    /// resubscribe to all previously subscribed projects after reconnect
    async fn resubscribe_all(&self) {
        let projects = self.subscribed_projects.lock().await.clone();
        if projects.is_empty() {
            return;
        }

        let mut client = self.client.lock().await;
        if let Some(ref mut c) = *client {
            for project_id in projects {
                match c.subscribe(&project_id).await {
                    Ok(_) => tracing::debug!("resubscribed to {}", project_id),
                    Err(e) => tracing::warn!("failed to resubscribe to {}: {}", project_id, e),
                }
            }
        }
    }

    /// attempt to spawn the ambient-fsd daemon (bundled sidecar)
    #[cfg(feature = "auto-launch")]
    async fn try_launch_daemon(&self) {
        use tauri::Manager;

        // Get resource directory path
        let resource_dir = match self.app_handle.path().resource_dir() {
            Ok(dir) => dir,
            Err(e) => {
                tracing::error!("Failed to get resource dir: {}", e);
                return;
            }
        };

        // Path to bundled ambient-fsd binary
        #[cfg(target_os = "macos")]
        let binary_name = "ambient-fsd-aarch64-apple-darwin";
        #[cfg(target_os = "linux")]
        let binary_name = "ambient-fsd-x86_64-unknown-linux-gnu";
        #[cfg(target_os = "windows")]
        let binary_name = "ambient-fsd-x86_64-pc-windows-msvc.exe";

        let binary_path = resource_dir.join("binaries").join(binary_name);

        if !binary_path.exists() {
            tracing::error!("ambient-fsd binary not found at {:?}", binary_path);
            return;
        }

        match tokio::process::Command::new(&binary_path)
            .arg("daemon")
            .spawn()
        {
            Ok(child) => {
                tracing::info!("ambient-fsd spawned (pid: {:?})", child.id());
            }
            Err(e) => {
                tracing::error!("Failed to spawn ambient-fsd: {}", e);
            }
        }
    }

    #[cfg(not(feature = "auto-launch"))]
    async fn try_launch_daemon(&self) {
        // auto-launch disabled, do nothing
    }

    /// main notification loop: receive pushed events from daemon, emit to tauri
    async fn run_notification_loop(&self) {
        loop {
            let notification = {
                let mut client = self.client.lock().await;
                match &mut *client {
                    Some(c) => match c.recv_notification().await {
                        Ok(Some(n)) => Some(n),
                        Ok(None) => {
                            // connection closed
                            tracing::info!("daemon closed connection");
                            *self.client.lock().await = None;
                            return;
                        }
                        Err(e) => {
                            tracing::error!("error receiving notification: {}", e);
                            // check if still connected
                            if !c.is_connected() {
                                *self.client.lock().await = None;
                                return;
                            }
                            continue;
                        }
                    },
                    None => return,
                }
            };

            if let Some(notif) = notification {
                self.handle_notification(notif).await;
            }
        }
    }

    /// handle a notification from the daemon
    async fn handle_notification(&self, notif: Notification) {
        match notif {
            Notification::Event { params: event } => {
                tracing::debug!("file event: {:?} on {}", event.event_type, event.file_path);
                crate::events::emit_file_event(&self.app_handle, event);
            }
            Notification::AwarenessChanged { params } => {
                tracing::debug!("awareness changed: {} in {}", params.file_path, params.project_id);
                crate::events::emit_awareness_changed(
                    &self.app_handle,
                    params.project_id,
                    params.file_path,
                    params.awareness,
                );
            }
            Notification::AnalysisComplete { params } => {
                tracing::debug!("analysis complete: {} ({} lines, {} todos)",
                    params.file_path, params.line_count, params.todo_count);
                // emit raw params as JSON - frontend can handle the structure
                let _ = self.app_handle.emit("ambient-fs://analysis-complete", &params);
            }
            Notification::TreePatch { params } => {
                tracing::debug!("tree patch for {}", params.project_id);
                let _ = self.app_handle.emit("ambient-fs://tree-patch", &params);
            }
        }
    }

    /// subscribe to a project's notifications
    pub async fn subscribe(&self, project_id: &str) -> Result<()> {
        // track for reconnection
        self.subscribed_projects.lock().await.push(project_id.to_string());

        let mut client = self.client.lock().await;
        if let Some(ref mut c) = *client {
            c.subscribe(project_id).await?;
            tracing::info!("subscribed to project: {}", project_id);
        }
        Ok(())
    }

    /// unsubscribe from a project's notifications
    pub async fn unsubscribe(&self, project_id: &str) -> Result<()> {
        // remove from tracked list
        let mut projects = self.subscribed_projects.lock().await;
        projects.retain(|p| p != project_id);

        let mut client = self.client.lock().await;
        if let Some(ref mut c) = *client {
            c.unsubscribe(project_id).await?;
            tracing::info!("unsubscribed from project: {}", project_id);
        }
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
            subscribed_projects: Arc::clone(&self.subscribed_projects),
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
