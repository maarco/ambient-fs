# ambient-fsd

CLI daemon binary for ambient-fs. Watches directories and logs events.

## Commands

### Start Daemon

```bash
ambient-fsd start
```

Creates PID file at `/tmp/ambient-fs.pid`, starts the server on `/tmp/ambient-fs.sock`.

### Stop Daemon

```bash
ambient-fsd stop
```

Sends SIGTERM to running daemon.

### Status

```bash
ambient-fsd status
```

Shows PID if running.

### Watch Project

```bash
ambient-fsd watch /path/to/project
```

Adds directory to watch list.

### Unwatch Project

```bash
ambient-fsd unwatch <project-id>
```

Removes project from watch list.

### Query Events

```bash
# Last hour of events
ambient-fsd events --since 1h

# Filter by source
ambient-fsd events --source ai_agent

# Filter by project
ambient-fsd events --project my-project

# Limit results
ambient-fsd events --limit 50
```

### Query Awareness

```bash
ambient-fsd awareness <project-id> <path>
```

Shows file awareness state (last modified, TODOs, etc.).

## Config

`~/.config/ambient-fs/config.toml`

```toml
[daemon]
socket_path = "/tmp/ambient-fs.sock"
log_level = "info"

[store]
path = "~/.local/share/ambient-fs/events.db"
retention_days = 90

[watcher]
debounce_ms = 100
ignore_patterns = [".git", "node_modules"]
```

## License

MIT
