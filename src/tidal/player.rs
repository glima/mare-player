// SPDX-License-Identifier: MIT

//! Shared playback value types.
//!
//! Playback itself runs through the GStreamer engine in [`crate::playback`];
//! this module just holds the small UI-facing types (`PlaybackState`,
//! `NowPlaying`) that the app model, handlers, views, and MPRIS bridge share.

/// Playback state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackState {
    /// No track loaded
    #[default]
    Stopped,
    /// Track is playing
    Playing,
    /// Track is paused
    Paused,
    /// Track is loading/buffering
    Loading,
}

/// Information about the currently playing track
#[derive(Debug, Clone, Default)]
pub struct NowPlaying {
    /// Track ID
    pub track_id: String,
    /// Track title
    pub title: String,
    /// Artist name
    pub artist: String,
    /// Album name
    pub album: Option<String>,
    /// Track duration in seconds
    pub duration: f64,
    /// Cover art URL
    pub cover_url: Option<String>,
    /// Context: playlist name if playing from a playlist
    pub playlist_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playback_state_default() {
        let state = PlaybackState::default();
        assert_eq!(state, PlaybackState::Stopped);
    }

    #[test]
    fn test_now_playing_default() {
        let np = NowPlaying::default();
        assert!(np.track_id.is_empty());
        assert!(np.title.is_empty());
    }
}
