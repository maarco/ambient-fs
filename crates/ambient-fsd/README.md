# ambient-fsd

CLI daemon binary for ambient-fs. Watches directories and logs events.

## Commands

### Start Daemon

```bash
# Background mode (forks, redirects stdio to /tmp/ambient-fs.log)
ambient-fsd start

# Foreground mode (stay in terminal, useful for debugging)
ambient-fsd start --foreground
# or
ambient-fsd start -F
```

Creates PID file at `/tmp/ambient-fs.pid`, starts the server on `/tmp/ambient-fs.sock`.

In background mode, stdout and stderr are redirected to `/tmp/ambient-fs.log` and stdin is
redirected from `/dev/null`. In foreground mode (--foreground/-F), stdio remains attached
to the terminal and no forking occurs.

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

Shows file awareness state (last modified, source, TODOs, etc.).

## AI Agent Source Detection

To attribute filesystem events to an AI agent, inject activity via the socket while the daemon is running:

```bash
NOW=$(date +%s)
SOCK=/tmp/ambient-fs.sock

# Signal that agent is editing a file
printf '{"jsonrpc":"2.0","method":"report_agent_activity","params":{
  "ts":%d,"agent":"claude-code","action":"edit",
  "file":"/path/to/file.rs","tool":"claude-code"},"id":1}\n' "$NOW" \
  | nc -U "$SOCK"

# ... agent edits file, events stored with source=ai_agent ...

# Signal agent is done
printf '{"jsonrpc":"2.0","method":"report_agent_activity","params":{
  "ts":%d,"agent":"claude-code","action":"edit",
  "file":"/path/to/file.rs","done":true},"id":2}\n' \
  "$(date +%s)" | nc -U "$SOCK"

# Query to verify
ambient-fsd events --source ai_agent --limit 10
```

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
