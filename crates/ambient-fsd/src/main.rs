mod daemon;
mod server;
mod config;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use chrono::TimeDelta;
use rusqlite::Connection;

/// Ambient filesystem awareness daemon
#[derive(Debug, Clone, Parser)]
#[command(name = "ambient-fsd")]
#[command(version = "0.1.0")]
#[command(about = "Watch project directories, log file events with source attribution, run background content analysis", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    /// Start the daemon (background, auto-restarts)
    Start {
        /// Run in foreground (don't redirect stdio, don't fork)
        #[arg(short = 'F', long)]
        foreground: bool,
    },
    /// Stop the daemon (kill via PID file)
    Stop,
    /// Show daemon status, watched projects, and stats
    Status,
    /// Add a project directory to the watch list
    Watch {
        /// Path to the project directory
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
    /// Remove a project from the watch list
    Unwatch {
        /// Project ID to remove
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Query the event log
    Events {
        /// Show events since this time (e.g., "1h", "30m", "2024-01-01")
        #[arg(short = 's', long)]
        since: Option<String>,

        /// Filter by source (user, ai_agent, git, build, voice)
        #[arg(short = 'S', long)]
        source: Option<String>,

        /// Filter by project ID
        #[arg(short, long)]
        project: Option<String>,

        /// Limit number of results
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// Query file awareness state for a project
    Awareness {
        /// Project ID
        #[arg(value_name = "PROJECT")]
        project: String,

        /// Path to file or directory
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
    /// Prune old events and analysis records
    Prune {
        /// Retention period in days (default: 90)
        #[arg(short = 'r', long, default_value = "90")]
        retention_days: i64,
    },
    /// Install service file for auto-start on login
    InstallService,
    /// Uninstall service file
    UninstallService,
}

/// Context from synchronous daemon setup, passed to async runtime.
struct StartContext {
    daemon: daemon::Daemon,
    server_config: server::ServerConfig,
}

fn main() {
    let cli = Cli::parse();

    // Start command: daemonize BEFORE creating tokio runtime.
    // fork() inside a tokio runtime corrupts kqueue/epoll fds.
    let start_ctx = if let Command::Start { foreground } = &cli.command {
        match prepare_start(*foreground) {
            Ok(ctx) => Some(ctx),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    // Create tokio runtime AFTER daemonization
    let rt = tokio::runtime::Runtime::new()
        .expect("failed to create tokio runtime");

    let result = rt.block_on(run(cli, start_ctx));

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Synchronous daemon preparation: config, logging, fork, PID, signals, stdio.
/// Must complete before tokio runtime is created.
fn prepare_start(foreground: bool) -> Result<StartContext, anyhow::Error> {
    let daemon_config = config::load()?;

    let log_level = daemon_config.log_level.clone();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&log_level))
        )
        .init();

    let daemon = daemon::Daemon::new().with_foreground(foreground);

    if daemon.is_foreground() {
        tracing::info!("starting daemon in foreground mode");
    } else {
        tracing::info!("starting daemon in background mode");
    }

    // Check if already running
    match daemon.status() {
        Ok(Some(pid)) => {
            println!("daemon already running (PID {})", pid);
            std::process::exit(0);
        }
        Ok(None) => {}
        Err(daemon::DaemonError::PidFileRead(_)) => {}
        Err(e) => return Err(e.into()),
    }

    // Fork + PID file + signal handlers + stdio redirect
    // ALL synchronous, ALL before tokio runtime creation
    daemon.start()?;

    let server_config = config::to_server_config(&daemon_config);
    tracing::info!("daemon started (PID {})", std::process::id());

    Ok(StartContext { daemon, server_config })
}

/// Async entry point, runs inside tokio runtime.
async fn run(cli: Cli, start_ctx: Option<StartContext>) -> Result<(), anyhow::Error> {
    match cli.command {
        Command::Start { .. } => {
            let ctx = start_ctx.expect("start context must be set");
            let server = server::DaemonServer::new(ctx.server_config).await?
                .with_shutdown_flag(ctx.daemon.shutdown_flag());
            server.run().await?;
            let _ = ctx.daemon.remove_pid_file();
            Ok(())
        }
        Command::Stop => cmd_stop().await,
        Command::Status => cmd_status().await,
        Command::Watch { path } => cmd_watch(path).await,
        Command::Unwatch { id } => cmd_unwatch(id).await,
        Command::Events {
            since,
            source,
            project,
            limit,
        } => cmd_events(since, source, project, limit).await,
        Command::Awareness { project, path } => cmd_awareness(project, path).await,
        Command::Prune { retention_days } => cmd_prune(retention_days).await,
        Command::InstallService => cmd_install_service(),
        Command::UninstallService => cmd_uninstall_service(),
    }
}

async fn cmd_stop() -> Result<(), anyhow::Error> {
    let daemon = daemon::Daemon::new();
    match daemon.stop() {
        Ok(()) => {
            println!("daemon stopped");
            Ok(())
        }
        Err(daemon::DaemonError::NotRunning) | Err(daemon::DaemonError::PidFileRead(_)) => {
            println!("daemon not running");
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

async fn cmd_status() -> Result<(), anyhow::Error> {
    let daemon = daemon::Daemon::new();
    match daemon.status() {
        Ok(Some(pid)) => println!("daemon running (PID {})", pid),
        Ok(None) => println!("daemon not running"),
        Err(daemon::DaemonError::PidFileRead(_)) => println!("daemon not running"),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

async fn cmd_watch(path: PathBuf) -> Result<(), anyhow::Error> {
    let canonical = path.canonicalize()?;
    let server_config = config::to_server_config(&config::load()?);
    let server = server::DaemonServer::new(server_config).await?;
    let project_id = server.watch_project(canonical.clone()).await?;
    println!("watching: {} (id: {})", canonical.display(), project_id);
    Ok(())
}

async fn cmd_unwatch(id: String) -> Result<(), anyhow::Error> {
    let server_config = config::to_server_config(&config::load()?);
    let server = server::DaemonServer::new(server_config).await?;
    server.unwatch_project(&id).await?;
    println!("unwatched project: {}", id);
    Ok(())
}

async fn cmd_events(
    since: Option<String>,
    source: Option<String>,
    project: Option<String>,
    limit: Option<usize>,
) -> Result<(), anyhow::Error> {
    let server_config = config::to_server_config(&config::load()?);
    let server = server::DaemonServer::new(server_config).await?;

    let since_duration = since.and_then(|s| parse_duration(&s));
    let source_enum = source.and_then(|s| s.parse().ok());

    let events = server.query_events(
        project.as_deref(),
        since_duration,
        source_enum,
        limit,
    ).await?;

    if events.is_empty() {
        println!("no events found");
    } else {
        for event in events {
            println!("{} {} {} {}",
                event.timestamp.format("%Y-%m-%d %H:%M:%S"),
                event.source,
                event.event_type,
                event.file_path,
            );
        }
    }
    Ok(())
}

/// Parse duration string like "1h", "30m", "7d"
fn parse_duration(s: &str) -> Option<TimeDelta> {
    let num = s.chars().take_while(|c| c.is_ascii_digit()).collect::<String>();
    let num: i64 = num.parse().ok()?;
    let suffix = s.chars().skip_while(|c| c.is_ascii_digit()).collect::<String>();

    match suffix.as_str() {
        "s" | "sec" => Some(TimeDelta::seconds(num)),
        "m" | "min" => Some(TimeDelta::minutes(num)),
        "h" | "hour" => Some(TimeDelta::hours(num)),
        "d" | "day" => Some(TimeDelta::days(num)),
        _ => None,
    }
}

async fn cmd_awareness(project: String, path: PathBuf) -> Result<(), anyhow::Error> {
    println!("querying awareness for project: {}", project);
    println!("  path: {}", path.display());

    let server_config = config::to_server_config(&config::load()?);
    let server = server::DaemonServer::new(server_config).await?;

    // Get latest event for this file
    if let Some(event) = server.query_events(Some(&project), None, None, Some(1)).await?.first() {
        println!("  last modified: {}", event.timestamp.format("%Y-%m-%d %H:%M:%S"));
        println!("  modified by: {}", event.source);
    } else {
        println!("  no events found");
    }

    Ok(())
}

async fn cmd_prune(retention_days: i64) -> Result<(), anyhow::Error> {
    use ambient_fs_store::{EventPruner, PruneConfig, migrations};

    let config = server::ServerConfig::default();
    let db_path = config.db_path;

    println!("pruning events older than {} days...", retention_days);

    let result = tokio::task::spawn_blocking(move || {
        let prune_config = PruneConfig::new(retention_days);
        let cutoff = prune_config.cutoff_timestamp();
        let conn = Connection::open(&db_path)?;

        // Ensure schema exists
        migrations::ensure_schema(&conn)?;

        let events = EventPruner::prune_events_before(&conn, cutoff)?;
        let analysis = EventPruner::prune_analysis_before(&conn, cutoff)?;

        if events > 0 || analysis > 0 {
            EventPruner::vacuum(&conn)?;
        }

        Ok::<_, anyhow::Error>((events, analysis))
    }).await??;

    println!("pruned {} events, {} analysis records", result.0, result.1);
    Ok(())
}

/// Service file templates embedded at compile time
#[cfg(target_os = "macos")]
const LAUNCHD_PLIST_TEMPLATE: &str = include_str!("../deploy/com.ambient-fs.daemon.plist");

#[cfg(target_os = "linux")]
const SYSTEMD_SERVICE_TEMPLATE: &str = include_str!("../deploy/ambient-fsd.service");

fn cmd_install_service() -> Result<(), anyhow::Error> {
    let binary = std::env::current_exe()?;
    let binary_path = binary.display().to_string();

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME")?;
        let service_dir = format!("{}/Library/LaunchAgents", home);
        let service_path = format!("{}/com.ambient-fs.daemon.plist", service_dir);

        // Create directory if it doesn't exist
        std::fs::create_dir_all(&service_dir)?;

        // Replace {BINARY} placeholder with actual binary path
        let plist_content = LAUNCHD_PLIST_TEMPLATE.replace("{BINARY}", &binary_path);

        // Write the service file
        std::fs::write(&service_path, plist_content)?;

        println!("service file installed to: {}", service_path);
        println!();
        println!("to enable and start the service, run:");
        println!("  launchctl load {}", service_path);
        println!();
        println!("to check status:");
        println!("  launchctl list | grep com.ambient-fs");
        println!();
        println!("to stop and unload:");
        println!("  launchctl unload {}", service_path);
    }

    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME")?;
        let service_dir = format!("{}/.config/systemd/user", home);
        let service_path = format!("{}/ambient-fsd.service", service_dir);

        // Create directory if it doesn't exist
        std::fs::create_dir_all(&service_dir)?;

        // Replace {BINARY} placeholder with actual binary path
        let service_content = SYSTEMD_SERVICE_TEMPLATE.replace("{BINARY}", &binary_path);

        // Write the service file
        std::fs::write(&service_path, service_content)?;

        println!("service file installed to: {}", service_path);
        println!();
        println!("to enable and start the service, run:");
        println!("  systemctl --user daemon-reload");
        println!("  systemctl --user enable ambient-fsd");
        println!("  systemctl --user start ambient-fsd");
        println!();
        println!("to check status:");
        println!("  systemctl --user status ambient-fsd");
        println!();
        println!("to stop and disable:");
        println!("  systemctl --user stop ambient-fsd");
        println!("  systemctl --user disable ambient-fsd");
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        anyhow::bail!("install-service not supported on this platform");
    }

    Ok(())
}

fn cmd_uninstall_service() -> Result<(), anyhow::Error> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME")?;
        let service_path = format!("{}/Library/LaunchAgents/com.ambient-fs.daemon.plist", home);

        if std::fs::exists(&service_path)? || std::fs::metadata(&service_path).is_ok() {
            std::fs::remove_file(&service_path)?;
            println!("service file removed: {}", service_path);
            println!();
            println!("if the service was loaded, unload it with:");
            println!("  launchctl unload {}", service_path);
        } else {
            println!("service file not found: {}", service_path);
        }
    }

    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME")?;
        let service_path = format!("{}/.config/systemd/user/ambient-fsd.service", home);

        if std::fs::exists(&service_path)? || std::fs::metadata(&service_path).is_ok() {
            std::fs::remove_file(&service_path)?;
            println!("service file removed: {}", service_path);
            println!();
            println!("if the service was enabled, disable it with:");
            println!("  systemctl --user stop ambient-fsd");
            println!("  systemctl --user disable ambient-fsd");
            println!("  systemctl --user daemon-reload");
        } else {
            println!("service file not found: {}", service_path);
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        anyhow::bail!("uninstall-service not supported on this platform");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_verify() {
        Cli::command().debug_assert();
    }

    #[test]
    fn cli_start() {
        let cli = Cli::parse_from(["ambient-fsd", "start"]);
        match cli.command {
            Command::Start { foreground } => {
                assert!(!foreground);
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn cli_start_foreground_short() {
        let cli = Cli::parse_from(["ambient-fsd", "start", "-F"]);
        match cli.command {
            Command::Start { foreground } => {
                assert!(foreground);
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn cli_start_foreground_long() {
        let cli = Cli::parse_from(["ambient-fsd", "start", "--foreground"]);
        match cli.command {
            Command::Start { foreground } => {
                assert!(foreground);
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn cli_stop() {
        let cli = Cli::parse_from(["ambient-fsd", "stop"]);
        assert!(matches!(cli.command, Command::Stop));
    }

    #[test]
    fn cli_status() {
        let cli = Cli::parse_from(["ambient-fsd", "status"]);
        assert!(matches!(cli.command, Command::Status));
    }

    #[test]
    fn cli_watch() {
        let cli = Cli::parse_from(["ambient-fsd", "watch", "/home/user/project"]);
        match cli.command {
            Command::Watch { path } => {
                assert_eq!(path, PathBuf::from("/home/user/project"));
            }
            _ => panic!("expected Watch command"),
        }
    }

    #[test]
    fn cli_watch_relative_path() {
        let cli = Cli::parse_from(["ambient-fsd", "watch", "./my-project"]);
        match cli.command {
            Command::Watch { path } => {
                assert_eq!(path, PathBuf::from("./my-project"));
            }
            _ => panic!("expected Watch command"),
        }
    }

    #[test]
    fn cli_unwatch() {
        let cli = Cli::parse_from(["ambient-fsd", "unwatch", "proj-123"]);
        match cli.command {
            Command::Unwatch { id } => {
                assert_eq!(id, "proj-123");
            }
            _ => panic!("expected Unwatch command"),
        }
    }

    #[test]
    fn cli_events_no_filters() {
        let cli = Cli::parse_from(["ambient-fsd", "events"]);
        match cli.command {
            Command::Events {
                since,
                source,
                project,
                limit,
            } => {
                assert!(since.is_none());
                assert!(source.is_none());
                assert!(project.is_none());
                assert!(limit.is_none());
            }
            _ => panic!("expected Events command"),
        }
    }

    #[test]
    fn cli_events_with_since() {
        let cli = Cli::parse_from(["ambient-fsd", "events", "--since", "1h"]);
        match cli.command {
            Command::Events { since, .. } => {
                assert_eq!(since.as_deref(), Some("1h"));
            }
            _ => panic!("expected Events command"),
        }
    }

    #[test]
    fn cli_events_with_source() {
        let cli = Cli::parse_from(["ambient-fsd", "events", "--source", "ai_agent"]);
        match cli.command {
            Command::Events { source, .. } => {
                assert_eq!(source.as_deref(), Some("ai_agent"));
            }
            _ => panic!("expected Events command"),
        }
    }

    #[test]
    fn cli_events_with_project() {
        let cli = Cli::parse_from(["ambient-fsd", "events", "--project", "my-project"]);
        match cli.command {
            Command::Events { project, .. } => {
                assert_eq!(project.as_deref(), Some("my-project"));
            }
            _ => panic!("expected Events command"),
        }
    }

    #[test]
    fn cli_events_with_limit() {
        let cli = Cli::parse_from(["ambient-fsd", "events", "--limit", "50"]);
        match cli.command {
            Command::Events { limit, .. } => {
                assert_eq!(limit, Some(50));
            }
            _ => panic!("expected Events command"),
        }
    }

    #[test]
    fn cli_events_all_filters() {
        let cli = Cli::parse_from([
            "ambient-fsd",
            "events",
            "--since",
            "30m",
            "--source",
            "git",
            "--project",
            "kollabor",
            "--limit",
            "100",
        ]);
        match cli.command {
            Command::Events {
                since,
                source,
                project,
                limit,
            } => {
                assert_eq!(since.as_deref(), Some("30m"));
                assert_eq!(source.as_deref(), Some("git"));
                assert_eq!(project.as_deref(), Some("kollabor"));
                assert_eq!(limit, Some(100));
            }
            _ => panic!("expected Events command"),
        }
    }

    #[test]
    fn cli_events_short_flags() {
        let cli = Cli::parse_from([
            "ambient-fsd",
            "events",
            "-s",
            "1h",
            "-S",
            "user",
            "-p",
            "test-proj",
            "-l",
            "20",
        ]);
        match cli.command {
            Command::Events {
                since,
                source,
                project,
                limit,
            } => {
                assert_eq!(since.as_deref(), Some("1h"));
                assert_eq!(source.as_deref(), Some("user"));
                assert_eq!(project.as_deref(), Some("test-proj"));
                assert_eq!(limit, Some(20));
            }
            _ => panic!("expected Events command"),
        }
    }

    #[test]
    fn cli_awareness() {
        let cli = Cli::parse_from(["ambient-fsd", "awareness", "my-project", "src/main.rs"]);
        match cli.command {
            Command::Awareness { project, path } => {
                assert_eq!(project, "my-project");
                assert_eq!(path, PathBuf::from("src/main.rs"));
            }
            _ => panic!("expected Awareness command"),
        }
    }

    #[test]
    fn cli_awareness_with_directory() {
        let cli = Cli::parse_from(["ambient-fsd", "awareness", "my-project", "src/"]);
        match cli.command {
            Command::Awareness { project, path } => {
                assert_eq!(project, "my-project");
                assert_eq!(path, PathBuf::from("src/"));
            }
            _ => panic!("expected Awareness command"),
        }
    }

    #[test]
    fn cli_prune_default_retention() {
        let cli = Cli::parse_from(["ambient-fsd", "prune"]);
        match cli.command {
            Command::Prune { retention_days } => {
                assert_eq!(retention_days, 90);
            }
            _ => panic!("expected Prune command"),
        }
    }

    #[test]
    fn cli_prune_custom_retention_short() {
        let cli = Cli::parse_from(["ambient-fsd", "prune", "-r", "30"]);
        match cli.command {
            Command::Prune { retention_days } => {
                assert_eq!(retention_days, 30);
            }
            _ => panic!("expected Prune command"),
        }
    }

    #[test]
    fn cli_prune_custom_retention_long() {
        let cli = Cli::parse_from(["ambient-fsd", "prune", "--retention-days", "14"]);
        match cli.command {
            Command::Prune { retention_days } => {
                assert_eq!(retention_days, 14);
            }
            _ => panic!("expected Prune command"),
        }
    }

    #[test]
    fn cli_long_about() {
        let cmd = Cli::command();
        assert!(cmd.get_about().is_some());
    }

    #[test]
    fn cli_version() {
        let cmd = Cli::command();
        assert_eq!(cmd.get_version().unwrap(), "0.1.0");
    }

    #[test]
    fn cli_name() {
        let cmd = Cli::command();
        assert_eq!(cmd.get_name(), "ambient-fsd");
    }

    #[test]
    fn cli_install_service() {
        let cli = Cli::parse_from(["ambient-fsd", "install-service"]);
        assert!(matches!(cli.command, Command::InstallService));
    }

    #[test]
    fn cli_uninstall_service() {
        let cli = Cli::parse_from(["ambient-fsd", "uninstall-service"]);
        assert!(matches!(cli.command, Command::UninstallService));
    }

    #[test]
    fn test_launchd_template_renders() {
        let result = LAUNCHD_PLIST_TEMPLATE.replace("{BINARY}", "/usr/local/bin/ambient-fsd");
        assert!(result.contains("/usr/local/bin/ambient-fsd"));
        assert!(result.contains("com.ambient-fs.daemon"));
        assert!(result.contains("start"));
        assert!(result.contains("--foreground"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_systemd_template_renders() {
        let result = SYSTEMD_SERVICE_TEMPLATE.replace("{BINARY}", "/usr/local/bin/ambient-fsd");
        assert!(result.contains("/usr/local/bin/ambient-fsd"));
        assert!(result.contains("ExecStart"));
        assert!(result.contains("Restart=on-failure"));
        assert!(result.contains("WantedBy=default.target"));
    }

    #[test]
    fn test_launchd_template_contains_expected_keys() {
        assert!(LAUNCHD_PLIST_TEMPLATE.contains("{BINARY}"));
        assert!(LAUNCHD_PLIST_TEMPLATE.contains("Label"));
        assert!(LAUNCHD_PLIST_TEMPLATE.contains("ProgramArguments"));
        assert!(LAUNCHD_PLIST_TEMPLATE.contains("RunAtLoad"));
        assert!(LAUNCHD_PLIST_TEMPLATE.contains("KeepAlive"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_systemd_template_contains_expected_keys() {
        assert!(SYSTEMD_SERVICE_TEMPLATE.contains("{BINARY}"));
        assert!(SYSTEMD_SERVICE_TEMPLATE.contains("ExecStart"));
        assert!(SYSTEMD_SERVICE_TEMPLATE.contains("Type=simple"));
        assert!(SYSTEMD_SERVICE_TEMPLATE.contains("Restart=on-failure"));
        assert!(SYSTEMD_SERVICE_TEMPLATE.contains("RestartSec=5"));
    }
}
