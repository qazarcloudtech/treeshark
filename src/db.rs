use anyhow::{Context, Result};
use colored::*;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

const DB_FILE: &str = "treeshark.db";

/// File status in the database
#[derive(Debug, Clone, PartialEq)]
pub enum FileStatus {
    Exists,
    Deleted,
    Missing,
    Moved,
}

impl FileStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "deleted" => Self::Deleted,
            "missing" => Self::Missing,
            "moved" => Self::Moved,
            _ => Self::Exists,
        }
    }

    pub fn colored(&self) -> String {
        match self {
            Self::Exists => "exists".green().to_string(),
            Self::Deleted => "deleted".red().to_string(),
            Self::Missing => "missing".yellow().to_string(),
            Self::Moved => "moved".blue().to_string(),
        }
    }
}

/// A file row from the database
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FileRow {
    pub path: String,
    pub size: u64,
    pub status: FileStatus,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub deleted_at: Option<String>,
    pub times_seen: u32,
    pub last_scan_id: i64,
    pub moved_to: Option<String>,
}

/// A scan session row
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ScanRow {
    pub id: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub min_size_bytes: u64,
    pub scan_paths: String,
    pub completed_paths: String,
    pub threads_used: u32,
    pub total_scanned: u64,
    pub files_found: u64,
    pub files_per_sec: u64,
    pub duration_secs: f64,
    pub status: String,
}

/// Database handle
pub struct Db {
    pub conn: Connection,
    pub path: PathBuf,
}

impl Db {
    pub fn db_path(base: &Path) -> PathBuf {
        base.join(DB_FILE)
    }

    pub fn open(base: &Path) -> Result<Self> {
        let path = Self::db_path(base);
        let conn = Connection::open(&path)
            .with_context(|| format!("Failed to open database: {}", path.display()))?;

        // Performance pragmas — maximize write throughput
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA cache_size = -64000;
             PRAGMA temp_store = MEMORY;
             PRAGMA busy_timeout = 5000;",
        )?;

        let db = Db { conn, path };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS scans (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at      TEXT    NOT NULL,
                finished_at     TEXT,
                min_size_bytes  INTEGER NOT NULL,
                scan_paths      TEXT    NOT NULL DEFAULT '[]',
                completed_paths TEXT    NOT NULL DEFAULT '[]',
                threads_used    INTEGER NOT NULL DEFAULT 1,
                total_scanned   INTEGER NOT NULL DEFAULT 0,
                files_found     INTEGER NOT NULL DEFAULT 0,
                files_per_sec   INTEGER NOT NULL DEFAULT 0,
                duration_secs   REAL    NOT NULL DEFAULT 0,
                status          TEXT    NOT NULL DEFAULT 'running'
             );

             CREATE TABLE IF NOT EXISTS files (
                path            TEXT    PRIMARY KEY,
                size            INTEGER NOT NULL,
                status          TEXT    NOT NULL DEFAULT 'exists',
                first_seen_at   TEXT    NOT NULL,
                last_seen_at    TEXT    NOT NULL,
                first_scan_id   INTEGER NOT NULL,
                last_scan_id    INTEGER NOT NULL,
                deleted_at      TEXT,
                times_seen      INTEGER NOT NULL DEFAULT 1
             );

