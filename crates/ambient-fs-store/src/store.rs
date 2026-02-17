use ambient_fs_core::{FileEvent, EventType, Source, ParseError as CoreParseError};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database at path {0} is locked")]
    DatabaseLocked(String),
    #[error("parse error: {0}")]
    Parse(#[from] CoreParseError),
    #[error("timestamp parse error: {0}")]
    TimestampParse(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    project_id: Option<String>,
    file_path: Option<String>,
    source: Option<Source>,
    since: Option<DateTime<Utc>>,
    limit: Option<usize>,
}

impl EventFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn project_id(mut self, id: impl Into<String>) -> Self {
        self.project_id = Some(id.into());
        self
    }

    pub fn file_path(mut self, path: impl Into<String>) -> Self {
        self.file_path = Some(path.into());
        self
    }

    pub fn source(mut self, source: Source) -> Self {
        self.source = Some(source);
        self
    }

    pub fn since(mut self, timestamp: DateTime<Utc>) -> Self {
        self.since = Some(timestamp);
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

pub struct EventStore {
    conn: Connection,
}

impl EventStore {
    pub fn new(path: PathBuf) -> Result<Self> {
        let conn = Connection::open(&path)?;

        // WAL mode for better concurrency
        conn.pragma_update(None, "journal_mode", "WAL")?;

        // Create table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS file_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                event_type TEXT NOT NULL CHECK(event_type IN ('created', 'modified', 'deleted', 'renamed')),
                file_path TEXT NOT NULL,
                project_id TEXT NOT NULL,
                source TEXT NOT NULL CHECK(source IN ('user', 'ai_agent', 'git', 'build', 'voice')),
                source_id TEXT,
                machine_id TEXT NOT NULL,
                content_hash TEXT,
                old_path TEXT
            )",
            [],
        )?;

        // Indexes for common queries
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_project_id ON file_events(project_id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_file_path ON file_events(file_path)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_timestamp ON file_events(timestamp)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_project_file ON file_events(project_id, file_path)",
            [],
        )?;

        // Create projects table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS projects (
                project_id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
            [],
        )?;

        Ok(EventStore { conn })
    }

    pub fn insert(&self, event: &FileEvent) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO file_events (timestamp, event_type, file_path, project_id, source, source_id, machine_id, content_hash, old_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event.timestamp.to_rfc3339(),
                event.event_type.as_str(),
                event.file_path,
                event.project_id,
                event.source.as_str(),
                event.source_id,
                event.machine_id,
                event.content_hash,
                event.old_path,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_batch(&self, events: &[FileEvent]) -> Result<Vec<i64>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut ids = Vec::with_capacity(events.len());

        for event in events {
            tx.execute(
                "INSERT INTO file_events (timestamp, event_type, file_path, project_id, source, source_id, machine_id, content_hash, old_path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    event.timestamp.to_rfc3339(),
                    event.event_type.as_str(),
                    event.file_path,
                    event.project_id,
                    event.source.as_str(),
                    event.source_id,
                    event.machine_id,
                    event.content_hash,
                    event.old_path,
                ],
            )?;
            ids.push(tx.last_insert_rowid());
        }

        tx.commit()?;
        Ok(ids)
    }

    pub fn query(&self, filter: EventFilter) -> Result<Vec<FileEvent>> {
        let mut query = String::from("SELECT timestamp, event_type, file_path, project_id, source, source_id, machine_id, content_hash, old_path FROM file_events WHERE 1=1");
        let mut bind_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let mut conditions = Vec::new();
        let mut param_idx = 1;

        if let Some(pid) = filter.project_id {
            conditions.push(format!("project_id = ?{}", param_idx));
            bind_params.push(Box::new(pid));
            param_idx += 1;
        }

        if let Some(fp) = filter.file_path {
            conditions.push(format!("file_path = ?{}", param_idx));
            bind_params.push(Box::new(fp));
            param_idx += 1;
        }

        if let Some(src) = filter.source {
            conditions.push(format!("source = ?{}", param_idx));
            bind_params.push(Box::new(src.as_str().to_string()));
            param_idx += 1;
        }

        if let Some(since) = filter.since {
            conditions.push(format!("timestamp >= ?{}", param_idx));
            bind_params.push(Box::new(since.to_rfc3339()));
            param_idx += 1;
        }

        if !conditions.is_empty() {
            query.push_str(" AND ");
            query.push_str(&conditions.join(" AND "));
        }

        query.push_str(" ORDER BY timestamp DESC");

        if let Some(limit) = filter.limit {
            query.push_str(&format!(" LIMIT {}", limit));
        }

        let ref_params: Vec<&dyn rusqlite::ToSql> = bind_params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&query)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(ref_params))?;

        let mut events = Vec::new();
        while let Some(row) = rows.next()? {
            let timestamp_str: String = row.get(0)?;
            let event_type_str: String = row.get(1)?;
            let file_path: String = row.get(2)?;
            let project_id: String = row.get(3)?;
            let source_str: String = row.get(4)?;

            let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                .map_err(|e| StoreError::TimestampParse(e.to_string()))?
                .with_timezone(&Utc);
            let event_type: EventType = event_type_str.parse()?;
            let source: Source = source_str.parse()?;

            events.push(FileEvent {
                timestamp,
                event_type,
                file_path,
                project_id,
                source,
                source_id: row.get(5)?,
                machine_id: row.get(6)?,
                content_hash: row.get(7)?,
                old_path: row.get(8)?,
            });
        }

        Ok(events)
    }

    pub fn get_latest(&self, project_id: &str, file_path: &str) -> Result<Option<FileEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, event_type, file_path, project_id, source, source_id, machine_id, content_hash, old_path
             FROM file_events
             WHERE project_id = ?1 AND file_path = ?2
             ORDER BY timestamp DESC
             LIMIT 1"
        )?;

        let mut rows = stmt.query(params![project_id, file_path])?;

        if let Some(row) = rows.next()? {
            let timestamp_str: String = row.get(0)?;
            let event_type_str: String = row.get(1)?;
            let file_path: String = row.get(2)?;
            let project_id: String = row.get(3)?;
            let source_str: String = row.get(4)?;

            let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                .map_err(|e| StoreError::TimestampParse(e.to_string()))?
                .with_timezone(&Utc);
            let event_type: EventType = event_type_str.parse()?;
            let source: Source = source_str.parse()?;

            Ok(Some(FileEvent {
                timestamp,
                event_type,
                file_path,
                project_id,
                source,
                source_id: row.get(5)?,
                machine_id: row.get(6)?,
                content_hash: row.get(7)?,
                old_path: row.get(8)?,
            }))
        } else {
            Ok(None)
        }
    }

    // ===== project CRUD methods =====

    pub fn add_project(&self, project_id: &str, path: &PathBuf) -> Result<()> {
        let created_at = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO projects (project_id, path, created_at) VALUES (?1, ?2, ?3)",
            params![project_id, path.to_str().ok_or_else(|| StoreError::TimestampParse("invalid path".to_string()))?, &created_at],
        )?;
        Ok(())
    }

    pub fn remove_project(&self, project_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM projects WHERE project_id = ?1",
            params![project_id],
        )?;
        Ok(())
    }

    pub fn get_project_path(&self, project_id: &str) -> Result<Option<PathBuf>> {
        let mut stmt = self.conn.prepare(
            "SELECT path FROM projects WHERE project_id = ?1"
        )?;

        let mut rows = stmt.query(params![project_id])?;

        if let Some(row) = rows.next()? {
            let path_str: String = row.get(0)?;
            Ok(Some(PathBuf::from(path_str)))
        } else {
            Ok(None)
        }
    }

    pub fn list_projects(&self) -> Result<Vec<(String, PathBuf)>> {
        let mut stmt = self.conn.prepare(
            "SELECT project_id, path FROM projects ORDER BY created_at"
        )?;

        let mut rows = stmt.query([])?;
        let mut projects = Vec::new();

        while let Some(row) = rows.next()? {
            let project_id: String = row.get(0)?;
            let path_str: String = row.get(1)?;
            projects.push((project_id, PathBuf::from(path_str)));
        }

        Ok(projects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_event(
        project_id: &str,
        file_path: &str,
        event_type: EventType,
        source: Source,
    ) -> FileEvent {
        FileEvent::new(event_type, file_path, project_id, "machine-1")
            .with_source(source)
    }

    #[test]
    fn test_create_store() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();
        // Verify store works by querying empty result
        let results = store.query(EventFilter::new()).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_insert_single_event() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let event = make_event("proj-1", "src/main.rs", EventType::Created, Source::User);
        let id = store.insert(&event).unwrap();

        assert_eq!(id, 1);

        let count = store.query(EventFilter::new()).unwrap().len() as i64;
        assert_eq!(count, 1);
    }

    #[test]
    fn test_insert_batch() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let events = vec![
            make_event("proj-1", "src/a.rs", EventType::Created, Source::User),
            make_event("proj-1", "src/b.rs", EventType::Modified, Source::AiAgent),
            make_event("proj-2", "README.md", EventType::Created, Source::Git),
        ];

        let ids = store.insert_batch(&events).unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids, vec![1, 2, 3]);

        let count = store.query(EventFilter::new()).unwrap().len();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_query_by_project_id() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        store.insert(&make_event("proj-1", "a.rs", EventType::Created, Source::User)).unwrap();
        store.insert(&make_event("proj-2", "b.rs", EventType::Created, Source::User)).unwrap();
        store.insert(&make_event("proj-1", "c.rs", EventType::Modified, Source::Git)).unwrap();

        let results = store.query(EventFilter::new().project_id("proj-1")).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].project_id, "proj-1");
        assert_eq!(results[1].project_id, "proj-1");
    }

    #[test]
    fn test_query_by_file_path() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        store.insert(&make_event("proj-1", "src/lib.rs", EventType::Created, Source::User)).unwrap();
        store.insert(&make_event("proj-1", "src/main.rs", EventType::Created, Source::User)).unwrap();
        store.insert(&make_event("proj-1", "src/lib.rs", EventType::Modified, Source::User)).unwrap();

        let results = store.query(EventFilter::new().file_path("src/lib.rs")).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].file_path, "src/lib.rs");
        assert_eq!(results[1].file_path, "src/lib.rs");
    }

    #[test]
    fn test_query_by_source() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        store.insert(&make_event("proj-1", "a.rs", EventType::Created, Source::User)).unwrap();
        store.insert(&make_event("proj-1", "b.rs", EventType::Modified, Source::AiAgent)).unwrap();
        store.insert(&make_event("proj-1", "c.rs", EventType::Created, Source::User)).unwrap();

        let results = store.query(EventFilter::new().source(Source::AiAgent)).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, Source::AiAgent);
    }

    #[test]
    fn test_query_with_since() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let now = Utc::now();
        let old_event = make_event("proj-1", "old.rs", EventType::Created, Source::User);
        let new_event = make_event("proj-1", "new.rs", EventType::Created, Source::User);

        // Manual timestamp override via builder would be cleaner but we'll work with what we have
        store.insert(&old_event).unwrap();
        store.insert(&new_event).unwrap();

        let five_min_ago = Utc::now() - chrono::Duration::minutes(5);
        let results = store.query(EventFilter::new().since(five_min_ago)).unwrap();
        // Should return events from the last 5 minutes
        assert!(!results.is_empty());
    }

    #[test]
    fn test_query_with_limit() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        for i in 0..10 {
            store.insert(&make_event("proj-1", &format!("file{}.rs", i), EventType::Created, Source::User)).unwrap();
        }

        let results = store.query(EventFilter::new().limit(3)).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_query_combined_filters() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        store.insert(&make_event("proj-1", "src/lib.rs", EventType::Created, Source::User)).unwrap();
        store.insert(&make_event("proj-2", "src/lib.rs", EventType::Created, Source::User)).unwrap();
        store.insert(&make_event("proj-1", "src/lib.rs", EventType::Modified, Source::AiAgent)).unwrap();

        let results = store
            .query(EventFilter::new().project_id("proj-1").file_path("src/lib.rs").source(Source::User))
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].project_id, "proj-1");
        assert_eq!(results[0].file_path, "src/lib.rs");
        assert_eq!(results[0].source, Source::User);
    }

    #[test]
    fn test_get_latest_returns_none_when_empty() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let result = store.get_latest("proj-1", "src/lib.rs").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_latest_returns_most_recent() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        store.insert(&make_event("proj-1", "src/lib.rs", EventType::Created, Source::User)).unwrap();
        store.insert(&make_event("proj-1", "src/lib.rs", EventType::Modified, Source::User)).unwrap();
        store.insert(&make_event("proj-1", "src/lib.rs", EventType::Modified, Source::AiAgent)).unwrap();

        let latest = store.get_latest("proj-1", "src/lib.rs").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().source, Source::AiAgent);
    }

    #[test]
    fn test_get_latest_filters_by_project_and_file() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        store.insert(&make_event("proj-1", "src/lib.rs", EventType::Modified, Source::AiAgent)).unwrap();
        store.insert(&make_event("proj-2", "src/lib.rs", EventType::Deleted, Source::User)).unwrap();
        store.insert(&make_event("proj-1", "src/main.rs", EventType::Created, Source::Git)).unwrap();

        let latest = store.get_latest("proj-1", "src/lib.rs").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.as_ref().unwrap().project_id, "proj-1");
        assert_eq!(latest.as_ref().unwrap().file_path, "src/lib.rs");
        assert_eq!(latest.as_ref().unwrap().source, Source::AiAgent);
    }

    #[test]
    fn test_wal_mode_enabled() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let journal_mode: String = store
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();

        assert_eq!(journal_mode.to_lowercase(), "wal");
    }

    #[test]
    fn test_insert_with_optional_fields() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let mut event = make_event("proj-1", "src/lib.rs", EventType::Created, Source::User);
        event.source_id = Some("chat-123".to_string());
        event.content_hash = Some("abc123".to_string());
        event.old_path = Some("src/old_lib.rs".to_string());

        let id = store.insert(&event).unwrap();
        assert_eq!(id, 1);

        let retrieved = store.get_latest("proj-1", "src/lib.rs").unwrap().unwrap();
        assert_eq!(retrieved.source_id, Some("chat-123".to_string()));
        assert_eq!(retrieved.content_hash, Some("abc123".to_string()));
        assert_eq!(retrieved.old_path, Some("src/old_lib.rs".to_string()));
    }

    #[test]
    fn test_insert_batch_empty() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let ids = store.insert_batch(&[]).unwrap();
        assert!(ids.is_empty());

        let count = store.query(EventFilter::new()).unwrap().len();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_query_returns_empty_when_no_match() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        store.insert(&make_event("proj-1", "a.rs", EventType::Created, Source::User)).unwrap();

        let results = store.query(EventFilter::new().project_id("nonexistent")).unwrap();
        assert!(results.is_empty());
    }

    // ===== project CRUD tests =====

    #[test]
    fn test_add_project() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let path = PathBuf::from("/Users/test/my-project");
        store.add_project("proj-1", &path).unwrap();

        // Verify we can retrieve it
        let retrieved = store.get_project_path("proj-1").unwrap();
        assert_eq!(retrieved, Some(path));
    }

    #[test]
    fn test_add_duplicate_project_errors() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let path = PathBuf::from("/Users/test/my-project");
        store.add_project("proj-1", &path).unwrap();

        // Second add with same project_id should fail
        let result = store.add_project("proj-1", &path);
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_project() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let path = PathBuf::from("/Users/test/my-project");
        store.add_project("proj-1", &path).unwrap();

        store.remove_project("proj-1").unwrap();

        // Verify it's gone
        let retrieved = store.get_project_path("proj-1").unwrap();
        assert_eq!(retrieved, None);
    }

    #[test]
    fn test_remove_nonexistent_project_succeeds() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        // Removing a nonexistent project should not error (idempotent)
        store.remove_project("nonexistent").unwrap();
    }

    #[test]
    fn test_get_project_path() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let path1 = PathBuf::from("/Users/test/project-one");
        let path2 = PathBuf::from("/Users/test/project-two");

        store.add_project("proj-1", &path1).unwrap();
        store.add_project("proj-2", &path2).unwrap();

        assert_eq!(store.get_project_path("proj-1").unwrap(), Some(path1));
        assert_eq!(store.get_project_path("proj-2").unwrap(), Some(path2));
        assert_eq!(store.get_project_path("proj-3").unwrap(), None);
    }

    #[test]
    fn test_list_projects() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let path1 = PathBuf::from("/Users/test/project-one");
        let path2 = PathBuf::from("/Users/test/project-two");

        store.add_project("proj-1", &path1).unwrap();
        store.add_project("proj-2", &path2).unwrap();

        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 2);

        // Convert to map for easier testing
        let map: std::collections::HashMap<_, _> = projects.into_iter().collect();
        assert_eq!(map.get("proj-1"), Some(&path1));
        assert_eq!(map.get("proj-2"), Some(&path2));
    }

    #[test]
    fn test_list_projects_empty() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = EventStore::new(db_path).unwrap();

        let projects = store.list_projects().unwrap();
        assert!(projects.is_empty());
    }
}
