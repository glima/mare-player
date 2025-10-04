// SPDX-License-Identifier: MIT

//! Audio player for TIDAL streaming using the built-in audio engine.
//!
//! This module provides a simple audio player interface that wraps the
//! audio engine.
//! Supports both direct URL streaming and DASH manifest playback for HiRes.

use crate::audio::spectrum::SharedSpectrumAnalyzer;
use crate::audio::{
    AudioEngine, AudioEngineEvent, PlaybackState as EnginePlaybackState, SpectrumData,
};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, error, info};

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

impl From<EnginePlaybackState> for PlaybackState {
    fn from(state: EnginePlaybackState) -> Self {
        match state {
            EnginePlaybackState::Stopped => PlaybackState::Stopped,
            EnginePlaybackState::Playing => PlaybackState::Playing,
            EnginePlaybackState::Paused => PlaybackState::Paused,
            EnginePlaybackState::Loading => PlaybackState::Loading,
        }
    }
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

/// Events emitted by the player
#[derive(Debug, Clone)]
pub enum PlayerEvent {
    /// Track finished playing naturally (EOF)
    TrackEnded,
    /// Preloaded track started playing (gapless transition)
    PreloadConsumed,
    /// Error occurred during playback
    Error(String),
    /// Playback state changed (e.g. Loading → Playing)
    StateChanged(PlaybackState),
    /// Download/buffering progress (0.0 to 1.0)
    LoadingProgress(f64),
}

/// Audio player using the built-in audio engine (symphonia + PulseAudio)
pub struct Player {
    /// The audio engine
    engine: Arc<AudioEngine>,
    /// Cached now playing info
    now_playing: Option<NowPlaying>,
}

impl Player {
    /// Create a new Player instance
    pub fn new() -> Result<Self, String> {
        info!("Initializing audio player");

        let engine =
            AudioEngine::new().map_err(|e| format!("Failed to create audio engine: {}", e))?;

        info!("Audio player initialized successfully");

        Ok(Self {
            engine: Arc::new(engine),
            now_playing: None,
        })
    }

    /// Play an audio stream from a URL
    pub fn play(&mut self, url: &str, track_info: NowPlaying) -> Result<(), String> {
        self.play_cached(url, track_info, None, None)
    }

    /// Play an audio stream from a URL, saving the download to `cache_path` when complete
    pub fn play_cached(
        &mut self,
        url: &str,
        track_info: NowPlaying,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    ) -> Result<(), String> {
        info!(
            "Playing track: {} - {}{}",
            track_info.artist,
            track_info.title,
            if cache_path.is_some() {
                " (will cache)"
            } else {
                ""
            },
        );
        debug!("Stream URL: {}...", &url[..url.len().min(50)]);

        // Store now playing info
        self.now_playing = Some(track_info.clone());

        // Send play command to engine
        self.engine
            .play_url_cached(url, cache_path, replay_gain_db)
            .map_err(|e| format!("Failed to play: {}", e))?;

        Ok(())
    }

    /// Play audio from a DASH manifest file (for HiRes streaming)
    pub fn play_dash<P: AsRef<Path>>(
        &mut self,
        manifest_path: P,
        track_info: NowPlaying,
    ) -> Result<(), String> {
        self.play_dash_cached(manifest_path, track_info, None, None)
    }

    /// Play audio from a DASH manifest file, saving the download to `cache_path` when complete
    pub fn play_dash_cached<P: AsRef<Path>>(
        &mut self,
        manifest_path: P,
        track_info: NowPlaying,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    ) -> Result<(), String> {
        let path_str = manifest_path.as_ref().to_string_lossy().to_string();
        info!(
            "Playing DASH track: {} - {} from {}{}",
            track_info.artist,
            track_info.title,
            path_str,
            if cache_path.is_some() {
                " (will cache)"
            } else {
                ""
            },
        );

        // Store now playing info
        self.now_playing = Some(track_info.clone());

        // Send DASH play command to engine (with optional cache path)
        self.engine
            .play_dash_cached(&path_str, cache_path, replay_gain_db)
            .map_err(|e| format!("Failed to play DASH: {}", e))?;

        Ok(())
    }

    /// Preload a URL for gapless transition to the next track
    pub fn preload_url(
        &self,
        url: &str,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    ) -> Result<(), String> {
        info!("Preloading next track URL for gapless playback");
        self.engine
            .preload_url(url, cache_path, replay_gain_db)
            .map_err(|e| format!("Failed to preload: {}", e))
    }

    /// Preload a DASH manifest for gapless transition
    pub fn preload_dash<P: AsRef<Path>>(
        &self,
        manifest_path: P,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    ) -> Result<(), String> {
        let path_str = manifest_path.as_ref().to_string_lossy().to_string();
        info!("Preloading next track DASH manifest for gapless playback");
        self.engine
            .preload_dash(&path_str, cache_path, replay_gain_db)
            .map_err(|e| format!("Failed to preload DASH: {}", e))
    }

