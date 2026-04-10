use crate::models::CheckResult;
use chrono::{TimeZone, Utc};
use rusqlite::{Connection, Result, params};
use std::path::Path;
use thiserror::Error;

#[cfg(test)]
mod tests;

#[derive(Error, Debug)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;
        Self::configure_and_migrate(conn)
    }

    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        Self::configure_and_migrate(conn)
    }

    fn configure_and_migrate(conn: Connection) -> Result<Self, DbError> {
        // WAL mode + minimal disk pressure
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA cache_size = -8000;
             PRAGMA temp_store = MEMORY;
             PRAGMA mmap_size = 134217728;",
        )?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), DbError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO schema_version (version) VALUES (0);",
        )?;

        let version: i64 = self
            .conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))?;

        if version < 1 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS checks (
                    id         INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts         INTEGER NOT NULL,
                    host       TEXT    NOT NULL,
                    ok         INTEGER NOT NULL,
                    latency_ms INTEGER NOT NULL,
                    source     TEXT    NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_checks_ts        ON checks (ts);
                CREATE INDEX IF NOT EXISTS idx_checks_host_ts   ON checks (host, ts);
                UPDATE schema_version SET version = 1;",
            )?;
        }

        Ok(())
    }

    pub fn insert(&self, result: &CheckResult) -> Result<(), DbError> {
        let ts = result.timestamp.timestamp_millis();
        let ok = result.ok as i32;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO checks (ts, host, ok, latency_ms, source) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        stmt.execute(params![
            ts,
            result.host,
            ok,
            result.latency_ms,
            result.source
        ])?;
        Ok(())
    }

    /// Returns the most recent CheckResult per host within the last `since_hours` hours.
    pub fn latest_status(&self, since_hours: u32) -> Result<Vec<CheckResult>, DbError> {
        let cutoff = Utc::now().timestamp_millis() - since_hours as i64 * 3_600_000;
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, ts, host, ok, latency_ms, source
             FROM checks
             WHERE ts > ?1
               AND id IN (
                   SELECT MAX(id) FROM checks WHERE ts > ?1 GROUP BY host
               )
             ORDER BY host",
        )?;
        let rows = stmt.query_map(params![cutoff], row_to_check)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Returns the last `limit` results for a given host, newest first.
    pub fn history(&self, host: &str, limit: u32) -> Result<Vec<CheckResult>, DbError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, ts, host, ok, latency_ms, source
             FROM checks
             WHERE host = ?1
             ORDER BY ts DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![host, limit], row_to_check)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Deletes all records older than `older_than_days` days.
    pub fn cleanup(&self, older_than_days: u32) -> Result<u64, DbError> {
        let cutoff = Utc::now().timestamp_millis() - older_than_days as i64 * 86_400_000;
        let mut stmt = self
            .conn
            .prepare_cached("DELETE FROM checks WHERE ts < ?1")?;
        let deleted = stmt.execute(params![cutoff])?;
        Ok(deleted as u64)
    }
}

fn row_to_check(row: &rusqlite::Row<'_>) -> rusqlite::Result<CheckResult> {
    let ts_ms: i64 = row.get(1)?;
    let ok: i32 = row.get(3)?;
    Ok(CheckResult {
        host: row.get(2)?,
        ok: ok != 0,
        latency_ms: row.get(4)?,
        timestamp: Utc.timestamp_millis_opt(ts_ms).single().unwrap_or_default(),
        source: row.get(5)?,
    })
}
