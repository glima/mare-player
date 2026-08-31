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
        // Tear down any in-flight video before (re)starting playback.
        self.stop_video();
        // Tear down any in-flight GStreamer audio pipeline (dropping it sets
        // the pipeline to Null). The next track builds a fresh one.
        self.media_player = None;

        self.playback_position = 0.0;
        self.video_resume_target = None;
        self.loading_progress = 0.0;
        self.playback_state = PlaybackState::Loading;

        // Music videos play through a video pipeline; tear down any audio
        // pipeline first so the two never overlap, then resolve the HLS URL.
        if track.is_video {
            self.visualizer_state.set_active(false);
            let video_id = track.id.clone();
            let client = self.tidal_client.clone();
            return Task::perform(
                async move {
                    let client = client.lock().await;
                    match client.get_video_hls_url(&video_id).await {
                        Ok(url) => Ok((track, url)),
                        Err(e) => Err(e.to_string()),
                    }
                },
                |result| cosmic::Action::App(Message::VideoUrlReceived(result)),
            );
        }

        let track_id = track.id.clone();
        let client = self.tidal_client.clone();
        // Switching to an audio track: if a video was popped out into its own
        // window, kill it — there's no video to show anymore.
        self.close_video_window_if_open();
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

    /// Tear down any active video pipeline (dropping the GStreamer pipeline
    /// sets it to `Null`).  Safe to call when no video is active.
    pub(crate) fn stop_video(&mut self) {
        if self.video_player.take().is_some() {
            tracing::info!("Video pipeline stopped");
        }
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
        tracing::info!("Play single track requested: {} (source: {:?})", track, source.as_ref().map(|s| (&s.kind, &s.id)));
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
            if let Some(current_track) = self.playback_queue.get(self.playback_queue_index).cloned() {
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
            Ok((track, playback_url)) => self.start_gst_audio(track, playback_url),
            Err(e) => {
                tracing::error!("Failed to get playback URL: {}", e);
                self.error_message = Some(format!("Failed to get playback URL: {}", e));
                Task::none()
            }
        }
    }

    /// Start an audio track through the GStreamer [`MediaPlayer`].
    ///
    /// Builds a GStreamer URI from the
    /// resolved [`PlaybackUrl`], starts the pipeline with the track's album
    /// replay gain feeding the `rg` volume element and the shared spectrum
    /// analyzer driving the visualizer, then updates now-playing, history,
    /// the play-attribution session, and MPRIS.
    ///
    /// Song disk-caching is intentionally skipped here: capturing the encoded
    /// stream (multi-segment DASH especially) is hard and is deferred.
    /// Previously-cached files still play instantly via a `file://` URI.
    fn start_gst_audio(&mut self, track: Track, playback_url: PlaybackUrl) -> Task<cosmic::Action<Message>> {
        let now_playing = NowPlaying {
            track_id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist_name.clone(),
            album: track.album_name.clone(),
            cover_url: track.cover_url.clone(),
            duration: track.duration as f64,
            playlist_name: self.playback_source.as_ref().map(|s| s.display_name.clone()),
        };

        // Tracks carry TIDAL's authored album replay gain; unity (0 dB) when
        // the API didn't provide one.
        let replay_gain_db = playback_url.replay_gain_db().unwrap_or(0.0);

        // Badge what TIDAL actually served, not what we asked for.
        self.now_playing_quality = playback_url.stream_quality();

        // `as_url()` returns a ready-to-use GStreamer URI: an http(s) URL for
        // direct qualities, or an inline `data:application/dash+xml` URI for
        // DASH (HiRes) — nothing is written to disk.
        let uri = playback_url.as_url();

        tracing::info!("GStreamer audio: {} ({})", track, if playback_url.is_dash() { "DASH" } else { "direct" },);

        let analyzer = self.visualizer_state.analyzer();
        match crate::playback::MediaPlayer::new_audio(&uri, analyzer, replay_gain_db) {
            Ok(mp) => {
                mp.set_volume(self.volume_level as f64);
                self.media_player = Some(mp);
                self.gst_transitions_seen = 0;
                self.playback_state = PlaybackState::Playing;
                self.now_playing = Some(now_playing);
                self.playback_position = 0.0;
                self.visualizer_state.set_active(true);

                // Record in local play history and open a TIDAL
                // play-attribution session (finalises the previous one).
                self.play_history.record(&track);
                self.persist_play_history();
                self.open_play_session(&track);

                // Stage the next track for gapless playback.
                let preload_task = Task::done(cosmic::Action::App(Message::PreloadNextTrack));
                let mpris_task = self.update_mpris_state();
                let mut tasks = vec![preload_task, mpris_task];
                if let Some(cover_url) = &track.cover_url
                    && !self.loaded_images.contains_key(cover_url)
                    && !self.pending_image_loads.contains(cover_url)
                {
                    tasks.push(self.load_images_for_urls(vec![cover_url.clone()]));
                }
                tasks.push(self.refresh_now_playing_lyrics(&track));
                Task::batch(tasks)
            }
            Err(e) => {
                tracing::error!("GStreamer audio playback failed: {}", e);
                self.error_message = Some(format!("Playback failed: {}", e));
                self.playback_state = PlaybackState::Stopped;
                self.now_playing = None;
                Task::none()
            }
        }
    }

    /// Advance the queue and now-playing metadata after a gapless transition
    /// into the preloaded next track. Shared by the symphonia `PreloadConsumed`
    /// event and the GStreamer `about-to-finish` path. Determines the new index
    /// per loop mode, updates now-playing/history/session, and stages the
    /// following track's preload.
    fn handle_gapless_advance(&mut self) -> Task<cosmic::Action<Message>> {
        use crate::tidal::mpris::LoopStatus;
        let new_index = match self.loop_status {
            LoopStatus::Track => Some(self.playback_queue_index),
            LoopStatus::Playlist => {
                let next = self.playback_queue_index + 1;
                if next < self.playback_queue.len() { Some(next) } else { Some(0) }
            }
            LoopStatus::None => {
                let next = self.playback_queue_index + 1;
                if next < self.playback_queue.len() { Some(next) } else { None }
            }
        };

        if let Some(idx) = new_index
            && let Some(track) = self.playback_queue.get(idx).cloned()
        {
            self.playback_queue_index = idx;
            self.now_playing = Some(NowPlaying {
                track_id: track.id.clone(),
                title: track.title.clone(),
                artist: track.artist_name.clone(),
                album: track.album_name.clone(),
                duration: track.duration as f64,
                cover_url: track.cover_url.clone(),
                playlist_name: self.now_playing.as_ref().and_then(|np| np.playlist_name.clone()),
            });
            self.playback_position = 0.0;
            self.playback_state = PlaybackState::Playing;

            self.play_history.record(&track);
            self.persist_play_history();
            self.open_play_session(&track);

            let preload_task = Task::done(cosmic::Action::App(Message::PreloadNextTrack));
            let mpris_task = self.update_mpris_state();
            let mut tasks = vec![preload_task, mpris_task];
            if let Some(cover_url) = &track.cover_url
                && !self.loaded_images.contains_key(cover_url)
                && !self.pending_image_loads.contains(cover_url)
            {
                tasks.push(self.load_images_for_urls(vec![cover_url.clone()]));
            }
            tasks.push(self.refresh_now_playing_lyrics(&track));
            return Task::batch(tasks);
        }
        Task::none()
    }

    /// Handle the resolved HLS URL for a music video: start the GStreamer
    /// pipeline and surface it in the now-playing pane.
    pub fn handle_video_url_received(&mut self, result: Result<(Track, String), String>) -> Task<cosmic::Action<Message>> {
        match result {
            Ok((track, url)) => {
                self.current_video_url = Some(url.clone());

                // If the video is popped out, hand the new stream to the child
                // window instead of building an inline pipeline. A failed write
                // means the child died without us having processed its `closed`
                // event yet, so drop the stale handle and fall through to
                // inline playback rather than pretending the track is playing.
                if self.video_window.is_some() {
                    let delivered = self.video_window.as_mut().is_some_and(|child| child.send(&format!("play 0 {url}")));
                    if delivered {
                        self.playback_state = PlaybackState::Playing;
                        self.playback_position = 0.0;
                        self.now_playing = Some(NowPlaying {
                            track_id: track.id.clone(),
                            title: track.title.clone(),
                            artist: track.artist_name.clone(),
                            album: track.album_name.clone(),
                            cover_url: track.cover_url.clone(),
                            duration: track.duration as f64,
                            playlist_name: self.playback_source.as_ref().map(|s| s.display_name.clone()),
                        });
                        self.play_history.record(&track);
                        self.persist_play_history();
                        tracing::info!("Video handed to pop-out window: {}", track);
                        return Task::batch([self.update_mpris_state(), self.refresh_now_playing_lyrics(&track)]);
                    }
                    tracing::warn!("Pop-out window is gone (broken pipe); playing {} inline instead", track);
                    self.video_window = None;
                }

                match crate::playback::MediaPlayer::new_video(&url, self.visualizer_state.analyzer(), self.config.video_preamp_db)
                {
                    Ok(video) => {
                        video.set_volume(self.volume_level as f64);
                        self.video_player = Some(video);
                        // Show the overlay controls briefly when playback starts.
                        self.video_controls_shown_at = Some(std::time::Instant::now());
                        self.playback_state = PlaybackState::Playing;
                        // The video pipeline taps its decoded audio into the shared
                        // spectrum analyzer, so the now-playing visualizer animates
                        // to the music video just as it does for audio tracks.
                        self.visualizer_state.set_active(true);
                        self.now_playing = Some(NowPlaying {
                            track_id: track.id.clone(),
                            title: track.title.clone(),
                            artist: track.artist_name.clone(),
                            album: track.album_name.clone(),
                            cover_url: track.cover_url.clone(),
                            duration: track.duration as f64,
                            playlist_name: self.playback_source.as_ref().map(|s| s.display_name.clone()),
                        });
                        self.playback_position = 0.0;
                        self.play_history.record(&track);
                        self.persist_play_history();
                        tracing::info!("Video playback started: {}", track);
                        return Task::batch([self.update_mpris_state(), self.refresh_now_playing_lyrics(&track)]);
                    }
                    Err(e) => {
                        tracing::error!("Failed to start video pipeline: {}", e);
                        self.error_message = Some(format!("Failed to play video: {}", e));
                        self.playback_state = PlaybackState::Stopped;
                        self.now_playing = None;
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to resolve video URL: {}", e);
                self.error_message = Some(format!("Failed to load video: {}", e));
                self.playback_state = PlaybackState::Stopped;
                self.now_playing = None;
            }
        }
        Task::none()
    }

    /// Handle seek to position (percentage)
    pub fn handle_seek_to(&mut self, percent: f64) -> Task<cosmic::Action<Message>> {
        if let Some(np) = &self.now_playing
            && np.duration > 0.0
        {
            let target_pos = (percent / 100.0) * np.duration;
            tracing::info!("SeekTo: {}% -> {:.2}s (duration: {:.2}s)", percent, target_pos, np.duration);
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
            // Popped-out video: forward the seek to the child window.
            if self.video_window.is_some() {
                if let Some(child) = self.video_window.as_mut() {
                    // A dead pipe is handled by the `closed` event; nothing
                    // useful to do with a failed seek here.
                    let _ = child.send(&format!("seek {target_pos:.3}"));
                }
                return self.update_mpris_state();
            }
            if let Some(video) = &self.video_player {
                video.seek_secs(target_pos);
                return self.update_mpris_state();
            }
            if let Some(mp) = &self.media_player {
                mp.seek_secs(target_pos);
                return self.update_mpris_state();
            }
        }
        Task::none()
    }

    /// Handle toggle play/pause
    pub fn handle_toggle_play_pause(&mut self) -> Task<cosmic::Action<Message>> {
        // Popped-out video: drive the child over its stdin pipe.
        if self.video_window.is_some() {
            let new_state = match self.playback_state {
                PlaybackState::Playing => PlaybackState::Paused,
                PlaybackState::Paused => PlaybackState::Playing,
                other => other,
            };
            if let Some(child) = self.video_window.as_mut() {
                match new_state {
                    PlaybackState::Paused => {
                        let _ = child.send("pause");
                    }
                    PlaybackState::Playing => {
                        let _ = child.send("resume");
                    }
                    _ => {}
                }
            }
            self.playback_state = new_state;
            return self.update_mpris_state();
        }

        // Video path: drive the GStreamer pipeline directly.
        if let Some(video) = &self.video_player {
            let new_state = match self.playback_state {
                PlaybackState::Playing => {
                    video.pause();
                    PlaybackState::Paused
                }
                PlaybackState::Paused => {
                    video.resume();
                    PlaybackState::Playing
                }
                other => other,
            };
            self.playback_state = new_state;
            // Mirror the audio path: settle the visualizer when paused (no PCM
            // flows while the pipeline is stopped), re-arm it when resuming.
            self.visualizer_state.set_active(new_state == PlaybackState::Playing);
            return self.update_mpris_state();
        }

        // GStreamer audio path: same direct pipeline control as video.
        if let Some(mp) = &self.media_player {
            let new_state = match self.playback_state {
                PlaybackState::Playing => {
                    mp.pause();
                    PlaybackState::Paused
                }
                PlaybackState::Paused => {
                    mp.resume();
                    PlaybackState::Playing
                }
                other => other,
            };
            self.playback_state = new_state;
            self.visualizer_state.set_active(new_state == PlaybackState::Playing);
            return self.update_mpris_state();
        }

        Task::none()
    }

    /// Handle stop playback
    pub fn handle_stop_playback(&mut self) -> Task<cosmic::Action<Message>> {
        // Finalise the in-progress TIDAL play-attribution session, if any,
        // before tearing down playback state.  Reports the listen if it
        // crossed the threshold.
        self.finalize_and_report_play_session();
        // Popped-out video: kill the child window and stop.
        if self.video_window.is_some() {
            self.close_video_window_if_open();
            self.playback_state = PlaybackState::Stopped;
            self.now_playing = None;
            self.playback_position = 0.0;
            self.visualizer_state.set_active(false);
            return self.update_mpris_state();
        }
        if self.video_player.is_some() {
            self.stop_video();
            self.playback_state = PlaybackState::Stopped;
            self.now_playing = None;
            self.playback_position = 0.0;
            self.visualizer_state.set_active(false);
            self.close_video_window_if_open();
            return self.update_mpris_state();
        }
        // Dropping the audio pipeline sets it to Null.
        if self.media_player.take().is_some() {
            self.playback_state = PlaybackState::Stopped;
            self.now_playing = None;
            self.playback_position = 0.0;
            self.visualizer_state.set_active(false);
            return self.update_mpris_state();
        }
        Task::none()
    }

    /// Toggle the video pop-out: hand the current video to a separate child
    /// window (`mare-video-window`) and tear down inline playback, or kill the
    /// child and resume the video inline.
    pub fn handle_toggle_video_window(&mut self) -> Task<cosmic::Action<Message>> {
        // Already popped out → kill the child and resume inline from the last
        // reported position.
        if self.video_window.is_some() {
            let pos = self.playback_position;
            self.close_video_window_if_open();
            if let Some(url) = self.current_video_url.clone() {
                return self.resume_inline_video(&url, pos);
            }
            return Task::none();
        }

        // Pop out: only meaningful while a video is playing inline and we know
        // its URL.
        if self.video_player.is_none() {
            return Task::none();
        }
        let Some(url) = self.current_video_url.clone() else {
            return Task::none();
        };
        let pos = self.playback_position;
        let vol = self.volume_level;
        let preamp = self.config.video_preamp_db;

        // Tear down the inline pipeline first: the child owns audio+video while
        // popped out, so the parent must not also decode (echo + drift).
        self.stop_video();

        match crate::playback::VideoWindowChild::spawn(&url, pos, vol, preamp, self.video_window_tx.clone()) {
            Some(child) => {
                self.video_window = Some(child);
                tracing::info!("Video popped out into child window at {:.1}s", pos);
                Task::none()
            }
            None => {
                // Couldn't launch the companion — stay inline.
                tracing::error!("could not launch mare-video-window; staying inline");
                self.resume_inline_video(&url, pos)
            }
        }
    }

    /// Rebuild the inline video [`MediaPlayer`] for `url`, seeking to `pos`.
    /// Used when popping a video back in (button, window-closed, or failed
    /// spawn).
    pub(crate) fn resume_inline_video(&mut self, url: &str, pos: f64) -> Task<cosmic::Action<Message>> {
        match crate::playback::MediaPlayer::new_video_at(url, self.visualizer_state.analyzer(), self.config.video_preamp_db, pos)
        {
            Ok(video) => {
                tracing::info!("Resuming video inline at {:.1}s", pos);
                video.set_volume(self.volume_level as f64);
                self.video_player = Some(video);
                self.video_controls_shown_at = Some(std::time::Instant::now());
                self.playback_state = PlaybackState::Playing;
                self.playback_position = pos;
                // Hold the slider at `pos` until the deferred seek lands, so the
                // fresh pipeline's pre-seek 0:00 doesn't flash on the bar.
                self.video_resume_target = Some((pos, std::time::Instant::now()));
                self.visualizer_state.set_active(true);
                self.update_mpris_state()
            }
            Err(e) => {
                tracing::error!("Failed to resume inline video: {}", e);
                self.error_message = Some(format!("Failed to play video: {}", e));
                self.playback_state = PlaybackState::Stopped;
                self.now_playing = None;
                Task::none()
            }
        }
    }

    /// Handle a raw event line from the popped-out video child's stdout.
    /// Lines are `position <secs>`, `eos`, or `closed`.
    pub fn handle_video_window_event(&mut self, line: String) -> Task<cosmic::Action<Message>> {
        // Ignore late events after the child was already torn down.
        if self.video_window.is_none() {
            return Task::none();
        }
        let (kind, rest) = match line.split_once(' ') {
            Some((k, r)) => (k, r.trim()),
            None => (line.as_str(), ""),
        };
        match kind {
            "position" => {
                if let Ok(pos) = rest.parse::<f64>() {
                    self.playback_position = pos;
                    // Keep the play-attribution session's high-water mark fresh.
                    if let Some(p) = &mut self.current_play {
                        p.observe_position(pos);
                    }
                    // Drive the karaoke-style highlight in the lyrics view.
                    if matches!(self.view_state, crate::state::ViewState::Lyrics)
                        && let (Some(track), Some(lyrics)) =
                            (self.playback_queue.get(self.playback_queue_index), self.selected_track_lyrics.as_ref())
                        && self.selected_lyrics_track.as_ref().is_some_and(|t| t.id == track.id)
                    {
                        let next = lyrics.line_index_at(pos);
                        if next != self.current_lyric_index {
                            self.current_lyric_index = next;
                        }
                    }
                }
                Task::none()
            }
            "eos" => self.handle_video_window_eos(),
            "closed" => {
                // The user closed the child window → pop back in and resume
                // inline from the last reported position.
                let pos = self.playback_position;
                self.video_window = None;
                tracing::info!("Video window closed by user; resuming inline");
                if let Some(url) = self.current_video_url.clone() { self.resume_inline_video(&url, pos) } else { Task::none() }
            }
            _ => Task::none(),
        }
    }

    /// Advance the queue when the popped-out video reaches its end, mirroring
    /// the inline video tick's end-of-stream handling. The child stays alive
    /// for video→video transitions (the new URL is sent via `play`); it is
    /// killed when the next track is audio or the queue ends.
    fn handle_video_window_eos(&mut self) -> Task<cosmic::Action<Message>> {
        match self.loop_status {
            LoopStatus::Track => self.play_track_at_index(self.playback_queue_index),
            _ => {
                let next_index = self.playback_queue_index + 1;
                if next_index < self.playback_queue.len() {
                    self.playback_queue_index = next_index;
                    self.play_track_at_index(next_index)
                } else if self.loop_status == LoopStatus::Playlist {
                    self.playback_queue_index = 0;
                    self.play_track_at_index(0)
                } else {
                    // End of a video playlist: stop and dismiss the child window.
                    self.close_video_window_if_open();
                    self.playback_state = PlaybackState::Stopped;
                    self.now_playing = None;
                    self.playback_position = 0.0;
                    self.visualizer_state.set_active(false);
                    self.update_mpris_state()
                }
            }
        }
    }

    /// Kill the popped-out video child if one is running. No-op otherwise.
    /// Used when video playback ends, switches to audio, or stops.
    pub(crate) fn close_video_window_if_open(&mut self) {
        if let Some(mut child) = self.video_window.take() {
            // Best-effort graceful quit; `kill` is the guarantee.
            let _ = child.send("quit");
            child.kill();
            tracing::info!("Video child window closed");
        }
    }

    /// Handle playback tick — updates position, processes engine events, and
    /// hides the volume bar after a timeout.
    ///
    /// The visualizer does not need updating here: it is a self-animating
    /// widget that reads spectrum data directly from the shared
    /// `SharedSpectrumAnalyzer` and drives its own redraws via
    /// `shell.request_redraw()`.
    pub fn handle_playback_tick(&mut self) -> Task<cosmic::Action<Message>> {
        // Video path: position comes from the GStreamer pipeline, and we
        // advance on EOS the same way the audio engine's TrackEnded does.
        if self.video_player.is_some() {
            if self.playback_state == PlaybackState::Playing
                && let Some(video) = &self.video_player
                && let Some(pos) = video.position_secs()
            {
                // After a pop-in we hold the slider at the resume target until
                // the deferred seek lands (the pipeline reports ~0 until then),
                // with a timeout fallback so a failed seek can't freeze it.
                match self.video_resume_target {
                    Some((target, since)) if pos + 1.0 < target && since.elapsed() < Duration::from_secs(6) => {
                        self.playback_position = target;
                    }
                    Some(_) => {
                        self.video_resume_target = None;
                        self.playback_position = pos;
                    }
                    None => {
                        self.playback_position = pos;
                    }
                }
            }
            let ended = self.video_player.as_ref().is_some_and(|v| v.is_eos() || v.poll());
            if ended {
                self.stop_video();
                match self.loop_status {
                    LoopStatus::Track => {
                        return self.play_track_at_index(self.playback_queue_index);
                    }
                    _ => {
                        let next_index = self.playback_queue_index + 1;
                        if next_index < self.playback_queue.len() {
                            return Task::done(cosmic::Action::App(Message::NextTrack));
                        } else if self.loop_status == LoopStatus::Playlist {
                            self.playback_queue_index = 0;
                            return self.play_track_at_index(0);
                        } else {
                            // Video playlist ended: stop and close the pop-out
                            // window if it's open.
                            self.playback_state = PlaybackState::Stopped;
                            self.now_playing = None;
                            self.visualizer_state.set_active(false);
                            self.close_video_window_if_open();
                            return Task::none();
                        }
                    }
                }
            }
            // Volume-bar auto-hide (mirrors the audio tail).
            if let Some(shown_at) = self.volume_bar_shown_at
                && shown_at.elapsed() > Duration::from_millis(1000)
            {
                self.show_volume_bar = false;
                self.volume_bar_shown_at = None;
            }
            return Task::none();
        }

        // GStreamer audio path: position comes from the pipeline, and we
        // advance on EOS the same way the symphonia engine's TrackEnded does.
        if self.media_player.is_some() {
            // Drain the bus first so EOS/errors/transitions are observed.
            let errored = self.media_player.as_ref().is_some_and(|mp| mp.poll());
            let eos = self.media_player.as_ref().is_some_and(|mp| mp.is_eos());
            let ended = errored || eos;

            // Gapless transition: a staged next track started playing without
            // a pipeline rebuild. Advance the queue + metadata to match.
            let transitions = self.media_player.as_ref().map_or(0, |mp| mp.transitions());
            if transitions > self.gst_transitions_seen {
                self.gst_transitions_seen = transitions;
                tracing::info!("GStreamer gapless transition observed");
                return self.handle_gapless_advance();
            }

            if self.playback_state == PlaybackState::Playing
                && let Some(pos) = self.media_player.as_ref().and_then(|mp| mp.position_secs())
            {
                self.playback_position = pos;
                // Keep the play-attribution session's high-water mark fresh.
                if let Some(p) = &mut self.current_play {
                    p.observe_position(pos);
                }
                // Drive the karaoke-style highlight in the lyrics view.
                if matches!(self.view_state, crate::state::ViewState::Lyrics)
                    && let (Some(track), Some(lyrics)) =
                        (self.playback_queue.get(self.playback_queue_index), self.selected_track_lyrics.as_ref())
                    && self.selected_lyrics_track.as_ref().is_some_and(|t| t.id == track.id)
                {
                    let next = lyrics.line_index_at(pos);
                    if next != self.current_lyric_index {
                        self.current_lyric_index = next;
                    }
                }
            }

            if ended {
                tracing::info!("GStreamer audio ended (errored={errored}, eos={eos}, state={:?})", self.playback_state);
                self.media_player = None;
                match self.loop_status {
                    LoopStatus::Track => {
                        return self.play_track_at_index(self.playback_queue_index);
                    }
                    _ => {
                        let next_index = self.playback_queue_index + 1;
                        if next_index < self.playback_queue.len() {
                            return Task::done(cosmic::Action::App(Message::NextTrack));
                        } else if self.loop_status == LoopStatus::Playlist {
                            self.playback_queue_index = 0;
                            return self.play_track_at_index(0);
                        } else {
                            self.playback_state = PlaybackState::Stopped;
                            self.now_playing = None;
                            self.visualizer_state.set_active(false);
                        }
                    }
                }
            }

            // Volume-bar auto-hide (mirrors the audio tail).
            if let Some(shown_at) = self.volume_bar_shown_at
                && shown_at.elapsed() > Duration::from_millis(1000)
            {
                self.show_volume_bar = false;
                self.volume_bar_shown_at = None;
            }
            return Task::none();
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

        // Apply to the video pipeline too, if one is active.
        if let Some(video) = &self.video_player {
            video.set_volume(new_volume as f64);
        }
        // Apply to the popped-out video child, if one is running.
        if let Some(child) = self.video_window.as_mut() {
            let _ = child.send(&format!("volume {new_volume}"));
        }
        // Apply to the audio pipeline too, if one is active.
        if let Some(mp) = &self.media_player {
            mp.set_volume(new_volume as f64);
        }

        // Persist volume to config
        self.config.volume_level = new_volume;
        if let Ok(config_context) = cosmic::cosmic_config::Config::new(Self::APP_ID, crate::config::Config::VERSION)
            && let Err(e) = self.config.write_entry(&config_context)
        {
            tracing::error!("Failed to save volume config: {}", e);
        }

        // Show volume bar and reset timeout
        self.show_volume_bar = true;
        self.volume_bar_shown_at = Some(Instant::now());

        tracing::info!("Volume adjusted to {:.0}% (delta: {:.2}, show_bar: {})", new_volume * 100.0, delta, self.show_volume_bar);
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
                if next < self.playback_queue.len() { Some(next) } else { None }
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
            "Preloading next track [{}/{}]: {} (loop: {:?})",
            next_index + 1,
            self.playback_queue.len(),
            track,
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

    /// Handle a preload URL response — stage it into the pipeline for gapless
    /// playback (consumed by about-to-finish).
    pub fn handle_preload_url_received(&mut self, result: Result<(Track, PlaybackUrl), String>) -> Task<cosmic::Action<Message>> {
        match result {
            Ok((track, playback_url)) => {
                if let Some(mp) = &self.media_player {
                    let rg = playback_url.replay_gain_db().unwrap_or(0.0);
                    // Ready-to-use GStreamer URI (http(s) or inline data: DASH).
                    let uri = playback_url.as_url();
                    mp.set_next(uri, rg);
                    tracing::info!("Gapless: staged next track: {}", track);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get preload URL (gapless won't work for next transition): {}", e);
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
        let end_ts_ms =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        self.play_reporter.record(in_progress.finalize(end_ts_ms, token));
    }
}
