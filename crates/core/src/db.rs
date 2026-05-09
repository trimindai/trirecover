//! Per-session SQLite store. One file per scan session under `sessions/<uuid>.db`.
//!
//! Why per-session: sessions can be 50+ GB of metadata (millions of files)
//! and we want to be able to delete one without touching others.

use crate::error::{Error, Result};
use crate::types::{FileRecord, JobState, ScanProgress, SessionId};
use crate::{config, types::FileSource};
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SessionStore {
    pool: SqlitePool,
    path: PathBuf,
    id: SessionId,
}

impl SessionStore {
    /// Open (creating if needed) the session database for `id`.
    pub async fn open(id: SessionId) -> Result<Self> {
        let path = config::sessions_dir()?.join(format!("{}.db", id.0));
        let url = format!("sqlite://{}", path.display());

        let opts = SqliteConnectOptions::from_str(&url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .pragma("synchronous", "NORMAL")
            .pragma("foreign_keys", "ON")
            .pragma("temp_store", "MEMORY");

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(opts)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool, path, id })
    }

    #[must_use]
    pub fn id(&self) -> SessionId {
        self.id
    }

    #[must_use]
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub async fn record_session_open(&self, drive_path: &str, strategy: &str) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO session(id, drive_path, strategy, opened_at)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET reopened_at = excluded.opened_at"#,
        )
        .bind(self.id.0.to_string())
        .bind(drive_path)
        .bind(strategy)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_progress(&self, p: &ScanProgress) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO progress(session_id, state, sectors_scanned, sectors_total,
                                    files_found, bytes_recoverable, current_phase,
                                    bad_sectors_skipped, ts)
               VALUES (?,?,?,?,?,?,?,?,?)"#,
        )
        .bind(self.id.0.to_string())
        .bind(format!("{:?}", p.state).to_lowercase())
        .bind(i64::try_from(p.sectors_scanned).unwrap_or(i64::MAX))
        .bind(i64::try_from(p.sectors_total).unwrap_or(i64::MAX))
        .bind(i64::try_from(p.files_found).unwrap_or(i64::MAX))
        .bind(i64::try_from(p.bytes_recoverable).unwrap_or(i64::MAX))
        .bind(&p.current_phase)
        .bind(i64::try_from(p.bad_sectors_skipped).unwrap_or(i64::MAX))
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn last_progress(&self) -> Result<Option<ScanProgress>> {
        let row = sqlx::query(
            r#"SELECT state, sectors_scanned, sectors_total, files_found, bytes_recoverable,
                       current_phase, bad_sectors_skipped
               FROM progress WHERE session_id = ?
               ORDER BY ts DESC LIMIT 1"#,
        )
        .bind(self.id.0.to_string())
        .fetch_optional(&self.pool)
        .await?;

        let Some(r) = row else { return Ok(None) };

        let state_str: String = r.try_get("state")?;
        let state = match state_str.as_str() {
            "queued" => JobState::Queued,
            "running" => JobState::Running,
            "paused" => JobState::Paused,
            "finished" => JobState::Finished,
            "failed" => JobState::Failed,
            "cancelled" => JobState::Cancelled,
            _ => JobState::Queued,
        };

        Ok(Some(ScanProgress {
            job_id: crate::types::JobId::default(),
            state,
            sectors_scanned: row_u64(&r, "sectors_scanned")?,
            sectors_total: row_u64(&r, "sectors_total")?,
            files_found: row_u64(&r, "files_found")?,
            bytes_recoverable: row_u64(&r, "bytes_recoverable")?,
            eta_secs: None,
            current_phase: r.try_get("current_phase")?,
            bad_sectors_skipped: row_u64(&r, "bad_sectors_skipped")?,
        }))
    }

    /// Bulk insert. Caller is expected to batch ~64–256 records per call.
    pub async fn insert_files(&self, files: &[FileRecord]) -> Result<()> {
        if files.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for f in files {
            let source_json = serde_json::to_string(&f.source)
                .map_err(|e| Error::Session(format!("serialize source: {e}")))?;
            sqlx::query(
                r#"INSERT INTO file(id, name, path, kind, size_bytes, modified, recoverability,
                                   head_hex, source_json, found_at)
                   VALUES (?,?,?,?,?,?,?,?,?,?)
                   ON CONFLICT(id) DO NOTHING"#,
            )
            .bind(i64::try_from(f.id).unwrap_or(i64::MAX))
            .bind(&f.name)
            .bind(&f.path)
            .bind(format!("{:?}", f.kind).to_lowercase())
            .bind(i64::try_from(f.size_bytes).unwrap_or(i64::MAX))
            .bind(f.modified)
            .bind(i64::from(f.recoverability))
            .bind(&f.head_hex)
            .bind(&source_json)
            .bind(Utc::now())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Paged query for the results table.
    pub async fn query_files(
        &self,
        filter: &FileFilter,
        page: u64,
        page_size: u64,
    ) -> Result<Vec<FileRecord>> {
        let mut sql = String::from(
            "SELECT id, name, path, kind, size_bytes, modified, recoverability, head_hex, \
             source_json FROM file WHERE 1 = 1",
        );

        if filter.kind.is_some() {
            sql.push_str(" AND kind = ?");
        }
        if filter.min_size.is_some() {
            sql.push_str(" AND size_bytes >= ?");
        }
        if filter.name_like.is_some() {
            sql.push_str(" AND name LIKE ?");
        }
        sql.push_str(" ORDER BY id LIMIT ? OFFSET ?");

        let mut q = sqlx::query(&sql);
        if let Some(k) = &filter.kind {
            q = q.bind(k);
        }
        if let Some(s) = filter.min_size {
            q = q.bind(i64::try_from(s).unwrap_or(i64::MAX));
        }
        if let Some(n) = &filter.name_like {
            q = q.bind(format!("%{n}%"));
        }
        q = q
            .bind(i64::try_from(page_size).unwrap_or(1024))
            .bind(i64::try_from(page * page_size).unwrap_or(0));

        let rows = q.fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_file).collect()
    }

    pub async fn count_files(&self, filter: &FileFilter) -> Result<u64> {
        let mut sql = String::from("SELECT COUNT(*) AS n FROM file WHERE 1 = 1");
        if filter.kind.is_some() {
            sql.push_str(" AND kind = ?");
        }
        if filter.min_size.is_some() {
            sql.push_str(" AND size_bytes >= ?");
        }
        if filter.name_like.is_some() {
            sql.push_str(" AND name LIKE ?");
        }
        let mut q = sqlx::query(&sql);
        if let Some(k) = &filter.kind {
            q = q.bind(k);
        }
        if let Some(s) = filter.min_size {
            q = q.bind(i64::try_from(s).unwrap_or(i64::MAX));
        }
        if let Some(n) = &filter.name_like {
            q = q.bind(format!("%{n}%"));
        }
        let r = q.fetch_one(&self.pool).await?;
        let n: i64 = r.try_get("n")?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    pub async fn close(self) {
        self.pool.close().await;
    }
}

