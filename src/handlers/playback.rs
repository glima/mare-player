// SPDX-License-Identifier: MIT

//! Playback control message handlers for Maré Player.
//!
//! This module handles play, pause, stop, seek, queue management, shuffle, and volume control.

use cosmic::Application;
use cosmic::cosmic_config::CosmicConfigEntry;
#[cfg(feature = "panel-applet")]
use cosmic::iced::platform_specific::shell::commands::popup::destroy_popup;
use cosmic::prelude::*;

use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::client::PlaybackUrl;
use crate::tidal::models::Track;
use crate::tidal::mpris::LoopStatus;
use crate::tidal::player::{NowPlaying, PlaybackState};
use std::sync::Arc;
use std::time::{Duration, Instant};

// =============================================================================
// Task Helper Methods
// =============================================================================

impl AppModel {
    /// Start playback of a track at the given index in the queue.
    /// Fetches the playback URL from TIDAL and triggers playback.
    pub(crate) fn play_track_at_index(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        if index >= self.playback_queue.len() {
            return Task::none();
        }
        let Some(track) = self.playback_queue.get(index).cloned() else {
            return Task::none();
        };
        // Reset position and switch to Loading immediately so the slider
        // rewinds to 0:00 before the new track starts buffering.  Setting
        // the state here prevents the tick handler from overwriting the
        // position with the *old* track's value while the URL is being
        // fetched (race between async fetch and the 50 ms tick).
        self.playback_position = 0.0;
        self.loading_progress = 0.0;
        self.playback_state = PlaybackState::Loading;

        // Drain any stale engine events (e.g. a queued StateChanged(Playing)
        // from the previous track) so they don't flip us back out of Loading
        // on the next tick.
        if let Some(player) = &self.player {
            let _ = player.process_events();
        }

        let track_id = track.id.clone();
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                match client.get_track_playback_url(&track_id).await {
                    Ok(url) => Ok((track, url)),
                    Err(e) => Err(e.to_string()),
                }
            },
            |result| cosmic::Action::App(Message::PlaybackUrlReceived(result)),
        )
    }
}

// =============================================================================
// Message Handlers
// =============================================================================

impl AppModel {
    /// Handle play single track
    pub fn handle_play_track(
        &mut self,
        track: Track,
        source: Option<crate::tidal::models::PlaybackSource>,
    ) -> Task<cosmic::Action<Message>> {
        tracing::info!(
            "Play single track requested: {} - {} (source: {:?})",
            track.artist_name,
            track.title,
            source.as_ref().map(|s| (&s.kind, &s.id))
        );
        self.playback_source = source;
        // Clear queue and play just this track
        self.playback_queue = vec![track.clone()];
        self.playback_queue_index = 0;
        let play_task = self.play_track_at_index(0);
        // Dismiss popup (panel-applet mode only)
        #[cfg(feature = "panel-applet")]
        if let Some(p) = self.popup.take() {
            return Task::batch(vec![play_task, destroy_popup(p)]);
        }
        play_task
    }

    /// Handle play track list starting at index
    pub fn handle_play_track_list(
        &mut self,
        tracks: Arc<[Track]>,
        start_index: usize,
        source: Option<crate::tidal::models::PlaybackSource>,
    ) -> Task<cosmic::Action<Message>> {
        tracing::info!(
            "Play track list requested: {} tracks, starting at index {} (source: {:?})",
            tracks.len(),
            start_index,
            source.as_ref().map(|s| (&s.kind, &s.id))
        );
        self.playback_source = source;
        self.playback_queue = tracks.to_vec();
        self.playback_queue_index = start_index;
        self.shuffle_enabled = false;
        let play_task = self.play_track_at_index(start_index);
        // Dismiss popup (panel-applet mode only)
        #[cfg(feature = "panel-applet")]
        if let Some(p) = self.popup.take() {
            return Task::batch(vec![play_task, destroy_popup(p)]);
        }
        play_task
    }