    /// Preload a cached file for gapless transition
    pub fn preload_file<P: AsRef<Path>>(
        &self,
        path: P,
        replay_gain_db: Option<f32>,
    ) -> Result<(), String> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        info!("Preloading next track file for gapless playback");
        self.engine
            .preload_file(&path_str, replay_gain_db)
            .map_err(|e| format!("Failed to preload file: {}", e))
    }

    /// Play audio from a local cached file on disk
    pub fn play_file<P: AsRef<Path>>(
        &mut self,
        path: P,
        track_info: NowPlaying,
        replay_gain_db: Option<f32>,
    ) -> Result<(), String> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        info!(
            "Playing cached file: {} - {} from {}",
            track_info.artist, track_info.title, path_str
        );

        // Store now playing info
        self.now_playing = Some(track_info.clone());

        // Send play-file command to engine (no network, instant decode)
        self.engine
            .play_file(&path_str, replay_gain_db)
            .map_err(|e| format!("Failed to play cached file: {}", e))?;

        Ok(())
    }

    /// Toggle pause/play
    pub fn toggle_pause(&self) -> Result<(), String> {
        self.engine
            .toggle_pause()
            .map_err(|e| format!("Failed to toggle pause: {}", e))
    }

    /// Stop playback
    pub fn stop(&mut self) -> Result<(), String> {
        debug!("Stopping playback");
        self.now_playing = None;
        self.engine
            .stop()
            .map_err(|e| format!("Failed to stop: {}", e))
    }

    /// Seek to absolute position (in seconds)
    pub fn seek_absolute(&self, seconds: f64) -> Result<(), String> {
        debug!("Seeking to {} seconds", seconds);
        self.engine
            .seek(seconds)
            .map_err(|e| format!("Failed to seek: {}", e))
    }

    /// Get current playback position in seconds
    pub fn get_position(&self) -> Option<f64> {
        let pos = self.engine.position();
        if pos > 0.0 || self.engine.is_playing() {
            Some(pos)
        } else {
            None
        }
    }

    /// Get current playback state
    pub fn get_state(&self) -> PlaybackState {
        self.engine.state().into()
    }

    /// Check if player is currently playing
    pub fn is_playing(&self) -> bool {
        self.engine.is_playing()
    }

    /// Get current spectrum data for visualization
    pub fn get_spectrum(&self) -> SpectrumData {
        self.engine.spectrum()
    }

    /// Get a cheap handle to the shared spectrum analyzer.
    ///
    /// Used by the self-animating visualizer widget to read FFT data
    /// directly on each frame, bypassing the `update()` → `view()` cycle.
    pub fn spectrum_analyzer(&self) -> SharedSpectrumAnalyzer {
        self.engine.spectrum_analyzer()
    }

    /// Set volume level (0.0 to 1.0)
    pub fn set_volume(&self, level: f32) -> Result<(), String> {
        self.engine
            .set_volume(level)
            .map_err(|e| format!("Failed to set volume: {}", e))
    }

    /// Get current volume level (0.0 to 1.0)
    pub fn volume(&self) -> f32 {
        self.engine.volume()
    }

    /// Process pending events and return any player events
    /// This should be called periodically (e.g., in a subscription)
    pub fn process_events(&self) -> Vec<PlayerEvent> {
        let mut events = Vec::new();

        // Process all pending events from the engine
        while let Some(event) = self.engine.try_recv_event() {
            match event {
                AudioEngineEvent::TrackEnded => {
                    info!("Track ended");
                    events.push(PlayerEvent::TrackEnded);
                }
                AudioEngineEvent::PreloadConsumed => {
                    info!("Gapless transition: preloaded track now playing");
                    events.push(PlayerEvent::PreloadConsumed);
                }
                AudioEngineEvent::Error(msg) => {
                    error!("Playback error: {}", msg);
                    events.push(PlayerEvent::Error(msg));
                }
                AudioEngineEvent::StateChanged(state) => {
                    let ps: PlaybackState = state.into();
                    debug!("State changed: {:?}", ps);
                    events.push(PlayerEvent::StateChanged(ps));
                }
                AudioEngineEvent::Position(_) => {
                    // Position updates are handled via get_position()
                }
                AudioEngineEvent::Duration(_) => {
                    // Duration is stored in track info
                }
                AudioEngineEvent::LoadingProgress(progress) => {
                    events.push(PlayerEvent::LoadingProgress(progress));
                }
            }
        }

        events
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        info!("Shutting down audio player");
        // Engine handles cleanup in its own Drop
    }
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