#[derive(Debug, Default, Clone)]
pub struct FileFilter {
    pub kind: Option<String>,
    pub min_size: Option<u64>,
    pub name_like: Option<String>,
}

fn row_u64(row: &sqlx::sqlite::SqliteRow, col: &str) -> Result<u64> {
    let v: i64 = row.try_get(col)?;
    Ok(u64::try_from(v).unwrap_or(0))
}

fn row_to_file(row: sqlx::sqlite::SqliteRow) -> Result<FileRecord> {
    let id: i64 = row.try_get("id")?;
    let name: String = row.try_get("name")?;
    let path: String = row.try_get("path")?;
    let kind_s: String = row.try_get("kind")?;
    let size: i64 = row.try_get("size_bytes")?;
    let modified: Option<DateTime<Utc>> = row.try_get("modified")?;
    let recoverability: i64 = row.try_get("recoverability")?;
    let head_hex: String = row.try_get("head_hex")?;
    let source_json: String = row.try_get("source_json")?;

    let kind = serde_json::from_value(serde_json::Value::String(kind_s))
        .map_err(|e| Error::Session(format!("kind parse: {e}")))?;
    let source: FileSource = serde_json::from_str(&source_json)
        .map_err(|e| Error::Session(format!("source parse: {e}")))?;

    Ok(FileRecord {
        id: u64::try_from(id).unwrap_or(0),
        name,
        path,
        kind,
        size_bytes: u64::try_from(size).unwrap_or(0),
        modified,
        source,
        recoverability: u8::try_from(recoverability.clamp(0, 100)).unwrap_or(0),
        head_hex,
    })
}
