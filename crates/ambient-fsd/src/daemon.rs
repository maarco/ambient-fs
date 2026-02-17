//! Daemon process management.
//!
//! Handles daemonization: PID file management, signal handling,
//! background forking, and stdio redirection.

use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use thiserror::Error;

/// Default PID file path.
pub const DEFAULT_PID_FILE: &str = "/tmp/ambient-fs.pid";

/// Default log file path.
pub const DEFAULT_LOG_FILE: &str = "/tmp/ambient-fs.log";

/// Errors from daemon operations.
#[derive(Error, Debug)]
pub enum DaemonError {
    #[error("daemon already running (PID {0})")]
    AlreadyRunning(u32),

    #[error("daemon not running")]
    NotRunning,

    #[error("failed to read PID file: {0}")]
    PidFileRead(io::Error),

    #[error("invalid PID format: {0}")]
    InvalidPid(String),

    #[error("failed to create directory: {0}")]
    DirCreate(io::Error),

    #[error("io error: {0}")]
    Io(io::Error),
}

/// Result type for daemon operations.
pub type Result<T> = std::result::Result<T, DaemonError>;

/// PID file manager.
///
/// Handles creating, reading, and removing the PID file.
/// Validates that the recorded process is actually running.
#[derive(Debug, Clone)]
pub struct PidFile {
    path: &'static str,
}

impl PidFile {
    /// Create a new PID file manager.
    pub const fn new(path: &'static str) -> Self {
        Self { path }
    }

    /// Check if daemon is running by reading PID file.
    pub fn is_running(&self) -> Result<bool> {
        match self.read_pid() {
            Ok(pid) => self.check_process(pid),
            Err(DaemonError::PidFileRead(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Get the PID of the running daemon.
    pub fn get_pid(&self) -> Result<u32> {
        let pid = self.read_pid()?;
        if self.check_process(pid)? {
            Ok(pid)
        } else {
            Err(DaemonError::NotRunning)
        }
    }

    /// Write current PID to file.
    ///
    /// Returns error if daemon is already running.
    pub fn create(&self) -> Result<()> {
        // Check if already running
        if let Ok(true) = self.is_running() {
            if let Ok(pid) = self.read_pid() {
                return Err(DaemonError::AlreadyRunning(pid));
            }
        }

        // Ensure parent dir exists
        if let Some(parent) = Path::new(self.path).parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent).map_err(DaemonError::DirCreate)?;
            }
        }

        let pid = std::process::id();
        fs::write(self.path, format!("{pid}\n")).map_err(DaemonError::Io)?;
        Ok(())
    }

    /// Remove PID file.
    pub fn remove(&self) -> Result<()> {
        if Path::new(self.path).exists() {
            fs::remove_file(self.path).map_err(DaemonError::Io)?;
        }
        Ok(())
    }

    /// Read PID from file.
    fn read_pid(&self) -> Result<u32> {
        let content = fs::read_to_string(self.path).map_err(DaemonError::PidFileRead)?;
        let pid_str = content.trim();
        pid_str
            .parse::<u32>()
            .map_err(|_| DaemonError::InvalidPid(pid_str.to_string()))
    }

    /// Check if a process with given PID is running.
    fn check_process(&self, pid: u32) -> Result<bool> {
        // Send signal 0 to check if process exists
        // On Unix, this doesn't actually send a signal but checks validity
        #[cfg(unix)]
        {
            use nix::unistd::Pid;
            use nix::sys::signal;
            match signal::kill(Pid::from_raw(pid as i32), None) {
                Ok(_) => Ok(true),
                Err(nix::errno::Errno::ESRCH) => Ok(false),
                Err(_) => Ok(true), // Other errors typically mean process exists
            }
        }

        #[cfg(not(unix))]
        {
            // On non-Unix, assume running if PID file exists
            // (not ideal, but this is a Unix daemon primarily)
            Ok(true)
        }
    }
}

/// Daemon process manager.
///
/// Handles the full daemon lifecycle: start, stop, status.
/// Manages PID file, signal handling, and graceful shutdown.
#[derive(Debug)]
pub struct Daemon {
    pid_file: PidFile,
    log_file: &'static str,
    shutdown: Arc<AtomicBool>,
    foreground: bool,
}

impl Daemon {
    /// Create a new daemon instance.
    pub fn new() -> Self {
        Self {
            pid_file: PidFile::new(DEFAULT_PID_FILE),
            log_file: DEFAULT_LOG_FILE,
            shutdown: Arc::new(AtomicBool::new(false)),
            foreground: false,
        }
    }

    /// Set foreground mode (skip stdio redirection and forking).
    pub fn with_foreground(mut self, foreground: bool) -> Self {
        self.foreground = foreground;
        self
    }

    /// Set custom PID file path.
    #[allow(dead_code)]
    pub fn with_pid_file(mut self, path: &'static str) -> Self {
        self.pid_file = PidFile::new(path);
        self
    }

