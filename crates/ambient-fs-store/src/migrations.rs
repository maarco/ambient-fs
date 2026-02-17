// migrations.rs - schema versioning for ambient-fs-store
//
// schema versioning with migration tracking
// all public functions tested

use rusqlite::{Connection, Result as SqliteResult};
use tracing::{debug, info};

/// current schema version
pub const CURRENT_VERSION: i64 = 1;

/// ensure the database schema is up to date
/// creates tables if missing, runs pending migrations if needed
pub fn ensure_schema(conn: &Connection) -> SqliteResult<()> {
    // first ensure migrations table exists
    ensure_migrations_table(conn)?;

    // get current version
    let current = get_current_version(conn)?;

    // run any pending migrations
    let mut version = current;
    while version < CURRENT_VERSION {
        version += 1;
        info!(target: "ambient-fs-store", "running migration {}", version);
        run_migration(conn, version)?;
        record_migration(conn, version)?;
    }

    if current < CURRENT_VERSION {
        info!(target: "ambient-fs-store",
            "migrated from version {} to {}", current, CURRENT_VERSION);
    } else {
        debug!(target: "ambient-fs-store", "schema at version {}", CURRENT_VERSION);
    }

    Ok(())
}

/// get the current schema version from migrations table
/// returns 0 if table is empty (no migrations applied yet)
fn get_current_version(conn: &Connection) -> SqliteResult<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM migrations",
        [],
        |row| row.get(0),
    )
}

/// ensure migrations table exists
/// called before get_current_version to handle fresh db
fn ensure_migrations_table(conn: &Connection) -> SqliteResult<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS migrations (
            version INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        )",
        [],
    )?;
    Ok(())
}

/// run a specific migration by version number
fn run_migration(conn: &Connection, version: i64) -> SqliteResult<()> {
    match version {
        1 => create_tables_v1(conn),
        _ => panic!("undefined migration version: {}", version),
    }
}

/// record that a migration was applied
fn record_migration(conn: &Connection, version: i64) -> SqliteResult<()> {
    let applied_at = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO migrations (version, applied_at) VALUES (?1, ?2)",
        [version, applied_at],
    )?;
    Ok(())
}

