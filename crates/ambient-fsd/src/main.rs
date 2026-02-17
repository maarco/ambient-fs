mod daemon;
mod server;
mod config;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

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
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Start { foreground } => cmd_start(foreground).await,
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
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn cmd_start(foreground: bool) -> Result<(), anyhow::Error> {
    // Load config (creates default if missing)
    let daemon_config = config::load()?;

    // Initialize logging
    let log_level = daemon_config.log_level.clone();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&log_level))
        )
        .init();

    let daemon = daemon::Daemon::new().with_foreground(foreground);

    // Check if already running (ignore PidFileRead errors)
    let already_running = match daemon.status() {
        Ok(Some(pid)) => {
            println!("daemon already running (PID {})", pid);
            return Ok(());
        }
        Ok(None) => false,
        Err(daemon::DaemonError::PidFileRead(_)) => false,
        Err(e) => return Err(e.into()),
    };

    if already_running {
        return Ok(());
    }

    // Create PID file
    daemon.create_pid_file()?;

    // Convert DaemonConfig to ServerConfig
    let server_config = config::to_server_config(&daemon_config);
    let server = server::DaemonServer::new(server_config).await?;
    println!("daemon started (PID {})", std::process::id());

    // Run server (blocks until shutdown)
    server.run().await?;

    // Clean up PID file on exit
    let _ = daemon.remove_pid_file();

    Ok(())
}

async fn cmd_stop() -> Result<(), anyhow::Error> {
    let daemon = daemon::Daemon::new();
    match daemon.stop() {
        Ok(()) => {
            println!("daemon stopped");
            Ok(())
        }
        Err(daemon::DaemonError::NotRunning) => {
            println!("daemon not running");
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

async fn cmd_status() -> Result<(), anyhow::Error> {
    let daemon = daemon::Daemon::new();
    match daemon.status()? {
        Some(pid) => {
            println!("daemon running (PID {})", pid);
        }
        None => {
            println!("daemon not running");
        }
    }
    Ok(())
}

async fn cmd_watch(path: PathBuf) -> Result<(), anyhow::Error> {
    let canonical = path.canonicalize()?;
    let config = server::ServerConfig::default();
    let server = server::DaemonServer::new(config).await?;
    let project_id = server.watch_project(canonical.clone()).await?;
    println!("watching: {} (id: {})", canonical.display(), project_id);
    Ok(())
}

async fn cmd_unwatch(id: String) -> Result<(), anyhow::Error> {
    let config = server::ServerConfig::default();
    let server = server::DaemonServer::new(config).await?;
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
    use ambient_fs_core::event::Source;

    let config = server::ServerConfig::default();
    let server = server::DaemonServer::new(config).await?;

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
fn parse_duration(s: &str) -> Option<Duration> {
    let num = s.chars().take_while(|c| c.is_ascii_digit()).collect::<String>();
    let num: u64 = num.parse().ok()?;
    let suffix = s.chars().skip_while(|c| c.is_ascii_digit()).collect::<String>();

    match suffix.as_str() {
        "s" | "sec" => Some(Duration::from_secs(num)),
        "m" | "min" => Some(Duration::from_secs(num * 60)),
        "h" | "hour" => Some(Duration::from_secs(num * 3600)),
        "d" | "day" => Some(Duration::from_secs(num * 86400)),
        _ => None,
    }
}

async fn cmd_awareness(project: String, path: PathBuf) -> Result<(), anyhow::Error> {
    println!("querying awareness for project: {}", project);
    println!("  path: {}", path.display());

    let config = server::ServerConfig::default();
    let server = server::DaemonServer::new(config).await?;

    // Get latest event for this file
    if let Some(event) = server.query_events(Some(&project), None, None, Some(1)).await?.first() {
        println!("  last modified: {}", event.timestamp.format("%Y-%m-%d %H:%M:%S"));
        println!("  modified by: {}", event.source);
    } else {
        println!("  no events found");
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
}
