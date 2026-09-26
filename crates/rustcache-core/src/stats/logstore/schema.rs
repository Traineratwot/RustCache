//! SQLite schema bootstrap and file-permission helpers for the request-log database.

use std::path::Path;

use rusqlite::Connection;

/// Restrict the database file to owner-only access (0600) on unix.
pub(super) fn set_db_mode(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Apply journal pragmas and create the `requests` table and indexes if missing.
pub(super) fn init_schema(conn: &Connection) -> anyhow::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS requests (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            ts          INTEGER NOT NULL,
            method      TEXT    NOT NULL,
            url         TEXT    NOT NULL,
            host        TEXT    NOT NULL,
            status      INTEGER NOT NULL,
            outcome     TEXT    NOT NULL,
            duration_ms INTEGER NOT NULL,
            resp_bytes  INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_requests_ts      ON requests(ts DESC);
        CREATE INDEX IF NOT EXISTS idx_requests_outcome ON requests(outcome);
        CREATE INDEX IF NOT EXISTS idx_requests_host    ON requests(host);
        CREATE INDEX IF NOT EXISTS idx_requests_method  ON requests(method);
        "#,
    )?;
    Ok(())
}

/// Optional peek used by tests to verify schema objects.
#[cfg(test)]
pub(super) fn table_exists(conn: &Connection, name: &str) -> anyhow::Result<bool> {
    use rusqlite::{params, OptionalExtension};
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Optional peek used by tests to verify schema objects.
#[cfg(test)]
pub(super) fn index_exists(conn: &Connection, name: &str) -> anyhow::Result<bool> {
    use rusqlite::{params, OptionalExtension};
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}