    /// Handle shuffle play tracks
    pub fn handle_shuffle_play(
        &mut self,
        tracks: Arc<[Track]>,
        source: Option<crate::tidal::models::PlaybackSource>,
    ) -> Task<cosmic::Action<Message>> {
        tracing::info!(
            "Shuffle play requested: {} tracks (source: {:?})",
            tracks.len(),
            source.as_ref().map(|s| (&s.kind, &s.id))
        );
        self.playback_source = source;
        use rand::seq::SliceRandom;
        let mut rng = rand::rng();
        let mut shuffled = tracks.to_vec();
        shuffled.shuffle(&mut rng);
        self.playback_queue = shuffled;
        self.playback_queue_index = 0;
        self.shuffle_enabled = true;
        let play_task = self.play_track_at_index(0);
        // Dismiss popup (panel-applet mode only)
        #[cfg(feature = "panel-applet")]
        if let Some(p) = self.popup.take() {
            return Task::batch(vec![play_task, destroy_popup(p)]);
        }
        play_task
    }

    /// Handle next track
    pub fn handle_next_track(&mut self) -> Task<cosmic::Action<Message>> {
        if self.playback_queue.is_empty() {
            return Task::none();
        }
        let next_index = self.playback_queue_index + 1;
        if next_index < self.playback_queue.len() {
            self.playback_queue_index = next_index;
            self.play_track_at_index(next_index)
        } else {
            // End of queue — behaviour depends on loop status
            match self.loop_status {
                LoopStatus::Track => {
                    // Repeat the current track
                    self.play_track_at_index(self.playback_queue_index)
                }
                LoopStatus::Playlist => {
                    // Wrap around to the beginning
                    self.playback_queue_index = 0;
                    self.play_track_at_index(0)
                }
                LoopStatus::None => {
                    // Stop playback
                    self.playback_state = PlaybackState::Stopped;
                    self.now_playing = None;
                    self.update_mpris_state()
                }
            }
        }
    }

    /// Handle previous track
    pub fn handle_previous_track(&mut self) -> Task<cosmic::Action<Message>> {
        if self.playback_queue.is_empty() {
            return Task::none();
        }
        // If we're more than 3 seconds in, restart the current track
        if self.playback_position > 3.0 {
            return self.play_track_at_index(self.playback_queue_index);
        }
        // Otherwise go to previous track
        if self.playback_queue_index > 0 {
            self.playback_queue_index -= 1;
            self.play_track_at_index(self.playback_queue_index)
        } else {
            // At the beginning, just restart
            self.play_track_at_index(0)
        }
    }

