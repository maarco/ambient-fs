use ambient_fs_core::analysis::FileAnalysis;
use rusqlite::{params, Connection, Result as SqliteResult};
use std::path::Path;

/// Cache for file analysis results backed by SQLite
pub struct FileAnalysisCache {
    conn: Connection,
}

impl FileAnalysisCache {
    /// Open cache at given db path, creates schema if needed
    pub fn open<P: AsRef<Path>>(path: P) -> SqliteResult<Self> {
        let conn = Connection::open(path)?;
        let cache = Self { conn };
        cache.init_schema()?;
        Ok(cache)
    }

    /// Open in-memory cache for testing
    pub fn in_memory() -> SqliteResult<Self> {
        let conn = Connection::open_in_memory()?;
        let cache = Self { conn };
        cache.init_schema()?;
        Ok(cache)
    }

    fn init_schema(&self) -> SqliteResult<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS file_analysis (
                file_path    TEXT NOT NULL,
                project_id   TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                analyzed_at  INTEGER NOT NULL,
                exports      TEXT,
                imports      TEXT,
                todo_count   INTEGER DEFAULT 0,
                lint_hints   TEXT,
                line_count   INTEGER DEFAULT 0,
                PRIMARY KEY (project_id, file_path)
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_analysis_hash
             ON file_analysis(project_id, content_hash)",
            [],
        )?;
        Ok(())
    }

    /// Get cached analysis for a specific file
    pub fn get(
        &self,
        project_id: &str,
        file_path: &str,
    ) -> SqliteResult<Option<FileAnalysis>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, project_id, content_hash, exports, imports,
                    todo_count, lint_hints, line_count
             FROM file_analysis
             WHERE project_id = ? AND file_path = ?",
        )?;

        let mut rows = stmt.query(params![project_id, file_path])?;

        if let Some(row) = rows.next()? {
            let exports_json: String = row.get(3)?;
            let imports_json: String = row.get(4)?;
            let lint_hints_json: String = row.get(6)?;

            Ok(Some(FileAnalysis {
                file_path: row.get(0)?,
                project_id: row.get(1)?,
                content_hash: row.get(2)?,
                exports: serde_json::from_str(&exports_json).unwrap_or_default(),
                imports: serde_json::from_str(&imports_json).unwrap_or_default(),
                todo_count: row.get(5)?,
                lint_hints: serde_json::from_str(&lint_hints_json).unwrap_or_default(),
                line_count: row.get(7)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Store analysis result in cache (upsert by project_id + file_path)
    pub fn put(&self, analysis: &FileAnalysis) -> SqliteResult<()> {
        let exports_json = serde_json::to_string(&analysis.exports).unwrap_or_default();
        let imports_json = serde_json::to_string(&analysis.imports).unwrap_or_default();
        let lint_hints_json = serde_json::to_string(&analysis.lint_hints).unwrap_or_default();

        self.conn.execute(
            "INSERT INTO file_analysis
                (file_path, project_id, content_hash, analyzed_at, exports, imports,
                 todo_count, lint_hints, line_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (project_id, file_path)
             DO UPDATE SET
                 content_hash = excluded.content_hash,
                 analyzed_at = excluded.analyzed_at,
                 exports = excluded.exports,
                 imports = excluded.imports,
                 todo_count = excluded.todo_count,
                 lint_hints = excluded.lint_hints,
                 line_count = excluded.line_count",
            params![
                analysis.file_path,
                analysis.project_id,
                analysis.content_hash,
                chrono::Utc::now().timestamp(),
                exports_json,
                imports_json,
                analysis.todo_count,
                lint_hints_json,
                analysis.line_count,
            ],
        )?;
        Ok(())
    }

    /// Remove cached analysis for a file
    pub fn invalidate(&self, project_id: &str, file_path: &str) -> SqliteResult<()> {
        self.conn.execute(
            "DELETE FROM file_analysis WHERE project_id = ? AND file_path = ?",
            params![project_id, file_path],
        )?;
        Ok(())
    }

    /// Find any file with matching content hash (useful for copy detection)
    pub fn get_by_hash(
        &self,
        project_id: &str,
        content_hash: &str,
    ) -> SqliteResult<Option<FileAnalysis>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, project_id, content_hash, exports, imports,
                    todo_count, lint_hints, line_count
             FROM file_analysis
             WHERE project_id = ? AND content_hash = ?
             LIMIT 1",
        )?;

        let mut rows = stmt.query(params![project_id, content_hash])?;

        if let Some(row) = rows.next()? {
            let exports_json: String = row.get(3)?;
            let imports_json: String = row.get(4)?;
            let lint_hints_json: String = row.get(6)?;

            Ok(Some(FileAnalysis {
                file_path: row.get(0)?,
                project_id: row.get(1)?,
                content_hash: row.get(2)?,
                exports: serde_json::from_str(&exports_json).unwrap_or_default(),
                imports: serde_json::from_str(&imports_json).unwrap_or_default(),
                todo_count: row.get(5)?,
                lint_hints: serde_json::from_str(&lint_hints_json).unwrap_or_default(),
                line_count: row.get(7)?,
            }))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ambient_fs_core::analysis::{ImportRef, LintHint, LintSeverity};
    use pretty_assertions::assert_eq;

    fn make_test_analysis(file_path: &str, content_hash: &str) -> FileAnalysis {
        FileAnalysis {
            file_path: file_path.to_string(),
            project_id: "test-project".to_string(),
            content_hash: content_hash.to_string(),
            exports: vec!["Foo".to_string(), "Bar".to_string()],
            imports: vec![ImportRef {
                path: "baz".to_string(),
                symbols: vec!["Qux".to_string()],
                line: 1,
            }],
            todo_count: 3,
            lint_hints: vec![LintHint {
                line: 10,
                column: 5,
                severity: LintSeverity::Warning,
                message: "unused".to_string(),
                rule: Some("dead_code".to_string()),
            }],
            line_count: 42,
        }
    }

    #[test]
    fn cache_put_then_get_returns_same_analysis() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        let analysis = make_test_analysis("src/lib.rs", "hash123");

        cache.put(&analysis).unwrap();
        let retrieved = cache.get("test-project", "src/lib.rs").unwrap();

        assert_eq!(Some(analysis), retrieved);
    }

    #[test]
    fn cache_get_missing_returns_none() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        let result = cache.get("test-project", "nonexistent.rs").unwrap();
        assert_eq!(None, result);
    }

    #[test]
    fn cache_put_overwrites_existing() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        let original = make_test_analysis("src/lib.rs", "hash1");
        let updated = FileAnalysis {
            todo_count: 99,
            line_count: 200,
            ..original.clone()
        };

        cache.put(&original).unwrap();
        cache.put(&updated).unwrap();

        let retrieved = cache.get("test-project", "src/lib.rs").unwrap().unwrap();
        assert_eq!(retrieved.todo_count, 99);
        assert_eq!(retrieved.line_count, 200);
    }

    #[test]
    fn cache_invalidated_file_returns_none() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        let analysis = make_test_analysis("src/lib.rs", "hash123");

        cache.put(&analysis).unwrap();
        cache.invalidate("test-project", "src/lib.rs").unwrap();

        let result = cache.get("test-project", "src/lib.rs").unwrap();
        assert_eq!(None, result);
    }

    #[test]
    fn cache_invalidate_nonexistent_is_ok() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        // should not error
        let result = cache.invalidate("test-project", "nonexistent.rs");
        assert!(result.is_ok());
    }

    #[test]
    fn cache_get_by_hash_finds_duplicate_content() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        let analysis1 = make_test_analysis("src/lib.rs", "same_hash");
        let analysis2 = make_test_analysis("src/copy.rs", "same_hash");

        cache.put(&analysis1).unwrap();
        cache.put(&analysis2).unwrap();

        let found = cache.get_by_hash("test-project", "same_hash").unwrap();
        // should find one of them (order not guaranteed)
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.content_hash, "same_hash");
    }

    #[test]
    fn cache_get_by_hash_missing_returns_none() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        let result = cache.get_by_hash("test-project", "nosuchhash").unwrap();
        assert_eq!(None, result);
    }

    #[test]
    fn cache_get_by_hash_respects_project_id() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        let analysis = make_test_analysis("src/lib.rs", "hash123");
        cache.put(&analysis).unwrap();

        // different project, same hash = not found
        let result = cache.get_by_hash("other-project", "hash123").unwrap();
        assert_eq!(None, result);
    }

    #[test]
    fn cache_handles_empty_analysis() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        let analysis = FileAnalysis::empty("src/empty.rs", "test-project", "empty_hash");

        cache.put(&analysis).unwrap();
        let retrieved = cache.get("test-project", "src/empty.rs").unwrap().unwrap();

        assert_eq!(retrieved.file_path, "src/empty.rs");
        assert!(retrieved.exports.is_empty());
        assert!(retrieved.imports.is_empty());
        assert_eq!(retrieved.todo_count, 0);
        assert_eq!(retrieved.line_count, 0);
    }

    #[test]
    fn cache_multiple_projects_dont_collide() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        let a1 = FileAnalysis::empty("lib.rs", "proj-a", "hash-a");
        let a2 = FileAnalysis::empty("lib.rs", "proj-b", "hash-b");

        cache.put(&a1).unwrap();
        cache.put(&a2).unwrap();

        let r1 = cache.get("proj-a", "lib.rs").unwrap().unwrap();
        let r2 = cache.get("proj-b", "lib.rs").unwrap().unwrap();

        assert_eq!(r1.project_id, "proj-a");
        assert_eq!(r2.project_id, "proj-b");
        assert_eq!(r1.content_hash, "hash-a");
        assert_eq!(r2.content_hash, "hash-b");
    }

    #[test]
    fn cache_schema_idempotent() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        // calling init again should be fine
        cache.init_schema().unwrap();
        cache.init_schema().unwrap();

        // verify it still works
        let analysis = make_test_analysis("src/lib.rs", "hash123");
        cache.put(&analysis).unwrap();
        assert!(cache.get("test-project", "src/lib.rs").unwrap().is_some());
    }

    #[test]
    fn cache_serdes_roundtrip_preserves_all_fields() {
        let cache = FileAnalysisCache::in_memory().unwrap();
        let analysis = make_test_analysis("src/complex.rs", "hash-complex");

        cache.put(&analysis).unwrap();
        let retrieved = cache.get("test-project", "src/complex.rs").unwrap().unwrap();

        assert_eq!(analysis.file_path, retrieved.file_path);
        assert_eq!(analysis.project_id, retrieved.project_id);
        assert_eq!(analysis.content_hash, retrieved.content_hash);
        assert_eq!(analysis.exports, retrieved.exports);
        assert_eq!(analysis.imports, retrieved.imports);
        assert_eq!(analysis.todo_count, retrieved.todo_count);
        assert_eq!(analysis.lint_hints, retrieved.lint_hints);
        assert_eq!(analysis.line_count, retrieved.line_count);
    }
}
