// SPDX-License-Identifier: MIT

//! Local play history for Maré Player.
//!
//! TIDAL's API does not expose a per-track "recently played" endpoint, so we
//! maintain one locally.  Each time a track starts playing successfully its
//! metadata is prepended to an ordered list that is persisted to the API disk
//! cache as JSON.  Duplicates are collapsed: if the same track is played again
//! it is moved to the front rather than appearing twice.
//!
//! The history is unbounded — every track ever played is retained.

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::disk_cache::DiskCache;
use crate::tidal::models::Track;

/// Cache key used for the serialised history inside the API [`DiskCache`].
const CACHE_KEY: &str = "play_history";

/// A timestamped history entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The track that was played.
    pub track: Track,
    /// UTC timestamp (ISO-8601) of when playback started.
    pub played_at: String,
}

/// Local play history backed by the API disk cache.
#[derive(Debug, Clone, Default)]
pub struct PlayHistory {
    /// Ordered list of history entries, most-recent first.
    entries: Vec<HistoryEntry>,
}

impl PlayHistory {
    /// Create an empty history.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Load a previously-persisted history from the API cache.
    ///
    /// Returns an empty history if no cache file exists or if it is corrupt.
    pub fn load(api_cache: &DiskCache) -> Self {
        let entries: Vec<HistoryEntry> = api_cache
            .get_hashed(CACHE_KEY, "json")
            .and_then(|data| {
                serde_json::from_slice(&data)
                    .map_err(|e| {
                        warn!("Failed to deserialise play history cache: {}", e);
                        e
                    })
                    .ok()
            })
            .unwrap_or_default();

        debug!("Loaded {} play history entries from cache", entries.len());
        Self { entries }
    }

    /// Persist the current history to the API cache.
    pub fn save(&self, api_cache: &DiskCache) {
        match serde_json::to_vec(&self.entries) {
            Ok(json) => {
                if let Err(e) = api_cache.put_hashed(CACHE_KEY, "json", &json) {
                    warn!("Failed to persist play history: {}", e);
                } else {
                    debug!(
                        "Persisted {} play history entries ({} bytes)",
                        self.entries.len(),
                        json.len()
                    );
                }
            }
            Err(e) => {
                warn!("Failed to serialise play history: {}", e);
            }
        }
    }

    /// Record a track as just-played.
    ///
    /// The track is prepended to the list.  If the same track ID already
    /// exists anywhere in the list it is removed first (dedup / move-to-front).
    ///
    /// **Does not** persist automatically — call [`save`](Self::save) afterwards
    /// when you have access to the cache.
    pub fn record(&mut self, track: &Track) {
        // Remove any previous occurrence of this track.
        self.entries.retain(|e| e.track.id != track.id);

        let entry = HistoryEntry {
            track: track.clone(),
            played_at: chrono::Utc::now().to_rfc3339(),
        };

        self.entries.insert(0, entry);
    }

    /// The full ordered list of history entries (most-recent first).
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Extract just the tracks (most-recent first), without timestamps.
    pub fn tracks(&self) -> Vec<Track> {
        self.entries.iter().map(|e| e.track.clone()).collect()
    }