    /// Handle toggle shuffle
    pub fn handle_toggle_shuffle(&mut self) {
        self.shuffle_enabled = !self.shuffle_enabled;
        if self.shuffle_enabled && !self.playback_queue.is_empty() {
            // Shuffle the remaining tracks (keep current track in place)
            use rand::seq::SliceRandom;
            let mut rng = rand::rng();
            if let Some(current_track) = self.playback_queue.get(self.playback_queue_index).cloned()
            {
                let mut remaining: Vec<_> = self
                    .playback_queue
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != self.playback_queue_index)
                    .map(|(_, t)| t.clone())
                    .collect();
                remaining.shuffle(&mut rng);
                self.playback_queue = vec![current_track];
                self.playback_queue.extend(remaining);
                self.playback_queue_index = 0;
            }
        }
    }

    /// Handle playback URL received
    pub fn handle_playback_url_received(
        &mut self,
        result: Result<(Track, PlaybackUrl), String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok((track, playback_url)) => {
                if let Some(player) = &mut self.player {
                    let now_playing = NowPlaying {
                        track_id: track.id.clone(),
                        title: track.title.clone(),
                        artist: track.artist_name.clone(),
                        album: track.album_name.clone(),
                        cover_url: track.cover_url.clone(),
                        duration: track.duration as f64,
                        playlist_name: self
                            .playback_source
                            .as_ref()
                            .map(|s| s.display_name.clone()),
                    };

                    let replay_gain_db = playback_url.replay_gain_db();

                    // Handle playback based on URL type.
                    //
                    // For CachedFile we skip `audio_cache_path_for` entirely:
                    // that method calls `reserve_room` which can trigger
                    // eviction even though no new file will be written.
                    // Calling it on every cache-hit skip was inflating the
                    // eviction pressure and could nuke the whole cache when
                    // the in-memory byte counter drifted.
                    let play_result = if playback_url.is_cached() {
                        // Cached file on disk - instant playback, no network.
                        // Protect the file from eviction while it's playing.
                        let path = playback_url.as_url();
                        {
                            let client = self.tidal_client.blocking_lock();
                            client
                                .audio_cache()
                                .protect_path(std::path::Path::new(&path));
                        }
                        tracing::info!("Playing from cache: {}", path);
                        player.play_file(&path, now_playing.clone(), replay_gain_db)
                    } else if playback_url.is_dash() {
                        // DASH manifest - use specialized DASH player.
                        // Compute cache path (triggers reserve_room for the
                        // incoming download).
                        let cache_path = {
                            let client = self.tidal_client.blocking_lock();
                            let p = client.audio_cache_path_for(&track.id);
                            client.audio_cache().protect_path(&p);
                            p.to_string_lossy().to_string()
                        };
                        let manifest_path = playback_url.as_url();
                        tracing::info!(
                            "Playing HiRes DASH manifest: {} (caching to {})",
                            manifest_path,
                            cache_path
                        );
                        player.play_dash_cached(
                            &manifest_path,
                            now_playing.clone(),
                            Some(cache_path),
                            replay_gain_db,
                        )
                    } else {
                        // Direct URL - use regular player.
                        // Compute cache path (triggers reserve_room for the
                        // incoming download).
                        let cache_path = {
                            let client = self.tidal_client.blocking_lock();
                            let p = client.audio_cache_path_for(&track.id);
                            client.audio_cache().protect_path(&p);
                            p.to_string_lossy().to_string()
                        };
                        let url = playback_url.as_url();
                        tracing::info!(
                            "Playing URL: {} (caching to {})",
                            &url[..url.len().min(60)],
                            cache_path
                        );
                        player.play_cached(
                            &url,
                            now_playing.clone(),
                            Some(cache_path),
                            replay_gain_db,
                        )
                    };

                    if let Err(e) = play_result {
                        tracing::error!("Playback failed: {}", e);
                        self.error_message = Some(format!("Playback failed: {}", e));
                    } else {
                        tracing::info!(
                            "Playback started - staying in Loading until engine is ready"
                        );
                        // Don't set Playing here — the engine will send
                        // StateChanged(Loading) and then StateChanged(Playing)
                        // once buffering finishes.  Setting Playing prematurely
                        // causes the tick handler to read the old track's
                        // position for a few frames.
                        self.playback_state = PlaybackState::Loading;
                        self.now_playing = Some(now_playing);
                        self.playback_position = 0.0;
                        self.visualizer_state.set_active(false);

                        // Record this track in the local play history
                        self.play_history.record(&track);
                        {
                            let client = self.tidal_client.blocking_lock();
                            self.play_history.save(client.api_cache());
                        }

                        // Open a TIDAL play-attribution session for this
                        // track.  Finalises the previous session (if any),
                        // so quick skips still report the prior listen if
                        // it met the threshold.
                        self.open_play_session(&track);

                        // Update MPRIS state
                        let mpris_task = self.update_mpris_state();

                        // Load cover image for panel button display
                        if let Some(cover_url) = &track.cover_url
                            && !self.loaded_images.contains_key(cover_url)
                            && !self.pending_image_loads.contains(cover_url)
                        {
                            return Task::batch(vec![
                                mpris_task,
                                self.load_images_for_urls(vec![cover_url.clone()]),
                            ]);
                        }
                        return mpris_task;
                    }
                } else {
                    tracing::error!("Player not available");
                    self.error_message = Some("Player not available".to_string());
                }
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to get playback URL: {}", e);
                self.error_message = Some(format!("Failed to get playback URL: {}", e));
                Task::none()
            }
        }
    }

    /// Handle seek to position (percentage)
    pub fn handle_seek_to(&mut self, percent: f64) -> Task<cosmic::Action<Message>> {
        if let Some(np) = &self.now_playing
            && np.duration > 0.0
        {
            let target_pos = (percent / 100.0) * np.duration;
            tracing::info!(
                "SeekTo: {}% -> {:.2}s (duration: {:.2}s)",
                percent,
                target_pos,
                np.duration
            );
            // Update UI position immediately for responsiveness
            self.playback_position = target_pos;
            // Store pending seek and increment version for debouncing
            self.pending_seek = Some(target_pos);
            self.seek_debounce_version = self.seek_debounce_version.wrapping_add(1);
            let version = self.seek_debounce_version;
            // Debounce: wait 50ms before actually seeking (reduced for snappier response)
            return Task::perform(
                async move {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    version
                },
                |v| cosmic::Action::App(Message::SeekDebounced(v)),
            );
        }
        Task::none()
    }

    /// Handle debounced seek execution
    pub fn handle_seek_debounced(&mut self, version: u64) -> Task<cosmic::Action<Message>> {
        // Only execute if this is the latest seek request
        if version == self.seek_debounce_version
            && let Some(target_pos) = self.pending_seek.take()
        {
            tracing::info!("SeekDebounced: executing seek to {:.2}s", target_pos);
            let start = std::time::Instant::now();
            if let Some(player) = &self.player {
                if let Err(e) = player.seek_absolute(target_pos) {
                    self.error_message = Some(format!("Seek failed: {}", e));
                    tracing::error!("Seek failed after {:?}: {}", start.elapsed(), e);
                } else {
                    tracing::info!("Seek completed in {:?}", start.elapsed());
                    return self.update_mpris_state();
                }
            }
        }
        Task::none()
    }

    /// Handle toggle play/pause
    pub fn handle_toggle_play_pause(&mut self) -> Task<cosmic::Action<Message>> {
        if let Some(player) = &self.player {
            // Determine new state BEFORE toggling (to avoid race condition)
            // toggle_pause sends async command to playback thread, so we can't
            // rely on is_playing() immediately after
            let new_state = match self.playback_state {
                PlaybackState::Playing => PlaybackState::Paused,
                PlaybackState::Paused => PlaybackState::Playing,
                other => other, // Don't change if stopped/loading
            };

            if let Err(e) = player.toggle_pause() {
                tracing::error!("Playback control failed: {}", e);
                self.error_message = Some(format!("Playback control failed: {}", e));
            } else {
                let is_playing = new_state == PlaybackState::Playing;
                self.playback_state = new_state;
                self.visualizer_state.set_active(is_playing);
                return self.update_mpris_state();
            }
        }
        Task::none()
    }

    /// Handle stop playback
    pub fn handle_stop_playback(&mut self) -> Task<cosmic::Action<Message>> {
        // Finalise the in-progress TIDAL play-attribution session, if any,
        // before tearing down playback state.  Reports the listen if it
        // crossed the threshold.
        self.finalize_and_report_play_session();
        if let Some(player) = &mut self.player {
            let _ = player.stop();
            self.playback_state = PlaybackState::Stopped;
            self.now_playing = None;
            self.playback_position = 0.0;
            self.visualizer_state.set_active(false);
            return self.update_mpris_state();
        }
        Task::none()
    }

    /// Handle playback tick — updates position, processes engine events, and
    /// hides the volume bar after a timeout.
    ///
    /// The visualizer does not need updating here: it is a self-animating
    /// widget that reads spectrum data directly from the shared
    /// `SharedSpectrumAnalyzer` and drives its own redraws via
    /// `shell.request_redraw()`.
    pub fn handle_playback_tick(&mut self) -> Task<cosmic::Action<Message>> {
        // Update playback position and process engine events.
        if (self.playback_state == PlaybackState::Playing
            || self.playback_state == PlaybackState::Loading)
            && let Some(player) = &self.player
        {
            // Update playback position (only meaningful when actually playing)
            if self.playback_state == PlaybackState::Playing
                && let Some(pos) = player.get_position()
            {
                self.playback_position = pos;
                // Keep the TIDAL play-attribution session's high-water-mark
                // position fresh; cheap when no session is open.  Inlined
                // because the surrounding `let Some(player) = &self.player`
                // borrow prevents calling `&mut self` methods on `self`.
                if let Some(p) = &mut self.current_play {
                    p.observe_position(pos);
                }
            }

            // Process player events
            for event in player.process_events() {
                match event {
                    crate::tidal::player::PlayerEvent::TrackEnded => {
                        // Auto-advance — behaviour depends on loop status.
                        match self.loop_status {
                            LoopStatus::Track => {
                                // Repeat the current track
                                return self.play_track_at_index(self.playback_queue_index);
                            }
                            _ => {
                                let next_index = self.playback_queue_index + 1;
                                if next_index < self.playback_queue.len() {
                                    return Task::done(cosmic::Action::App(Message::NextTrack));
                                } else if self.loop_status == LoopStatus::Playlist {
                                    // Wrap around to the beginning
                                    self.playback_queue_index = 0;
                                    return self.play_track_at_index(0);
                                } else {
                                    // End of queue
                                    self.playback_state = PlaybackState::Stopped;
                                    self.now_playing = None;
                                    self.visualizer_state.set_active(false);
                                }
                            }
                        }
                    }
                    crate::tidal::player::PlayerEvent::PreloadConsumed => {
                        // Gapless transition occurred — the preloaded track
                        // started playing automatically.
                        tracing::info!("Gapless transition: preloaded track now playing");

                        // Determine the index of the track that just started,
                        // respecting loop modes.
                        use crate::tidal::mpris::LoopStatus;
                        let new_index = match self.loop_status {
                            LoopStatus::Track => {
                                // Repeat-track: index stays the same
                                Some(self.playback_queue_index)
                            }
                            LoopStatus::Playlist => {
                                let next = self.playback_queue_index + 1;
                                if next < self.playback_queue.len() {
                                    Some(next)
                                } else {
                                    // Wrapped around to the start
                                    Some(0)
                                }
                            }
                            LoopStatus::None => {
                                let next = self.playback_queue_index + 1;
                                if next < self.playback_queue.len() {
                                    Some(next)
                                } else {
                                    None // shouldn't happen — preload not sent
                                }
                            }
                        };

                        if let Some(idx) = new_index
                            && let Some(track) = self.playback_queue.get(idx).cloned()
                        {
                            self.playback_queue_index = idx;
                            self.now_playing = Some(crate::tidal::player::NowPlaying {
                                track_id: track.id.clone(),
                                title: track.title.clone(),
                                artist: track.artist_name.clone(),
                                album: track.album_name.clone(),
                                duration: track.duration as f64,
                                cover_url: track.cover_url.clone(),
                                playlist_name: self
                                    .now_playing
                                    .as_ref()
                                    .and_then(|np| np.playlist_name.clone()),
                            });
                            self.playback_position = 0.0;
                            self.playback_state = PlaybackState::Playing;

                            // Record this track in the local play history
                            self.play_history.record(&track);
                            {
                                let client = self.tidal_client.blocking_lock();
                                self.play_history.save(client.api_cache());
                            }

                            // Open a TIDAL play-attribution session for the
                            // gaplessly-transitioned new track (finalises the
                            // previous track's session as part of the open).
                            self.open_play_session(&track);

                            // Preload the *next* next track + update MPRIS
                            let preload_task =
                                Task::done(cosmic::Action::App(Message::PreloadNextTrack));
                            let mpris_task = self.update_mpris_state();

                            // Load cover art for the new track if needed
                            let mut tasks = vec![preload_task, mpris_task];
                            if let Some(cover_url) = &track.cover_url
                                && !self.loaded_images.contains_key(cover_url)
                                && !self.pending_image_loads.contains(cover_url)
                            {
                                tasks.push(self.load_images_for_urls(vec![cover_url.clone()]));
                            }
                            return Task::batch(tasks);
                        }
                    }
                    crate::tidal::player::PlayerEvent::Error(e) => {
                        tracing::error!("Playback error: {}", e);
                        self.error_message = Some(format!("Playback error: {}", e));
                    }
                    crate::tidal::player::PlayerEvent::StateChanged(new_state) => {
                        // Engine state transitions (e.g. Loading → Playing)
                        // override the app-level state so the UI reflects
                        // buffering vs actual playback.
                        if new_state != self.playback_state {
                            tracing::debug!(
                                "Engine state: {:?} -> {:?}",
                                self.playback_state,
                                new_state
                            );
                            let was_loading = self.playback_state == PlaybackState::Loading;
                            self.playback_state = new_state;
                            let is_playing = new_state == PlaybackState::Playing;
                            self.visualizer_state.set_active(is_playing);

                            // Reset progress when we leave Loading
                            if new_state != PlaybackState::Loading {
                                self.loading_progress = 1.0;
                            }

                            // When a track transitions from Loading → Playing,
                            // kick off preloading the next track for gapless
                            // playback.
                            if was_loading && is_playing {
                                return Task::done(cosmic::Action::App(Message::PreloadNextTrack));
                            }
                        }
                    }
                    crate::tidal::player::PlayerEvent::LoadingProgress(progress) => {
                        self.loading_progress = progress as f32;
                    }
                }
            }
        }

        // Check if volume bar should be hidden (after ~1 second)
        if self.show_volume_bar
            && let Some(shown_at) = self.volume_bar_shown_at
            && shown_at.elapsed() > Duration::from_millis(1000)
        {
            self.show_volume_bar = false;
            self.volume_bar_shown_at = None;
        }

        Task::none()
    }

    /// Handle volume adjustment from mouse wheel on panel button
    pub fn handle_adjust_volume(&mut self, delta: f32) -> Task<cosmic::Action<Message>> {
        // Adjust volume by delta (typically ±0.05 per scroll step)
        let new_volume = (self.volume_level + delta).clamp(0.0, 1.0);
        self.volume_level = new_volume;

        // Apply to player
        if let Some(player) = &self.player
            && let Err(e) = player.set_volume(new_volume)
        {
            tracing::warn!("Failed to set volume: {}", e);
        }

        // Persist volume to config
        self.config.volume_level = new_volume;
        if let Ok(config_context) =
            cosmic::cosmic_config::Config::new(Self::APP_ID, crate::config::Config::VERSION)
            && let Err(e) = self.config.write_entry(&config_context)
        {
            tracing::error!("Failed to save volume config: {}", e);
        }

        // Show volume bar and reset timeout
        self.show_volume_bar = true;
        self.volume_bar_shown_at = Some(Instant::now());

        tracing::info!(
            "Volume adjusted to {:.0}% (delta: {:.2}, show_bar: {})",
            new_volume * 100.0,
            delta,
            self.show_volume_bar
        );
        Task::none()
    }

    /// Preload the next track in the queue for gapless playback.
    ///
    /// This is triggered after a track starts playing (or after a gapless
    /// transition) so the engine has the next track's audio data ready
    /// before the current one ends.
    ///
    /// Respects loop modes:
    /// - `LoopStatus::Track`: preloads the *same* track for seamless repeat.
    /// - `LoopStatus::Playlist`: wraps around to track 0 at end of queue.
    /// - `LoopStatus::None`: no preload at end of queue.
    pub fn handle_preload_next_track(&mut self) -> Task<cosmic::Action<Message>> {
        use crate::tidal::mpris::LoopStatus;

        let preload_index = match self.loop_status {
            LoopStatus::Track => {
                // Repeat-track: preload the same track for gapless repeat
                Some(self.playback_queue_index)
            }
            LoopStatus::Playlist => {
                let next = self.playback_queue_index + 1;
                if next < self.playback_queue.len() {
                    Some(next)
                } else if !self.playback_queue.is_empty() {
                    // Wrap around to the start
                    Some(0)
                } else {
                    None
                }
            }
            LoopStatus::None => {
                let next = self.playback_queue_index + 1;
                if next < self.playback_queue.len() {
                    Some(next)
                } else {
                    None
                }
            }
        };

        let Some(next_index) = preload_index else {
            tracing::debug!("No next track to preload (end of queue, no loop)");
            return Task::none();
        };

        let Some(track) = self.playback_queue.get(next_index).cloned() else {
            tracing::debug!("No track at preload index {}", next_index);
            return Task::none();
        };

        tracing::info!(
            "Preloading next track [{}/{}]: {} - {} (loop: {:?})",
            next_index + 1,
            self.playback_queue.len(),
            track.artist_name,
            track.title,
            self.loop_status,
        );

        let track_id = track.id.clone();
        let client = self.tidal_client.clone();

        Task::perform(
            async move {
                let client = client.lock().await;
                match client.get_track_playback_url(&track_id).await {
                    Ok(url) => Ok((track, url)),
                    Err(e) => Err(e.to_string()),
                }
            },
            |result| cosmic::Action::App(Message::PreloadUrlReceived(result)),
        )
    }

    /// Handle a preload URL response — feed it to the engine's preload buffer.
    pub fn handle_preload_url_received(
        &mut self,
        result: Result<(Track, PlaybackUrl), String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok((track, playback_url)) => {
                if let Some(player) = &self.player {
                    let replay_gain_db = playback_url.replay_gain_db();

                    // Same guard as handle_playback_url_received: skip
                    // audio_cache_path_for (and its reserve_room) for
                    // cache hits so we never trigger spurious eviction.
                    let preload_result = if playback_url.is_cached() {
                        let path = playback_url.as_url();
                        tracing::info!("Preloading from cache: {}", path);
                        player.preload_file(&path, replay_gain_db)
                    } else if playback_url.is_dash() {
                        let cache_path = {
                            let client = self.tidal_client.blocking_lock();
                            let p = client.audio_cache_path_for(&track.id);
                            p.to_string_lossy().to_string()
                        };
                        let manifest_path = playback_url.as_url();
                        tracing::info!("Preloading HiRes DASH: {}", manifest_path);
                        player.preload_dash(&manifest_path, Some(cache_path), replay_gain_db)
                    } else {
                        let cache_path = {
                            let client = self.tidal_client.blocking_lock();
                            let p = client.audio_cache_path_for(&track.id);
                            p.to_string_lossy().to_string()
                        };
                        let url = playback_url.as_url();
                        tracing::info!("Preloading URL: {}", &url[..url.len().min(60)]);
                        player.preload_url(&url, Some(cache_path), replay_gain_db)
                    };

                    if let Err(e) = preload_result {
                        tracing::warn!(
                            "Preload failed (will fall back to normal transition): {}",
                            e
                        );
                    } else {
                        tracing::info!("Next track preloaded for gapless playback");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to get preload URL (gapless won't work for next transition): {}",
                    e
                );
            }
        }
        Task::none()
    }

    /// Handle a gapless transition message from the UI layer.
    ///
    /// Most of the real work (queue advancement, metadata update) is done
    /// inside the `PlayerEvent::PreloadConsumed` arm of `handle_playback_tick`.
    /// This message exists so other parts of the app can react to the
    /// transition (e.g. update MPRIS metadata).
    pub fn handle_gapless_transition(&mut self) -> Task<cosmic::Action<Message>> {
        tracing::info!("Gapless transition acknowledged");
        self.update_mpris_state()
    }

    // ═══ TIDAL play attribution helpers ════════════════════════════════════

    /// Open a new in-progress play session for `track`.
    ///
    /// Closes the previous session first (if any) so a quick track switch
    /// still finalizes the prior listen — a play that crossed the 30s /
    /// 50% threshold gets reported even if the user skips to the next one.
    pub(crate) fn open_play_session(&mut self, track: &Track) {
        // Finalize whatever was playing before (skip/replace case).
        self.finalize_and_report_play_session();

        // Map the user's configured quality preference to the TIDAL
        // string the event-producer expects.  TIDAL's modern hi-res tier
        // is wire-named HI_RES_LOSSLESS even though tidlers' Display
        // emits the older HI_RES; the latter is no longer accepted in
        // playback_session events.
        let quality = match self.config.audio_quality {
            crate::config::AudioQuality::Low => "LOW",
            crate::config::AudioQuality::High => "HIGH",
            crate::config::AudioQuality::Lossless => "LOSSLESS",
            crate::config::AudioQuality::HiRes => "HI_RES_LOSSLESS",
        };

        // Use the threaded container source (ALBUM / PLAYLIST / MIX /
        // ARTIST) when available so TIDAL's play_log consumer surfaces
        // the play in Recently Played.  Falls back to TRACK/track_id
        // for ad-hoc plays (favorites list, history view, MPRIS OpenUri
        // single-track playback) — those still credit royalties and
        // count toward 'Most Listened' aggregates but won't appear in
        // Recently Played per the SDK source.
        let (source_type, source_id) = match &self.playback_source {
            Some(s) => {
                // For Track-kind ad-hoc sources, prefer the live track's id
                // so each listen attributes individually; the source.id is
                // a placeholder in that case.
                let id = if matches!(s.kind, crate::tidal::models::PlaybackSourceKind::Track) {
                    track.id.clone()
                } else {
                    s.id.clone()
                };
                (Some(s.kind.as_tidal_str().to_string()), Some(id))
            }
            None => (Some("TRACK".to_string()), Some(track.id.clone())),
        };

        self.current_play = Some(crate::tidal::play_reporter::InProgressPlay::open(
            track.id.clone(),
            quality.to_string(),
            source_type,
            source_id,
            0.0,
            track.duration as f64,
        ));
    }

    /// Finalize the current in-progress session and dispatch to the
    /// reporter if the listen exceeded the threshold (30s OR 50%).
    ///
    /// Safe to call when no session is open (no-op).  Drops the session
    /// either way, so callers should `open_play_session` before the next
    /// listen if they want it tracked.
    pub(crate) fn finalize_and_report_play_session(&mut self) {
        let Some(in_progress) = self.current_play.take() else {
            return;
        };
        if !in_progress.meets_threshold() {
            tracing::debug!(
                track = %in_progress.track_id,
                listened = in_progress.last_position_s - in_progress.start_position_s,
                duration = in_progress.duration_s,
                "play below threshold; skipping report",
            );
            return;
        }
        // Snapshot the access token from the TIDAL client.  Uses
        // `try_lock` internally; if the client lock is contended we
        // skip reporting this one play rather than block the UI thread.
        let token = {
            let client = self.tidal_client.blocking_lock();
            client.current_access_token()
        };
        let Some(token) = token else {
            tracing::debug!(
                track = %in_progress.track_id,
                "no access token available; skipping report",
            );
            return;
        };
        let end_ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.play_reporter
            .record(in_progress.finalize(end_ts_ms, token));
    }
}