             CREATE INDEX IF NOT EXISTS idx_files_size   ON files(size DESC);
             CREATE INDEX IF NOT EXISTS idx_files_status ON files(status);",
        )?;

        // Migration: add moved_to column for move tracking
        self.conn
            .execute_batch("ALTER TABLE files ADD COLUMN moved_to TEXT;")
            .ok();

        Ok(())
    }

    // ─── Scan operations ────────────────────────────────────

    pub fn create_scan(
        &self,
        min_size_bytes: u64,
        scan_paths: &[String],
        threads: u32,
    ) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        let paths_json = serde_json::to_string(scan_paths)?;
        self.conn.execute(
            "INSERT INTO scans (started_at, min_size_bytes, scan_paths, threads_used, status)
             VALUES (?1, ?2, ?3, ?4, 'running')",
            params![now, min_size_bytes as i64, paths_json, threads],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn finish_scan(
        &self,
        scan_id: i64,
        total_scanned: u64,
        files_found: u64,
        files_per_sec: u64,
        duration_secs: f64,
        status: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE scans SET finished_at = ?1, total_scanned = ?2, files_found = ?3,
             files_per_sec = ?4, duration_secs = ?5, status = ?6 WHERE id = ?7",
            params![
                now,
                total_scanned as i64,
                files_found as i64,
                files_per_sec as i64,
                duration_secs,
                status,
                scan_id
            ],
        )?;
        Ok(())
    }

    pub fn mark_scan_path_completed(&self, scan_id: i64, completed_path: &str) -> Result<()> {
        let current: String = self
            .conn
            .query_row(
                "SELECT completed_paths FROM scans WHERE id = ?1",
                params![scan_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "[]".to_string());

        let mut paths: Vec<String> = serde_json::from_str(&current).unwrap_or_default();
        if !paths.contains(&completed_path.to_string()) {
            paths.push(completed_path.to_string());
        }
        let updated = serde_json::to_string(&paths)?;
        self.conn.execute(
            "UPDATE scans SET completed_paths = ?1 WHERE id = ?2",
            params![updated, scan_id],
        )?;
        Ok(())
    }

    pub fn get_last_interrupted_scan(&self) -> Result<Option<ScanRow>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, started_at, finished_at, min_size_bytes, scan_paths,
                        completed_paths, threads_used, total_scanned, files_found,
                        files_per_sec, duration_secs, status
                 FROM scans WHERE status = 'interrupted'
                 ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok(ScanRow {
                        id: row.get(0)?,
                        started_at: row.get(1)?,
                        finished_at: row.get(2)?,
                        min_size_bytes: row.get::<_, i64>(3)? as u64,
                        scan_paths: row.get(4)?,
                        completed_paths: row.get(5)?,
                        threads_used: row.get(6)?,
                        total_scanned: row.get::<_, i64>(7)? as u64,
                        files_found: row.get::<_, i64>(8)? as u64,
                        files_per_sec: row.get::<_, i64>(9)? as u64,
                        duration_secs: row.get(10)?,
                        status: row.get(11)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn get_scan_history(&self, limit: usize) -> Result<Vec<ScanRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, started_at, finished_at, min_size_bytes, scan_paths,
                    completed_paths, threads_used, total_scanned, files_found,
                    files_per_sec, duration_secs, status
             FROM scans ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(ScanRow {
                    id: row.get(0)?,
                    started_at: row.get(1)?,
                    finished_at: row.get(2)?,
                    min_size_bytes: row.get::<_, i64>(3)? as u64,
                    scan_paths: row.get(4)?,
                    completed_paths: row.get(5)?,
                    threads_used: row.get(6)?,
                    total_scanned: row.get::<_, i64>(7)? as u64,
                    files_found: row.get::<_, i64>(8)? as u64,
                    files_per_sec: row.get::<_, i64>(9)? as u64,
                    duration_secs: row.get(10)?,
                    status: row.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ─── File operations ────────────────────────────────────

    /// Batch upsert for performance — wraps in a transaction.
    /// Returns (new_files, updated_files) count.
    pub fn upsert_files_batch(&self, files: &[(String, u64)], scan_id: i64) -> Result<(usize, usize)> {
        let tx = self.conn.unchecked_transaction()?;
        let now = chrono::Utc::now().to_rfc3339();
        let mut new_count = 0usize;
        let mut updated_count = 0usize;
        {
            let mut check_stmt = tx.prepare_cached(
                "SELECT 1 FROM files WHERE path = ?1",
            )?;
            let mut stmt = tx.prepare_cached(
                "INSERT INTO files (path, size, status, first_seen_at, last_seen_at, first_scan_id, last_scan_id, times_seen)
                 VALUES (?1, ?2, 'exists', ?3, ?3, ?4, ?4, 1)
                 ON CONFLICT(path) DO UPDATE SET
                    size = ?2,
                    status = 'exists',
                    last_seen_at = ?3,
                    last_scan_id = ?4,
                    deleted_at = NULL,
                    times_seen = times_seen + 1",
            )?;
            for (path, size) in files {
                let exists = check_stmt.query_row(params![path], |_| Ok(true)).unwrap_or(false);
                stmt.execute(params![path, *size as i64, now, scan_id])?;
                if exists {
                    updated_count += 1;
                } else {
                    new_count += 1;
                }
            }
        }
        tx.commit()?;
        Ok((new_count, updated_count))
    }

    /// Mark a file as deleted
    pub fn mark_deleted(&self, path: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE files SET status = 'deleted', deleted_at = ?1 WHERE path = ?2",
            params![now, path],
        )?;
        Ok(())
    }

    /// Mark a file as deleted by its moved_to path (used by purge)
    pub fn mark_deleted_by_moved_to(&self, moved_to: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE files SET status = 'deleted', deleted_at = ?1, moved_to = NULL WHERE moved_to = ?2",
            params![now, moved_to],
        )?;
        Ok(())
    }

    /// Mark a file as moved to a new location
    pub fn mark_moved(&self, original_path: &str, moved_to: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE files SET status = 'moved', deleted_at = ?1, moved_to = ?2 WHERE path = ?3",
            params![now, moved_to, original_path],
        )?;
        Ok(())
    }

    /// Get all moved files (for restore)
    pub fn get_moved_files(&self) -> Result<Vec<FileRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, size, status, first_seen_at, last_seen_at, deleted_at, times_seen, last_scan_id, moved_to
             FROM files WHERE status = 'moved' ORDER BY size DESC",
        )?;
        let rows = stmt
            .query_map([], Self::map_file_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Restore a moved file back to exists
    pub fn mark_restored(&self, path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE files SET status = 'exists', deleted_at = NULL, moved_to = NULL WHERE path = ?1",
            params![path],
        )?;
        Ok(())
    }

    /// Mark files not seen in current scan as 'missing' — only under the scanned paths.
    /// Files outside the scan scope are left untouched.
    /// Only marks files whose size >= min_size_bytes — files below the threshold
    /// simply weren't looked for, so we can't know if they're still there.
    pub fn mark_missing_from_scan(&self, scan_id: i64, scanned_paths: &[String], min_size_bytes: u64) -> Result<u64> {
        if scanned_paths.is_empty() {
            return Ok(0);
        }
        // Build path prefix conditions
        let path_clauses: Vec<String> = scanned_paths
            .iter()
            .enumerate()
            .map(|(i, _)| format!("path LIKE ?{} || '%'", i + 3))
            .collect();
        let sql = format!(
            "UPDATE files SET status = 'missing'
             WHERE status = 'exists' AND last_scan_id != ?1 AND size >= ?2 AND ({})",
            path_clauses.join(" OR ")
        );

        // Normalize paths with trailing /
        use rusqlite::types::ToSql;
        let mut sql_params: Vec<Box<dyn ToSql>> = Vec::new();
        sql_params.push(Box::new(scan_id));
        sql_params.push(Box::new(min_size_bytes as i64));
        for p in scanned_paths {
            let normalized = if p.ends_with('/') {
                p.clone()
            } else {
                format!("{}/", p)
            };
            sql_params.push(Box::new(normalized));
        }
        let refs: Vec<&dyn ToSql> = sql_params.iter().map(|b| b.as_ref()).collect();
        let changed = self
            .conn
            .execute(&sql, rusqlite::params_from_iter(refs))?;
        Ok(changed as u64)
    }

    /// Get top files by size, scoped to path prefixes, with optional status filter.
    /// If `path_prefixes` is empty, returns files from ALL paths (no filter).
    pub fn get_top_files(
        &self,
        limit: usize,
        status_filter: Option<&str>,
        path_prefixes: &[String],
    ) -> Result<Vec<FileRow>> {
        // Build WHERE clauses
        let mut conditions: Vec<String> = Vec::new();
        let mut bind_values: Vec<String> = Vec::new();

        // Status filter
        if let Some(status) = status_filter {
            conditions.push(format!("status = ?{}", bind_values.len() + 1));
            bind_values.push(status.to_string());
        }

        // Path prefix filter — files must start with one of the scan paths
        if !path_prefixes.is_empty() {
            let path_clauses: Vec<String> = path_prefixes
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let idx = bind_values.len() + i + 1;
                    format!("path LIKE ?{} || '%'", idx)
                })
                .collect();
            // Append trailing / to each prefix so /usr doesn't match /usr2
            for p in path_prefixes {
                let normalized = if p.ends_with('/') {
                    p.clone()
                } else {
                    format!("{}/", p)
                };
                bind_values.push(normalized);
            }
            conditions.push(format!("({})", path_clauses.join(" OR ")));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT path, size, status, first_seen_at, last_seen_at, deleted_at, times_seen, last_scan_id, moved_to
             FROM files {} ORDER BY size DESC LIMIT ?{}",
            where_clause,
            bind_values.len() + 1
        );

        let mut stmt = self.conn.prepare(&sql)?;

        // Bind all params dynamically
        use rusqlite::types::ToSql;
        let limit_val = limit as i64;
        let mut sql_params: Vec<&dyn ToSql> = Vec::new();
        for v in &bind_values {
            sql_params.push(v as &dyn ToSql);
        }
        sql_params.push(&limit_val as &dyn ToSql);

        let rows = stmt
            .query_map(rusqlite::params_from_iter(sql_params), Self::map_file_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn map_file_row(row: &rusqlite::Row) -> rusqlite::Result<FileRow> {
        Ok(FileRow {
            path: row.get(0)?,
            size: row.get::<_, i64>(1)? as u64,
            status: FileStatus::from_str(&row.get::<_, String>(2)?),
            first_seen_at: row.get(3)?,
            last_seen_at: row.get(4)?,
            deleted_at: row.get(5)?,
            times_seen: row.get(6)?,
            last_scan_id: row.get(7)?,
            moved_to: row.get(8)?,
        })
    }

    /// Get DB stats
    pub fn stats(&self) -> Result<DbStats> {
        let total_files: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE status = 'exists'",
            [],
            |r| r.get(0),
        )?;
        let deleted: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE status = 'deleted'",
            [],
            |r| r.get(0),
        )?;
        let missing: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE status = 'missing'",
            [],
            |r| r.get(0),
        )?;
        let moved: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE status = 'moved'",
            [],
            |r| r.get(0),
        )?;
        let total_size_exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT SUM(size) FROM files WHERE status = 'exists'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let total_size_deleted: Option<i64> = self
            .conn
            .query_row(
                "SELECT SUM(size) FROM files WHERE status = 'deleted'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let total_size_moved: Option<i64> = self
            .conn
            .query_row(
                "SELECT SUM(size) FROM files WHERE status = 'moved'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let total_scans: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM scans", [], |r| r.get(0))?;

        Ok(DbStats {
            total_files: total_files as u64,
            exists: exists as u64,
            deleted: deleted as u64,
            missing: missing as u64,
            moved: moved as u64,
            total_size_exists: total_size_exists.unwrap_or(0) as u64,
            total_size_deleted: total_size_deleted.unwrap_or(0) as u64,
            total_size_moved: total_size_moved.unwrap_or(0) as u64,
            total_scans: total_scans as u64,
        })
    }

    /// Wipe all data
    pub fn reset(&self) -> Result<()> {
        self.conn
            .execute_batch("DELETE FROM files; DELETE FROM scans;")?;
        self.conn.execute_batch("VACUUM;")?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct DbStats {
    pub total_files: u64,
    pub exists: u64,
    pub deleted: u64,
    pub missing: u64,
    pub moved: u64,
    pub total_size_exists: u64,
    pub total_size_deleted: u64,
    pub total_size_moved: u64,
    pub total_scans: u64,
}