/// migration 1: create all base tables
/// creates file_events, file_analysis, projects tables
fn create_tables_v1(conn: &Connection) -> SqliteResult<()> {
    // file_events: core append-only log
    conn.execute(
        "CREATE TABLE IF NOT EXISTS file_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            file_path TEXT NOT NULL,
            project_id TEXT NOT NULL,
            source TEXT NOT NULL DEFAULT 'user',
            source_id TEXT,
            machine_id TEXT NOT NULL,
            content_hash TEXT,
            old_path TEXT
        )",
        [],
    )?;

    // indexes for file_events
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_events_project_path
            ON file_events(project_id, file_path)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_events_project_time
            ON file_events(project_id, timestamp DESC)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_events_source
            ON file_events(project_id, source)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_events_machine
            ON file_events(machine_id, timestamp DESC)",
        [],
    )?;

    // file_analysis: content analysis cache
    conn.execute(
        "CREATE TABLE IF NOT EXISTS file_analysis (
            file_path TEXT NOT NULL,
            project_id TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            analyzed_at INTEGER NOT NULL,
            exports TEXT,
            imports TEXT,
            todo_count INTEGER DEFAULT 0,
            lint_hints TEXT,
            line_count INTEGER DEFAULT 0,
            PRIMARY KEY (project_id, file_path)
        )",
        [],
    )?;

    // projects: watched projects registry
    conn.execute(
        "CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            added_at INTEGER NOT NULL,
            active INTEGER NOT NULL DEFAULT 1
        )",
        [],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_conn() -> (Connection, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let conn = Connection::open(&path).unwrap();
        (conn, dir)
    }

    #[test]
    fn test_create_tables_creates_all_tables() {
        let (conn, _dir) = temp_conn();

        // run migration 1
        ensure_schema(&conn).unwrap();

        // verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|t| t.unwrap())
            .collect();

        assert!(tables.contains(&"file_events".to_string()));
        assert!(tables.contains(&"file_analysis".to_string()));
        assert!(tables.contains(&"projects".to_string()));
        assert!(tables.contains(&"migrations".to_string()));
    }

    #[test]
    fn test_create_tables_creates_indexes() {
        let (conn, _dir) = temp_conn();
        ensure_schema(&conn).unwrap();

        let indexes: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|t| t.unwrap())
            .collect();

        // sqlite auto-creates indexes for primary keys and uniques
        // our custom indexes should be present
        assert!(indexes.contains(&"idx_events_project_path".to_string()));
        assert!(indexes.contains(&"idx_events_project_time".to_string()));
        assert!(indexes.contains(&"idx_events_source".to_string()));
        assert!(indexes.contains(&"idx_events_machine".to_string()));
    }

    #[test]
    fn test_ensure_schema_idempotent() {
        let (conn, _dir) = temp_conn();

        // run twice - should not error
        ensure_schema(&conn).unwrap();
        ensure_schema(&conn).unwrap();

        // migrations table should only have one entry
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM migrations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_ensure_schema_sets_current_version() {
        let (conn, _dir) = temp_conn();
        ensure_schema(&conn).unwrap();

        let version: i64 = get_current_version(&conn).unwrap();
        assert_eq!(version, CURRENT_VERSION);
    }

    #[test]
    fn test_file_events_table_accepts_insert() {
        let (conn, _dir) = temp_conn();
        ensure_schema(&conn).unwrap();

        conn.execute(
            "INSERT INTO file_events
                (timestamp, event_type, file_path, project_id, source, machine_id)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (1704000000i64, "created", "src/main.rs", "test-project", "user", "machine-1"),
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM file_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_file_analysis_table_accepts_insert() {
        let (conn, _dir) = temp_conn();
        ensure_schema(&conn).unwrap();

        conn.execute(
            "INSERT INTO file_analysis
                (file_path, project_id, content_hash, analyzed_at, line_count)
                VALUES (?1, ?2, ?3, ?4, ?5)",
            ("src/main.rs", "test-project", "hash123", 1704000000i64, 42i32),
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM file_analysis", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_projects_table_accepts_insert() {
        let (conn, _dir) = temp_conn();
        ensure_schema(&conn).unwrap();

        conn.execute(
            "INSERT INTO projects (id, path, name, added_at)
                VALUES (?1, ?2, ?3, ?4)",
            ("proj-1", "/path/to/project", "my-project", 1704000000i64),
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_file_events_default_source() {
        let (conn, _dir) = temp_conn();
        ensure_schema(&conn).unwrap();

        // insert without specifying source
        conn.execute(
            "INSERT INTO file_events
                (timestamp, event_type, file_path, project_id, machine_id)
                VALUES (?1, ?2, ?3, ?4, ?5)",
            (1704000000i64, "created", "src/main.rs", "test-project", "machine-1"),
        )
        .unwrap();

        let source: String = conn
            .query_row("SELECT source FROM file_events WHERE rowid=1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(source, "user");
    }

    #[test]
    fn test_projects_unique_path_constraint() {
        let (conn, _dir) = temp_conn();
        ensure_schema(&conn).unwrap();

        conn.execute(
            "INSERT INTO projects (id, path, name, added_at)
                VALUES (?1, ?2, ?3, ?4)",
            ("proj-1", "/path/to/project", "my-project", 1704000000i64),
        )
        .unwrap();

        // duplicate path should fail
        let result = conn.execute(
            "INSERT INTO projects (id, path, name, added_at)
                VALUES (?1, ?2, ?3, ?4)",
            ("proj-2", "/path/to/project", "other-project", 1704000001i64),
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_file_analysis_primary_key() {
        let (conn, _dir) = temp_conn();
        ensure_schema(&conn).unwrap();

        conn.execute(
            "INSERT INTO file_analysis
                (file_path, project_id, content_hash, analyzed_at, line_count)
                VALUES (?1, ?2, ?3, ?4, ?5)",
            ("src/main.rs", "test-project", "hash123", 1704000000i64, 42i32),
        )
        .unwrap();

        // duplicate (project_id, file_path) should fail
        let result = conn.execute(
            "INSERT INTO file_analysis
                (file_path, project_id, content_hash, analyzed_at, line_count)
                VALUES (?1, ?2, ?3, ?4, ?5)",
            ("src/main.rs", "test-project", "hash456", 1704000001i64, 50i32),
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_file_events_all_columns() {
        let (conn, _dir) = temp_conn();
        ensure_schema(&conn).unwrap();

        conn.execute(
            "INSERT INTO file_events
                (timestamp, event_type, file_path, project_id, source, source_id,
                 machine_id, content_hash, old_path)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            (
                1704000000i64,
                "renamed",
                "src/new.rs",
                "test-project",
                "ai_agent",
                "chat_42",
                "machine-1",
                "hash123",
                "src/old.rs",
            ),
        )
        .unwrap();

        let (event_type, old_path): (String, Option<String>) = conn
            .query_row(
                "SELECT event_type, old_path FROM file_events WHERE rowid=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(event_type, "renamed");
        assert_eq!(old_path, Some("src/old.rs".to_string()));
    }

    #[test]
    fn test_migrations_table_tracks_applied_version() {
        let (conn, _dir) = temp_conn();
        ensure_schema(&conn).unwrap();

        let (version, applied_at): (i64, i64) = conn
            .query_row(
                "SELECT version, applied_at FROM migrations WHERE version=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(version, 1);
        assert!(applied_at > 0);
    }

    #[test]
    fn test_get_current_version_returns_zero_on_fresh_db() {
        let (conn, _dir) = temp_conn();

        // don't run ensure_schema - fresh db has no tables
        // create migrations table only
        ensure_migrations_table(&conn).unwrap();

        let version = get_current_version(&conn).unwrap();
        assert_eq!(version, 0);
    }

    #[test]
    fn test_get_current_version_returns_applied_version() {
        let (conn, _dir) = temp_conn();
        ensure_schema(&conn).unwrap();

        let version = get_current_version(&conn).unwrap();
        assert_eq!(version, CURRENT_VERSION);
    }
}