    /// Set custom log file path.
    #[allow(dead_code)]
    pub fn with_log_file(mut self, path: &'static str) -> Self {
        self.log_file = path;
        self
    }

    /// Create PID file (public for use in main.rs)
    #[allow(dead_code)]
    pub fn create_pid_file(&self) -> Result<()> {
        self.pid_file.create()
    }

    /// Remove PID file (public for use in main.rs)
    pub fn remove_pid_file(&self) -> Result<()> {
        self.pid_file.remove()
    }

    /// Start the daemon.
    ///
    /// Creates PID file, forks to background, redirects stdio.
    /// Returns error if already running.
    pub fn start(&self) -> Result<()> {
        // Check if already running
        if self.pid_file.is_running()? {
            let pid = self.pid_file.read_pid()?;
            return Err(DaemonError::AlreadyRunning(pid));
        }

        // Create PID file before fork
        self.pid_file.create()?;

        // Fork to background (skip in foreground mode)
        if !self.foreground {
            self.fork()?;
        }

        // Setup signal handlers
        self.setup_signals();

        // Redirect stdout/stderr to log (skip in foreground mode)
        if !self.foreground {
            self.redirect_stdio()?;
        }

        Ok(())
    }

    /// Stop the daemon.
    ///
    /// Sends SIGTERM to running daemon, waits for graceful shutdown.
    pub fn stop(&self) -> Result<()> {
        let pid = self.pid_file.get_pid()?;

        #[cfg(unix)]
        {
            use nix::unistd::Pid;
            use nix::sys::signal;
            signal::kill(Pid::from_raw(pid as i32), signal::Signal::SIGTERM)
                .map_err(|e| DaemonError::Io(io::Error::new(io::ErrorKind::Other, e)))?;
        }

        #[cfg(not(unix))]
        {
            let _ = pid;
            return Err(DaemonError::Io(io::Error::new(
                io::ErrorKind::Unsupported,
                "daemon stop only supported on Unix",
            )));
        }

        Ok(())
    }

    /// Get daemon status.
    ///
    /// Returns None if not running, Some(pid) if running.
    pub fn status(&self) -> Result<Option<u32>> {
        match self.pid_file.get_pid() {
            Ok(pid) => Ok(Some(pid)),
            Err(DaemonError::NotRunning) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Check if shutdown was requested.
    pub fn is_shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    /// Get the shutdown flag for sharing with other components.
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    /// Check if running in foreground mode.
    pub fn is_foreground(&self) -> bool {
        self.foreground
    }

    /// Fork process to background.
    fn fork(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use nix::unistd::{fork, ForkResult};
            match unsafe { fork() } {
                Ok(ForkResult::Parent { .. }) => {
                    // Parent exits after fork succeeds
                    exit(0);
                }
                Ok(ForkResult::Child) => {
                    // Child continues
                    Ok(())
                }
                Err(e) => Err(DaemonError::Io(io::Error::new(io::ErrorKind::Other, e))),
            }
        }

        #[cfg(not(unix))]
        {
            let _ = self;
            Err(DaemonError::Io(io::Error::new(
                io::ErrorKind::Unsupported,
                "daemonization only supported on Unix",
            )))
        }
    }

    /// Setup signal handlers for graceful shutdown.
    fn setup_signals(&self) {
        #[cfg(unix)]
        {
            use signal_hook::consts::SIGTERM;
            use signal_hook::iterator::Signals;
            use std::thread;

            let shutdown = self.shutdown.clone();

            thread::spawn(move || {
                let mut signals = Signals::new([SIGTERM]).expect("failed to create signal handler");
                for sig in signals.forever() {
                    if sig == SIGTERM {
                        shutdown.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            });
        }

        #[cfg(not(unix))]
        {
            let _ = self;
        }
    }

    /// Redirect stdout and stderr to log file.
    fn redirect_stdio(&self) -> Result<()> {
        // Ensure log directory exists
        if let Some(parent) = Path::new(self.log_file).parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent).map_err(DaemonError::DirCreate)?;
            }
        }

        // Open log file for append
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_file)
            .map_err(DaemonError::Io)?;

        #[cfg(unix)]
        {
            use nix::unistd::dup2;
            use nix::fcntl::open;
            use nix::fcntl::OFlag;

            let log_fd = log.as_raw_fd();

            // Redirect stdout to log file
            dup2(log_fd, 1)
                .map_err(|e| DaemonError::Io(io::Error::new(io::ErrorKind::Other, e)))?;

            // Redirect stderr to log file
            dup2(log_fd, 2)
                .map_err(|e| DaemonError::Io(io::Error::new(io::ErrorKind::Other, e)))?;

            // Redirect stdin from /dev/null
            let dev_null = open("/dev/null", OFlag::O_RDONLY, nix::sys::stat::Mode::empty())
                .map_err(|e| DaemonError::Io(io::Error::new(io::ErrorKind::Other, e)))?;
            dup2(dev_null, 0)
                .map_err(|e| DaemonError::Io(io::Error::new(io::ErrorKind::Other, e)))?;
        }

