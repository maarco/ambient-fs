// Event pruning for ambient-fs-store
// Tests first, implementation below

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    #[test]
    fn prune_events_before_deletes_old_rows() {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();

        conn.execute(
            "CREATE TABLE events (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                event_type TEXT NOT NULL,
                file_path TEXT NOT NULL,
                project_id TEXT NOT NULL,
                source TEXT NOT NULL,
                source_id TEXT,
                machine_id TEXT NOT NULL,
                content_hash TEXT,
                old_path TEXT
            )",
            [],
        )
        .unwrap();

        let now = Utc::now();
        let old = now - Duration::days(100);
        let recent = now - Duration::days(10);

        conn.execute(
            "INSERT INTO events (timestamp, event_type, file_path, project_id, source, machine_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            [&old.to_rfc3339(), "created", "old.txt", "proj", "user", "m1"],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO events (timestamp, event_type, file_path, project_id, source, machine_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            [&recent.to_rfc3339(), "created", "new.txt", "proj", "user", "m1"],
        )
        .unwrap();

        let cutoff = now - Duration::days(30);
        let deleted = EventPruner::prune_events_before(&conn, cutoff).unwrap();

        assert_eq!(deleted, 1);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let path: String = conn
            .query_row("SELECT file_path FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(path, "new.txt");
    }

    #[test]
    fn prune_events_before_returns_zero_when_no_old_events() {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();

        conn.execute(
            "CREATE TABLE events (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                event_type TEXT NOT NULL,
                file_path TEXT NOT NULL,
                project_id TEXT NOT NULL,
                source TEXT NOT NULL,
                machine_id TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

        let now = Utc::now();
        conn.execute(
            "INSERT INTO events (timestamp, event_type, file_path, project_id, source, machine_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            [&now.to_rfc3339(), "created", "file.txt", "proj", "user", "m1"],
        )
        .unwrap();

        let cutoff = now - Duration::days(365);
        let deleted = EventPruner::prune_events_before(&conn, cutoff).unwrap();

        assert_eq!(deleted, 0);
    }

    #[test]
    fn prune_events_before_handles_empty_table() {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();

        conn.execute(
            "CREATE TABLE events (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

        let cutoff = Utc::now();
        let deleted = EventPruner::prune_events_before(&conn, cutoff).unwrap();

        assert_eq!(deleted, 0);
    }

    #[test]
    fn prune_events_before_deletes_all_when_cutoff_is_future() {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();

        conn.execute(
            "CREATE TABLE events (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                event_type TEXT NOT NULL,
                file_path TEXT NOT NULL,
                project_id TEXT NOT NULL,
                source TEXT NOT NULL,
                machine_id TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

        let now = Utc::now();
        conn.execute(
            "INSERT INTO events (timestamp, event_type, file_path, project_id, source, machine_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            [&now.to_rfc3339(), "created", "file.txt", "proj", "user", "m1"],
        )
        .unwrap();

        let cutoff = now + Duration::days(1);
        let deleted = EventPruner::prune_events_before(&conn, cutoff).unwrap();

        assert_eq!(deleted, 1);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn prune_analysis_before_deletes_old_analysis() {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();

        conn.execute(
            "CREATE TABLE file_analysis (
                id INTEGER PRIMARY KEY,
                file_path TEXT NOT NULL,
                project_id TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                analyzed_at TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

        let now = Utc::now();
        let old = now - Duration::days(100);
        let recent = now - Duration::days(10);

        conn.execute(
            "INSERT INTO file_analysis (file_path, project_id, content_hash, analyzed_at)
             VALUES (?1, ?2, ?3, ?4)",
            ["old.rs", "proj", "hash1", &old.to_rfc3339()],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO file_analysis (file_path, project_id, content_hash, analyzed_at)
             VALUES (?1, ?2, ?3, ?4)",
            ["new.rs", "proj", "hash2", &recent.to_rfc3339()],
        )
        .unwrap();

        let cutoff = now - Duration::days(30);
        let deleted = EventPruner::prune_analysis_before(&conn, cutoff).unwrap();

        assert_eq!(deleted, 1);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM file_analysis", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let path: String = conn
            .query_row("SELECT file_path FROM file_analysis", [], |row| row.get(0))
            .unwrap();
        assert_eq!(path, "new.rs");
    }

    #[test]
    fn prune_analysis_before_returns_zero_when_no_old_analysis() {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();

        conn.execute(
            "CREATE TABLE file_analysis (
                id INTEGER PRIMARY KEY,
                file_path TEXT NOT NULL,
                project_id TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                analyzed_at TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

        let now = Utc::now();
        conn.execute(
            "INSERT INTO file_analysis (file_path, project_id, content_hash, analyzed_at)
             VALUES (?1, ?2, ?3, ?4)",
            ["file.rs", "proj", "hash", &now.to_rfc3339()],
        )
        .unwrap();

        let cutoff = now - Duration::days(365);
        let deleted = EventPruner::prune_analysis_before(&conn, cutoff).unwrap();

        assert_eq!(deleted, 0);
    }

    #[test]
    fn prune_analysis_before_handles_empty_table() {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();

        conn.execute(
            "CREATE TABLE file_analysis (
                id INTEGER PRIMARY KEY,
                file_path TEXT NOT NULL,
                analyzed_at TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

        let cutoff = Utc::now();
        let deleted = EventPruner::prune_analysis_before(&conn, cutoff).unwrap();

        assert_eq!(deleted, 0);
    }

    #[test]
    fn vacuum_reclaims_space() {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();

        conn.execute(
            "CREATE TABLE test_table (
                id INTEGER PRIMARY KEY,
                data TEXT
            )",
            [],
        )
        .unwrap();

        for i in 0..100 {
            conn.execute(
                "INSERT INTO test_table (data) VALUES (?1)",
                [format!("some data {}", i)],
            )
            .unwrap();
        }

        conn.execute("DELETE FROM test_table", []).unwrap();

        EventPruner::vacuum(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM test_table", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn vacuum_on_empty_database() {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();

        conn.execute(
            "CREATE TABLE dummy (id INTEGER PRIMARY KEY)",
            [],
        )
        .unwrap();

        EventPruner::vacuum(&conn).unwrap();
    }

    #[test]
    fn retention_days_default_90() {
        let config = PruneConfig::default();
        assert_eq!(config.retention_days, 90);
    }

    #[test]
    fn retention_days_custom() {
        let config = PruneConfig::new(30);
        assert_eq!(config.retention_days, 30);
    }

    #[test]
    fn retention_days_calculates_cutoff() {
        let config = PruneConfig::new(7);
        let now = Utc::now();
        let cutoff = config.cutoff_timestamp();

        let diff = now - cutoff;
        assert!(diff.num_days() >= 6);
        assert!(diff.num_days() <= 8);
    }

    #[test]
    fn prune_by_config() {
        let temp = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp.path()).unwrap();

        conn.execute(
            "CREATE TABLE events (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                event_type TEXT NOT NULL,
                file_path TEXT NOT NULL,
                project_id TEXT NOT NULL,
                source TEXT NOT NULL,
                machine_id TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

        let now = Utc::now();

        conn.execute(
            "INSERT INTO events (timestamp, event_type, file_path, project_id, source, machine_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            [&(now - Duration::days(5)).to_rfc3339(), "created", "keep.txt", "proj", "user", "m1"],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO events (timestamp, event_type, file_path, project_id, source, machine_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            [&(now - Duration::days(20)).to_rfc3339(), "created", "delete.txt", "proj", "user", "m1"],
        )
        .unwrap();

        let config = PruneConfig::new(10);
        let cutoff = config.cutoff_timestamp();
        let deleted = EventPruner::prune_events_before(&conn, cutoff).unwrap();

        assert_eq!(deleted, 1);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}

use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use thiserror::Error;

/// Configuration for event pruning
#[derive(Debug, Clone)]
pub struct PruneConfig {
    pub retention_days: i64,
}

impl PruneConfig {
    pub fn new(retention_days: i64) -> Self {
        Self { retention_days }
    }

    pub fn cutoff_timestamp(&self) -> DateTime<Utc> {
        Utc::now() - Duration::days(self.retention_days)
    }
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self { retention_days: 90 }
    }
}

/// Errors from pruning operations
#[derive(Debug, Error)]
pub enum PruneError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, PruneError>;

/// Event pruning operations
pub struct EventPruner;

impl EventPruner {
    pub fn prune_events_before(conn: &Connection, cutoff: DateTime<Utc>) -> Result<usize> {
        let cutoff_str = cutoff.to_rfc3339();
        conn.execute(
            "DELETE FROM events WHERE timestamp < ?1",
            [&cutoff_str],
        )
        .map_err(Into::into)
    }

    pub fn prune_analysis_before(conn: &Connection, cutoff: DateTime<Utc>) -> Result<usize> {
        let cutoff_str = cutoff.to_rfc3339();
        conn.execute(
            "DELETE FROM file_analysis WHERE analyzed_at < ?1",
            [&cutoff_str],
        )
        .map_err(Into::into)
    }

    pub fn vacuum(conn: &Connection) -> Result<()> {
        conn.execute("VACUUM", []).map(|_| ())
            .map_err(Into::into)
    }
}
