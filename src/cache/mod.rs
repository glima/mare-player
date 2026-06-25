// SPDX-License-Identifier: MIT

//! Turso-backed cache database.
//!
//! A single embedded SQLite-compatible database (pure-Rust `turso`) that backs:
//!
//! * **view-state cache** — the last-seen row model of each view, so navigation
//!   paints instantly from cache and then upserts when the network responds
//!   (stale-while-revalidate);
//! * **image cache** — artwork blobs (replacing the on-disk image partition);
//! * **kv** — small odds and ends (play history, reviews, …).
//!
//! Songs are deliberately **not** stored here — large audio stays on the
//! filesystem so it can be streamed and seeked while still downloading. Videos
//! are never cached at all.
//!
//! ## Concurrency
//!
//! All access goes through a single [`turso::Connection`] behind a
//! `tokio::Mutex`. The cache is not a high-throughput hot path (a handful of
//! ops per navigation / image), so serialising keeps the model simple and
//! avoids relying on the young engine's concurrent-writer behaviour.
//!
//! ## Disposability
//!
//! Everything here is a cache: it can always be rebuilt from TIDAL. So instead
//! of migrations we stamp `PRAGMA user_version` and **drop + recreate** the
//! tables whenever [`SCHEMA_VERSION`] changes. That also de-risks running a
//! beta database engine — a corrupt or incompatible file just triggers a cold
//! refetch, never data loss.

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;
use turso::Builder;

/// Bump to invalidate (drop + recreate) all cached tables.
const SCHEMA_VERSION: i64 = 1;

/// Handle to the cache database. Cheap to clone (shared connection).
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<turso::Connection>>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `turso::Connection` isn't `Debug`; the handle has no useful fields to
        // print anyway. This impl exists so `Db` can ride inside `Message`.
        f.debug_struct("Db").finish_non_exhaustive()
    }
}

