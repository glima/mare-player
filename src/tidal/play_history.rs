// SPDX-License-Identifier: MIT

//! Local play history for Maré Player.
//!
//! TIDAL's API does not expose a per-track "recently played" endpoint, so we
//! maintain one locally.  Each time a track starts playing successfully its
//! metadata is prepended to an ordered in-memory list and upserted as a single
//! row into the cache database's `play_history` table.  Duplicates are
//! collapsed: if the same track is played again it is moved to the front
//! rather than appearing twice.
//!
//! The history is unbounded — every track ever played is retained.

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::tidal::models::Track;

/// A timestamped history entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The track that was played.
    pub track: Track,
    /// UTC timestamp (ISO-8601) of when playback started.
    pub played_at: String,
}

impl HistoryEntry {
    /// Parse [`played_at`](Self::played_at) (RFC-3339) into epoch milliseconds
    /// for the `play_history` table's ordering column, falling back to the
    /// current time if the stored string can't be parsed.
    pub fn played_at_millis(&self) -> i64 {
        chrono::DateTime::parse_from_rfc3339(&self.played_at)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis())
    }

    /// Serialise this entry to JSON bytes, or `None` on failure.
    ///
    /// Used to persist one row into the cache database's `play_history` table.
    pub fn to_json(&self) -> Option<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|e| {
                warn!("Failed to serialise play history entry: {}", e);
                e
            })
            .ok()
    }
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
        Self { entries: Vec::new() }
    }

    /// Replace all entries (used after the database loads them at startup).
    pub fn set_entries(&mut self, entries: Vec<HistoryEntry>) {
        self.entries = entries;
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

        let entry = HistoryEntry { track: track.clone(), played_at: chrono::Utc::now().to_rfc3339() };

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
        assert!(entry.played_at.contains('T'), "expected ISO-8601 timestamp, got: {}", entry.played_at);
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

    #[test]
    fn entry_to_json_round_trips() {
        let mut h = PlayHistory::new();
        h.record(&make_track("a", "A"));
        let entry = &h.entries()[0];

        let bytes = entry.to_json().expect("serialise");
        let restored: HistoryEntry = serde_json::from_slice(&bytes).expect("deserialise");
        assert_eq!(restored.track.id, "a");
        assert_eq!(restored.played_at, entry.played_at);
    }

    #[test]
    fn played_at_millis_parses_rfc3339() {
        let entry = HistoryEntry { track: make_track("a", "A"), played_at: "2021-02-26T00:00:00+00:00".to_string() };
        // 2021-02-26T00:00:00Z == 1614297600 s == 1614297600000 ms.
        assert_eq!(entry.played_at_millis(), 1_614_297_600_000);
    }

    #[test]
    fn played_at_millis_falls_back_on_garbage() {
        let entry = HistoryEntry { track: make_track("a", "A"), played_at: "not-a-timestamp".to_string() };
        // Falls back to "now" rather than panicking; just assert it's positive.
        assert!(entry.played_at_millis() > 0);
    }

    #[test]
    fn set_entries_replaces_contents() {
        let mut h = PlayHistory::new();
        h.record(&make_track("x", "X"));
        h.set_entries(Vec::new());
        assert!(h.is_empty());
    }
}
