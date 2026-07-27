// SPDX-License-Identifier: MIT

//! View-state cache helpers (stale-while-revalidate).
//!
//! Each browsable view keeps a serialized snapshot of its last-seen row model
//! in the cache database. On a cold open (e.g. after a restart) the snapshot is
//! read and painted **instantly** while the network request refreshes it in the
//! background — so the user never stares at a spinner for content they've
//! already seen.
//!
//! The split is deliberate:
//!
//! * **writes** happen inside each `load_*` task (the network path only), via
//!   [`cache_put`], so the cache-hit path never needlessly rewrites identical
//!   data;
//! * **reads** happen in the view's show-handler via [`AppModel::read_view_cache`],
//!   which feeds the cached payload straight into the view's existing "Loaded"
//!   message so all the painting logic is reused.

use cosmic::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::cache::Db;
use crate::messages::Message;
use crate::state::AppModel;

/// Maximum serialized size of a single cached view payload (1 MiB). Larger
/// views are simply not cached — this tier is for snappy paint, not bulk
/// storage.
const MAX_VIEW_ENTRY_BYTES: i64 = 1024 * 1024;

/// Total byte budget across all cached view payloads (LRU-evicted).
const VIEW_CACHE_BUDGET_BYTES: i64 = 16 * 1024 * 1024;

/// Serialize `payload` and write it into the view cache under `key`.
///
/// Best-effort: a missing database, a serialization failure, or an oversized
/// payload are all silently skipped. Intended to be `.await`ed at the tail of a
/// `load_*` task's async block, just before the result is handed to the UI.
pub(crate) async fn cache_put<T: Serialize>(db: Option<Db>, key: &str, payload: &T) {
    let Some(db) = db else {
        return;
    };
    let Ok(bytes) = serde_json::to_vec(payload) else {
        return;
    };
    if bytes.len() as i64 > MAX_VIEW_ENTRY_BYTES {
        tracing::debug!("view cache: skipping oversized payload for {key} ({} bytes)", bytes.len());
        return;
    }
    db.put_view(key, &bytes, None, VIEW_CACHE_BUDGET_BYTES).await;
}

impl AppModel {
    /// Build a task that reads the cached payload for `key` and, on hit, feeds
    /// it into `on_hit` (typically the view's existing "Loaded" message) so the
    /// view paints instantly from its last-seen state.
    ///
    /// On a cache miss — or when the database hasn't finished opening yet — it
    /// resolves to [`Message::Noop`], leaving the view in its loading state
    /// until the network responds.
    pub(crate) fn read_view_cache<T, F>(&self, key: impl Into<String>, on_hit: F) -> Task<cosmic::Action<Message>>
    where
        T: DeserializeOwned + Send + 'static,
        F: Fn(T) -> Message + Send + 'static,
    {
        let Some(db) = self.cache_db.clone() else {
            return Task::none();
        };
        let key = key.into();
        Task::perform(
            async move { db.get_view(&key).await.and_then(|bytes| serde_json::from_slice::<T>(&bytes).ok()) },
            move |opt| {
                cosmic::Action::App(match opt {
                    Some(data) => on_hit(data),
                    None => Message::Noop,
                })
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn cache_put_round_trips_via_db() {
        let db = crate::cache::Db::open(Path::new(":memory:")).await.expect("open db");
        let payload = vec!["alpha".to_string(), "beta".to_string()];
        cache_put(Some(db.clone()), "library:playlists", &payload).await;

        let bytes = db.get_view("library:playlists").await.expect("cache hit");
        let back: Vec<String> = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(back, payload);
    }

    #[tokio::test]
    async fn cache_put_without_db_is_noop() {
        // No database: must be a silent no-op, never a panic.
        cache_put::<Vec<u8>>(None, "k", &Vec::new()).await;
    }

    #[tokio::test]
    async fn cache_put_skips_oversized_payload() {
        let db = crate::cache::Db::open(Path::new(":memory:")).await.expect("open db");
        // One byte over the per-entry cap serializes larger than the cap and is
        // skipped, so the read misses.
        let huge = vec![0u8; (MAX_VIEW_ENTRY_BYTES as usize) + 1];
        cache_put(Some(db.clone()), "big", &huge).await;
        assert!(db.get_view("big").await.is_none());
    }
}