        // On non-Unix, just drop the log file (nothing to redirect)
        let _ = log;
        Ok(())
    }
}

impl Default for Daemon {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test PID file paths - unique per test.
    const TEST_PID_1: &str = "/tmp/ambient-fs-test-1.pid";
    const TEST_PID_2: &str = "/tmp/ambient-fs-test-2.pid";
    const TEST_PID_3: &str = "/tmp/ambient-fs-test-3.pid";
    const TEST_PID_4: &str = "/tmp/ambient-fs-test-4.pid";
    const TEST_PID_5: &str = "/tmp/ambient-fs-test-5.pid";
    const TEST_PID_6: &str = "/tmp/ambient-fs-test-6.pid";
    const TEST_PID_7: &str = "/tmp/ambient-fs-test-7.pid";

    #[test]
    fn test_pid_file_not_running_when_missing() {
        let pid_file = PidFile::new(TEST_PID_1);
        let _ = pid_file.remove();

        let result = pid_file.is_running();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[test]
    fn test_pid_file_create_and_read() {
        let pid_file = PidFile::new(TEST_PID_2);
        let _ = pid_file.remove();

        pid_file.create().unwrap();
        let content = fs::read_to_string(TEST_PID_2).unwrap();
        assert_eq!(content.trim(), std::process::id().to_string());

        pid_file.remove().unwrap();
    }

    #[test]
    fn test_pid_file_remove() {
        let pid_file = PidFile::new(TEST_PID_3);
        let _ = pid_file.remove();

        pid_file.create().unwrap();
        assert!(Path::new(TEST_PID_3).exists());
        pid_file.remove().unwrap();
        assert!(!Path::new(TEST_PID_3).exists());
    }

    #[test]
    fn test_pid_file_invalid_format() {
        let pid_file = PidFile::new(TEST_PID_4);
        let _ = pid_file.remove();

        fs::write(TEST_PID_4, "not-a-pid\n").unwrap();

        let result = pid_file.read_pid();
        assert!(result.is_err());
        assert!(matches!(result, Err(DaemonError::InvalidPid(_))));

        let _ = fs::remove_file(TEST_PID_4);
    }

    #[test]
    fn test_daemon_new() {
        let daemon = Daemon::new();
        assert_eq!(daemon.log_file, DEFAULT_LOG_FILE);
    }

    #[test]
    fn test_daemon_with_custom_paths() {
        let daemon = Daemon::new()
            .with_pid_file("/custom/pid")
            .with_log_file("/custom/log");

        assert_eq!(daemon.log_file, "/custom/log");
    }

    #[test]
    fn test_daemon_status_not_running() {
        let daemon = Daemon::new().with_pid_file(TEST_PID_5);
        let _ = daemon.pid_file.remove();

        // When PID file doesn't exist, status returns error
        let result = daemon.status();
        assert!(result.is_err() || result.unwrap().is_none());
    }

    #[test]
    fn test_daemon_status_running() {
        let daemon = Daemon::new().with_pid_file(TEST_PID_6);
        let _ = daemon.pid_file.remove();

        daemon.pid_file.create().unwrap();

        let status = daemon.status().unwrap();
        assert!(status.is_some());
        assert_eq!(status.unwrap(), std::process::id());

        daemon.pid_file.remove().unwrap();
    }

    #[test]
    fn test_daemon_shutdown_flag() {
        let daemon = Daemon::new();
        assert!(!daemon.is_shutdown_requested());

        daemon.shutdown.store(true, Ordering::Relaxed);
        assert!(daemon.is_shutdown_requested());
    }

    #[test]
    fn test_already_running_error() {
        let pid_file = PidFile::new(TEST_PID_7);
        let _ = pid_file.remove();

        pid_file.create().unwrap();

        let result = pid_file.create();
        assert!(result.is_err());
        assert!(matches!(result, Err(DaemonError::AlreadyRunning(_))));

        pid_file.remove().unwrap();
    }

    #[test]
    fn test_daemon_default_background() {
        let daemon = Daemon::new();
        assert!(!daemon.is_foreground());
    }

    #[test]
    fn test_daemon_with_foreground() {
        let daemon = Daemon::new().with_foreground(true);
        assert!(daemon.is_foreground());
    }

    #[test]
    fn test_daemon_with_foreground_false() {
        let daemon = Daemon::new().with_foreground(false);
        assert!(!daemon.is_foreground());
    }

    #[test]
    fn test_daemon_builder_chain() {
        let daemon = Daemon::new()
            .with_pid_file("/custom/pid")
            .with_log_file("/custom/log")
            .with_foreground(true);

        assert!(daemon.is_foreground());
        assert_eq!(daemon.log_file, "/custom/log");
    }

    // Note: redirect_stdio() uses dup2 which redirects test runner output.
    // Integration tests should spawn the daemon binary and verify log file contents.
}