/// Seconds since the Unix epoch, used for LRU `accessed_at` / `updated_at`.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Db {
    /// Open (or create) the cache database at `path`.
    ///
    /// On a [`SCHEMA_VERSION`] mismatch the cached tables are dropped and
    /// recreated. Pass `:memory:` for an ephemeral database (used in tests).
    pub async fn open(path: &Path) -> turso::Result<Self> {
        let path_str = path.to_str().unwrap_or(":memory:");
        let db = Builder::new_local(path_str).build().await?;
        let conn = db.connect()?;
        let me = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        me.init_schema().await?;
        Ok(me)
    }

    async fn init_schema(&self) -> turso::Result<()> {
        let conn = self.conn.lock().await;

        let mut ver: i64 = 0;
        {
            let mut rows = conn.query("PRAGMA user_version", ()).await?;
            if let Some(row) = rows.next().await? {
                ver = row.get_value(0)?.as_integer().copied().unwrap_or(0);
            }
        }

        if ver != SCHEMA_VERSION {
            for t in ["view_cache", "image", "kv"] {
                let _ = conn.execute(&format!("DROP TABLE IF EXISTS {t}"), ()).await;
            }
        }

        conn.execute(
            "CREATE TABLE IF NOT EXISTS view_cache (
                key         TEXT PRIMARY KEY,
                payload     BLOB NOT NULL,
                etag        TEXT,
                bytes       INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL,
                accessed_at INTEGER NOT NULL
            )",
            (),
        )
        .await?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS image (
                url         TEXT PRIMARY KEY,
                data        BLOB NOT NULL,
                bytes       INTEGER NOT NULL,
                accessed_at INTEGER NOT NULL
            )",
            (),
        )
        .await?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS kv (
                k TEXT PRIMARY KEY,
                v BLOB NOT NULL
            )",
            (),
        )
        .await?;

        conn.execute(&format!("PRAGMA user_version = {SCHEMA_VERSION}"), ())
            .await?;
        Ok(())
    }

    // ── view-state cache ────────────────────────────────────────────────

    /// Fetch a cached view payload by key, bumping its LRU timestamp.
    pub async fn get_view(&self, key: &str) -> Option<Vec<u8>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query("SELECT payload FROM view_cache WHERE key = ?1", [key])
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        let data = row.get_value(0).ok()?.as_blob().cloned()?;
        let _ = conn
            .execute(
                "UPDATE view_cache SET accessed_at = ?1 WHERE key = ?2",
                (now_secs(), key),
            )
            .await;
        Some(data)
    }

    /// Upsert a view payload, then evict oldest entries past `budget_bytes`.
    pub async fn put_view(&self, key: &str, payload: &[u8], etag: Option<&str>, budget_bytes: i64) {
        let now = now_secs();
        let conn = self.conn.lock().await;
        let etag = etag.map(|s| s.to_string());
        let res = conn
            .execute(
                "INSERT INTO view_cache (key, payload, etag, bytes, updated_at, accessed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(key) DO UPDATE SET
                    payload = excluded.payload,
                    etag = excluded.etag,
                    bytes = excluded.bytes,
                    updated_at = excluded.updated_at,
                    accessed_at = excluded.accessed_at",
                (key, payload.to_vec(), etag, payload.len() as i64, now),
            )
            .await;
        if let Err(e) = res {
            tracing::warn!("cache put_view failed: {e}");
            return;
        }
        Self::enforce_budget(&conn, "view_cache", budget_bytes).await;
    }

    // ── image cache ─────────────────────────────────────────────────────

    /// Fetch a cached image by URL, bumping its LRU timestamp.
    pub async fn get_image(&self, url: &str) -> Option<Vec<u8>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query("SELECT data FROM image WHERE url = ?1", [url])
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        let data = row.get_value(0).ok()?.as_blob().cloned()?;
        let _ = conn
            .execute(
                "UPDATE image SET accessed_at = ?1 WHERE url = ?2",
                (now_secs(), url),
            )
            .await;
        Some(data)
    }

    /// Upsert an image blob, then evict oldest entries past `budget_bytes`.
    pub async fn put_image(&self, url: &str, data: &[u8], budget_bytes: i64) {
        let now = now_secs();
        let conn = self.conn.lock().await;
        let res = conn
            .execute(
                "INSERT INTO image (url, data, bytes, accessed_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(url) DO UPDATE SET
                    data = excluded.data,
                    bytes = excluded.bytes,
                    accessed_at = excluded.accessed_at",
                (url, data.to_vec(), data.len() as i64, now),
            )
            .await;
        if let Err(e) = res {
            tracing::warn!("cache put_image failed: {e}");
            return;
        }
        Self::enforce_budget(&conn, "image", budget_bytes).await;
    }

    // ── small key/value ─────────────────────────────────────────────────

    /// Fetch a raw kv blob.
    pub async fn get_kv(&self, k: &str) -> Option<Vec<u8>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query("SELECT v FROM kv WHERE k = ?1", [k])
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        row.get_value(0).ok()?.as_blob().cloned()
    }

    /// Upsert a raw kv blob.
    pub async fn put_kv(&self, k: &str, v: &[u8]) {
        let conn = self.conn.lock().await;
        let res = conn
            .execute(
                "INSERT INTO kv (k, v) VALUES (?1, ?2)
                 ON CONFLICT(k) DO UPDATE SET v = excluded.v",
                (k, v.to_vec()),
            )
            .await;
        if let Err(e) = res {
            tracing::warn!("cache put_kv failed: {e}");
        }
    }

    // ── eviction ────────────────────────────────────────────────────────

    /// Evict the oldest rows (by `accessed_at`) from `table` until the total
    /// `bytes` fits under `budget_bytes`. Window-function-free so it works on
    /// the current engine: drop the single oldest row, repeat.
    async fn enforce_budget(conn: &turso::Connection, table: &str, budget_bytes: i64) {
        if budget_bytes <= 0 {
            return;
        }
        loop {
            let total: i64 = match conn
                .query(&format!("SELECT COALESCE(SUM(bytes), 0) FROM {table}"), ())
                .await
            {
                Ok(mut rows) => match rows.next().await {
                    Ok(Some(row)) => row
                        .get_value(0)
                        .ok()
                        .and_then(|v| v.as_integer().copied())
                        .unwrap_or(0),
                    _ => 0,
                },
                Err(_) => return,
            };
            if total <= budget_bytes {
                return;
            }
            let deleted = conn
                .execute(
                    &format!(
                        "DELETE FROM {table} WHERE rowid = (
                            SELECT rowid FROM {table} ORDER BY accessed_at ASC LIMIT 1
                        )"
                    ),
                    (),
                )
                .await
                .unwrap_or(0);
            if deleted == 0 {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_db() -> Db {
        Db::open(Path::new(":memory:")).await.expect("open db")
    }

    #[tokio::test]
    async fn view_blob_round_trips() {
        let db = mem_db().await;
        let payload = vec![0u8, 1, 2, 3, 250, 251, 252, 253];
        db.put_view("album:42", &payload, Some("etag-1"), 1024 * 1024)
            .await;
        let got = db.get_view("album:42").await;
        assert_eq!(got.as_deref(), Some(payload.as_slice()));
        assert!(db.get_view("album:nope").await.is_none());
    }

    #[tokio::test]
    async fn view_upsert_replaces() {
        let db = mem_db().await;
        db.put_view("k", b"first", None, 1024 * 1024).await;
        db.put_view("k", b"second", None, 1024 * 1024).await;
        assert_eq!(db.get_view("k").await.as_deref(), Some(&b"second"[..]));
    }

    #[tokio::test]
    async fn image_round_trips() {
        let db = mem_db().await;
        db.put_image("https://x/y.jpg", &[9u8; 64], 1024 * 1024)
            .await;
        assert_eq!(
            db.get_image("https://x/y.jpg").await.map(|d| d.len()),
            Some(64)
        );
    }

    #[tokio::test]
    async fn kv_round_trips() {
        let db = mem_db().await;
        db.put_kv("history", b"[1,2,3]").await;
        assert_eq!(db.get_kv("history").await.as_deref(), Some(&b"[1,2,3]"[..]));
    }
}