    /// Number of entries in the history.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove all entries.
    ///
    /// Call [`save`](Self::save) afterwards to persist the change.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_track(id: &str, title: &str) -> Track {
        Track {
            id: id.to_string(),
            title: title.to_string(),
            artist_name: "Test Artist".to_string(),
            duration: 180,
            ..Default::default()
        }
    }

    #[test]
    fn new_history_is_empty() {
        let h = PlayHistory::new();
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
        assert!(h.tracks().is_empty());
        assert!(h.entries().is_empty());
    }

    #[test]
    fn default_history_is_empty() {
        let h = PlayHistory::default();
        assert!(h.is_empty());
    }

    #[test]
    fn record_adds_to_front() {
        let mut h = PlayHistory::new();
        h.record(&make_track("1", "First"));
        h.record(&make_track("2", "Second"));
        h.record(&make_track("3", "Third"));

        let tracks = h.tracks();
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0].id, "3");
        assert_eq!(tracks[1].id, "2");
        assert_eq!(tracks[2].id, "1");
    }

    #[test]
    fn record_deduplicates_by_moving_to_front() {
        let mut h = PlayHistory::new();
        h.record(&make_track("1", "First"));
        h.record(&make_track("2", "Second"));
        h.record(&make_track("3", "Third"));

        // Play track "1" again — it should move to the front.
        h.record(&make_track("1", "First (replayed)"));

        let tracks = h.tracks();
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0].id, "1");
        assert_eq!(tracks[0].title, "First (replayed)");
        assert_eq!(tracks[1].id, "3");
        assert_eq!(tracks[2].id, "2");
    }

    #[test]
    fn record_does_not_cap_entries() {
        let mut h = PlayHistory::new();
        for i in 0..500 {
            h.record(&make_track(&i.to_string(), &format!("Track {}", i)));
        }
        assert_eq!(h.len(), 500);

        // Most recent should be the last one recorded.
        assert_eq!(h.tracks()[0].id, "499");
    }

    #[test]
    fn clear_empties_the_history() {
        let mut h = PlayHistory::new();
        h.record(&make_track("1", "First"));
        h.record(&make_track("2", "Second"));
        assert_eq!(h.len(), 2);

        h.clear();
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn entries_have_timestamps() {
        let mut h = PlayHistory::new();
        h.record(&make_track("1", "First"));

        let entry = &h.entries()[0];
        assert!(!entry.played_at.is_empty());
        // Should be a valid RFC-3339 / ISO-8601 timestamp.
        assert!(
            entry.played_at.contains('T'),
            "expected ISO-8601 timestamp, got: {}",
            entry.played_at
        );
    }

    #[test]
    fn tracks_returns_cloned_vec() {
        let mut h = PlayHistory::new();
        h.record(&make_track("1", "First"));
        h.record(&make_track("2", "Second"));

        let tracks = h.tracks();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].id, "2");
        assert_eq!(tracks[1].id, "1");
    }

    #[test]
    fn serde_roundtrip() {
        let mut h = PlayHistory::new();
        h.record(&make_track("1", "First"));
        h.record(&make_track("2", "Second"));

        let json = serde_json::to_vec(&h.entries).expect("serialise");
        let restored: Vec<HistoryEntry> = serde_json::from_slice(&json).expect("deserialise");

        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].track.id, "2");
        assert_eq!(restored[1].track.id, "1");
    }

    #[test]
    fn deserialise_empty_array() {
        let json = b"[]";
        let entries: Vec<HistoryEntry> = serde_json::from_slice(json).expect("deserialise empty");
        assert!(entries.is_empty());
    }

    #[test]
    fn deserialise_corrupt_data_falls_back() {
        let bad = b"not valid json!!!";
        let result: Result<Vec<HistoryEntry>, _> = serde_json::from_slice(bad);
        assert!(result.is_err());
    }

    #[test]
    fn record_same_track_many_times_keeps_one() {
        let mut h = PlayHistory::new();
        for i in 0..50 {
            h.record(&make_track("same", &format!("Attempt {}", i)));
        }
        assert_eq!(h.len(), 1);
        assert_eq!(h.tracks()[0].title, "Attempt 49");
    }

    #[test]
    fn interleaved_record_and_dedup() {
        let mut h = PlayHistory::new();
        h.record(&make_track("a", "A"));
        h.record(&make_track("b", "B"));
        h.record(&make_track("a", "A2"));
        h.record(&make_track("c", "C"));
        h.record(&make_track("b", "B2"));

        let tracks = h.tracks();
        let ids: Vec<&str> = tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "c", "a"]);
        assert_eq!(h.tracks()[0].title, "B2");
        assert_eq!(h.tracks()[2].title, "A2");
    }
}
