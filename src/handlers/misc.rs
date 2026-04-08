// SPDX-License-Identifier: MIT

//! Miscellaneous message handlers for Maré Player.
//!
//! This module handles configuration updates, error management,
//! image loading, sharing (song.link), MPRIS integration, and
//! in-app screenshot capture (**Ctrl+Shift+S**).

use crate::config::{AudioQuality, Config};
use crate::image_cache::make_circular;
use crate::messages::{Message, MprisStartResult};
use crate::state::{AppModel, ViewState};
use crate::tidal::mpris::{
    LoopStatus, MprisCommand, MprisMetadata, MprisPlaybackStatus, MprisState, MprisTrackEntry,
    track_object_path,
};
use crate::tidal::player::PlaybackState;
use cosmic::Application;
use cosmic::cosmic_config::CosmicConfigEntry;

use cosmic::prelude::*;

impl AppModel {
    /// Handle subscription channel event (startup)
    pub fn handle_subscription_channel(&mut self) -> Task<cosmic::Action<Message>> {
        // Try to restore session on startup
        if !self.session_restore_attempted {
            self.session_restore_attempted = true;
            return self.restore_session();
        }
        Task::none()
    }

    /// Handle config update
    pub fn handle_update_config(&mut self, config: Config) {
        self.config = config;
    }

    /// Handle clear error message
    pub fn handle_clear_error(&mut self) {
        self.error_message = None;
    }

    /// Handle load image request
    pub fn handle_load_image(&mut self, url: String) -> Task<cosmic::Action<Message>> {
        // Skip if already loaded or pending
        if self.loaded_images.contains_key(&url) || self.pending_image_loads.contains(&url) {
            return Task::none();
        }
        self.pending_image_loads.insert(url.clone());
        let cache = self.image_cache.clone();
        let url_clone = url.clone();
        Task::perform(
            async move {
                if let Some(cached) = cache.get_or_load(&url_clone).await {
                    Some((url_clone, cached.data.to_vec()))
                } else {
                    None
                }
            },
            |result| {
                if let Some((url, data)) = result {
                    cosmic::Action::App(Message::ImageLoaded(url, data))
                } else {
                    cosmic::Action::App(Message::ClearError) // No-op
                }
            },
        )
    }

    /// Handle image loaded
    pub fn handle_image_loaded(&mut self, url: String, data: Vec<u8>) {
        self.pending_image_loads.remove(&url);
        // Make the image circular before creating the handle
        let circular_data = match make_circular(&data) {
            Ok(circular) => circular,
            Err(e) => {
                tracing::warn!("Failed to make image circular: {}, using original", e);
                data
            }
        };
        let handle = cosmic::widget::image::Handle::from_bytes(circular_data);
        self.loaded_images.insert(url, handle);
    }

    /// Handle set audio cache max size in megabytes
    pub fn handle_set_audio_cache_max_mb(&mut self, mb: u32) {
        tracing::info!("Setting audio cache max size to {} MB", mb);
        self.config.audio_cache_max_mb = mb;

        // Propagate to the client's DiskCache so eviction uses the new limit
        {
            let mut client = self.tidal_client.blocking_lock();
            client.set_audio_cache_max_mb(mb);
        }

        // Persist config to disk
        if let Ok(config_context) =
            cosmic::cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
            && let Err(e) = self.config.write_entry(&config_context)
        {
            tracing::error!("Failed to save audio cache config: {}", e);
        }
    }

    /// Handle clear audio cache
    pub fn handle_clear_audio_cache(&mut self) {
        tracing::info!("Clearing audio cache");
        let client = self.tidal_client.blocking_lock();
        client.clear_audio_cache();
    }

    /// Handle clear play history
    pub fn handle_clear_history(&mut self) {
        tracing::info!("Clearing play history");
        self.play_history.clear();
        let client = self.tidal_client.blocking_lock();
        self.play_history.save(client.api_cache());
    }

    /// Handle set audio quality
    pub fn handle_set_audio_quality(
        &mut self,
        quality: AudioQuality,
    ) -> Task<cosmic::Action<Message>> {
        // Update local config
        self.config.audio_quality = quality;

        // Persist config to disk
        if let Ok(config_context) =
            cosmic::cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
            && let Err(e) = self.config.write_entry(&config_context)
        {
            tracing::error!("Failed to save audio quality config: {}", e);
        }

        // Apply to the TIDAL client
        let client = self.tidal_client.clone();
        let tidlers_quality = quality.to_tidlers();
        Task::perform(
            async move {
                let mut client = client.lock().await;
                client.set_audio_quality(tidlers_quality).await;
            },
            |_| cosmic::Action::App(Message::ClearError), // No-op on completion
        )
    }

    /// Handle show share prompt
    pub fn handle_show_share_prompt(&mut self, track: crate::tidal::models::Track) {
        let track_id = track.id.clone();
        let track_title = track.title.clone();
        let album_id = track.album_id.clone();
        let album_title = track.album_name.clone();
        self.view_state = ViewState::SharePrompt(track_id, track_title, album_id, album_title);
    }

    /// Handle share track
    pub fn handle_share_track(
        &mut self,
        track_id: String,
        track_title: String,
    ) -> Task<cosmic::Action<Message>> {
        let tidal_url = format!("https://tidal.com/browse/track/{}", track_id);
        tracing::info!("Generating song.link for track: {}", track_title);
        // Return to previous view
        self.view_state = ViewState::Main;
        Task::perform(
            async move { crate::helpers::generate_songlink(&tidal_url).await },
            |result| cosmic::Action::App(Message::ShareLinkGenerated(result)),
        )
    }

    /// Handle share album
    pub fn handle_share_album(
        &mut self,
        album_id: String,
        album_title: String,
    ) -> Task<cosmic::Action<Message>> {
        let tidal_url = format!("https://tidal.com/browse/album/{}", album_id);
        tracing::info!("Generating song.link for album: {}", album_title);
        // Return to previous view
        self.view_state = ViewState::Main;
        Task::perform(
            async move { crate::helpers::generate_songlink(&tidal_url).await },
            |result| cosmic::Action::App(Message::ShareLinkGenerated(result)),
        )
    }

    /// Handle cancel share
    pub fn handle_cancel_share(&mut self) {
        self.view_state = ViewState::Main;
    }

    /// Handle share link generated
    pub fn handle_share_link_generated(
        &mut self,
        result: Result<String, String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(url) => {
                tracing::info!("Song.link generated: {}", url);
                // Copy to clipboard and open in browser
                let url_for_clipboard = url.clone();
                let url_for_browser = url.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::helpers::copy_to_clipboard(&url_for_clipboard).await {
                        tracing::warn!("Failed to copy to clipboard: {}", e);
                    }
                });
                tokio::spawn(async move {
                    if let Err(e) = crate::helpers::open_in_browser(&url_for_browser).await {
                        tracing::warn!("Failed to open in browser: {}", e);
                    }
                });
                // Show success message briefly
                self.error_message = Some(format!("Link copied & opened: {}", url));
                // Clear the message after a delay
                Task::perform(
                    async {
                        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                    },
                    |_| cosmic::Action::App(Message::ClearError),
                )
            }
            Err(e) => {
                tracing::error!("Failed to generate share link: {}", e);
                self.error_message = Some(format!("Failed to generate share link: {}", e));
                Task::none()
            }
        }
    }

    /// Handle MPRIS service started
    pub fn handle_mpris_service_started(
        &mut self,
        result: MprisStartResult,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok((handle, rx)) => {
                tracing::info!("MPRIS D-Bus service started successfully");
                self.mpris_handle = Some(handle);
                self.mpris_command_rx = Some(rx);
                // Update MPRIS with current state
                self.update_mpris_state()
            }
            Err(e) => {
                tracing::warn!("Failed to start MPRIS service: {}", e);
                Task::none()
            }
        }
    }

    /// Handle MPRIS command
    pub fn handle_mpris_command(&mut self, cmd: MprisCommand) -> Task<cosmic::Action<Message>> {
        tracing::debug!("Received MPRIS command: {:?}", cmd);
        match cmd {
            MprisCommand::Play => {
                if self.playback_state == crate::tidal::player::PlaybackState::Paused {
                    return Task::done(cosmic::Action::App(Message::TogglePlayPause));
                }
            }
            MprisCommand::Pause => {
                if self.playback_state == crate::tidal::player::PlaybackState::Playing {
                    return Task::done(cosmic::Action::App(Message::TogglePlayPause));
                }
            }
            MprisCommand::PlayPause => {
                return Task::done(cosmic::Action::App(Message::TogglePlayPause));
            }
            MprisCommand::Stop => {
                return Task::done(cosmic::Action::App(Message::StopPlayback));
            }
            MprisCommand::Next => {
                return Task::done(cosmic::Action::App(Message::NextTrack));
            }
            MprisCommand::Previous => {
                return Task::done(cosmic::Action::App(Message::PreviousTrack));
            }
            MprisCommand::Seek(position_us) => {
                // Convert microseconds to percentage
                if let Some(np) = &self.now_playing
                    && np.duration > 0.0
                {
                    let position_secs = position_us as f64 / 1_000_000.0;
                    let percent = (position_secs / np.duration) * 100.0;
                    return Task::done(cosmic::Action::App(Message::SeekTo(percent)));
                }
            }
            MprisCommand::SetPosition(track_id, position_us) => {
                // Verify track ID matches current track
                if let Some(np) = &self.now_playing
                    && np.track_id == track_id
                    && np.duration > 0.0
                {
                    let position_secs = position_us as f64 / 1_000_000.0;
                    let percent = (position_secs / np.duration) * 100.0;
                    return Task::done(cosmic::Action::App(Message::SeekTo(percent)));
                }
            }
            MprisCommand::Raise => {
                // Panel-applet mode: toggles the popup window.
                // Standalone mode: no-op (the window is already visible).
                return Task::done(cosmic::Action::App(Message::TogglePopup));
            }
            MprisCommand::Quit => {
                // Request application quit
                tracing::info!("MPRIS quit requested");
                // For now, just stop playback
                return Task::done(cosmic::Action::App(Message::StopPlayback));
            }
            MprisCommand::OpenUri(uri) => {
                return self.handle_mpris_open_uri(uri);
            }
            MprisCommand::SetVolume(volume) => {
                let delta = (volume as f32) - self.volume_level;
                if delta.abs() > f32::EPSILON {
                    return self.handle_adjust_volume(delta);
                }
            }
            MprisCommand::SetShuffle(enabled) => {
                if enabled != self.shuffle_enabled {
                    return Task::done(cosmic::Action::App(Message::ToggleShuffle));
                }
            }
            MprisCommand::SetLoopStatus(status) => {
                if status != self.loop_status {
                    self.loop_status = status;
                    return self.update_mpris_state();
                }
            }
            MprisCommand::GoTo(track_path) => {
                // The object path encodes the queue index as a suffix:
                //   /org/mpris/MediaPlayer2/track/{track_id}_{queue_index}
                // Parse the index out and jump to it.
                if let Some(suffix) = track_path.rsplit('/').next()
                    && let Some(idx_str) = suffix.rsplit('_').next()
                    && let Ok(index) = idx_str.parse::<usize>()
                    && index < self.playback_queue.len()
                {
                    tracing::info!(
                        "MPRIS GoTo: jumping to queue index {} (path: {})",
                        index,
                        track_path
                    );
                    self.playback_queue_index = index;
                    return self.play_track_at_index(index);
                }
                tracing::warn!("MPRIS GoTo: unrecognised track path: {}", track_path);
            }
        }
        Task::none()
    }

    // =========================================================================
    // MPRIS OpenUri
    // =========================================================================

    /// Parse and handle a URI received via MPRIS `OpenUri`.
    ///
    /// Supported formats:
    /// - `tidal://track/{id}`
    /// - `tidal://album/{id}`
    /// - `tidal://playlist/{uuid}`
    /// - `tidal://artist/{id}`
    /// - `tidal://mix/{id}`
    /// - `https://tidal.com/browse/track/{id}`
    /// - `https://listen.tidal.com/track/{id}` (and album/playlist/artist/mix)
    /// - `https://tidal.com/track/{id}` (short form)
    fn handle_mpris_open_uri(&mut self, uri: String) -> Task<cosmic::Action<Message>> {
        tracing::info!("MPRIS OpenUri requested: {}", uri);

        // Normalise: strip the scheme/host prefix down to "{type}/{id}"
        let path = uri
            .strip_prefix("tidal://")
            .or_else(|| uri.strip_prefix("https://tidal.com/browse/"))
            .or_else(|| uri.strip_prefix("https://listen.tidal.com/"))
            .or_else(|| uri.strip_prefix("https://tidal.com/"))
            .unwrap_or("");

        // Split into (resource_type, id).  Handle optional trailing slashes
        // or query strings: "track/12345?foo=bar" → ("track", "12345")
        let (resource_type, raw_id) = match path.split_once('/') {
            Some((t, rest)) => (t, rest.split(['?', '#', '/']).next().unwrap_or("")),
            None => {
                tracing::warn!("MPRIS OpenUri: cannot parse path from: {}", uri);
                return Task::none();
            }
        };

        let id = raw_id.to_string();
        if id.is_empty() {
            tracing::warn!("MPRIS OpenUri: empty ID in: {}", uri);
            return Task::none();
        }

        match resource_type {
            "track" => {
                // Fetch track metadata, then play it as a single-track queue
                let client = self.tidal_client.clone();
                return Task::perform(
                    async move {
                        let client = client.lock().await;
                        client.get_track_by_id(&id).await.map_err(|e| e.to_string())
                    },
                    |result| match result {
                        Ok(track) => cosmic::Action::App(Message::PlayTrackList(
                            vec![track],
                            0,
                            Some("MPRIS OpenUri".to_string()),
                        )),
                        Err(e) => {
                            tracing::error!("MPRIS OpenUri: failed to fetch track: {}", e);
                            cosmic::Action::App(Message::ClearError)
                        }
                    },
                );
            }
            "album" => {
                return Task::done(cosmic::Action::App(Message::ShowAlbumDetailById(id)));
            }
            "playlist" => {
                return Task::done(cosmic::Action::App(Message::ShowPlaylistDetail(
                    id,
                    "MPRIS OpenUri".to_string(),
                )));
            }
            "artist" => {
                return Task::done(cosmic::Action::App(Message::ShowArtistDetail(id)));
            }
            "mix" => {
                return Task::done(cosmic::Action::App(Message::ShowMixDetail(
                    id,
                    "MPRIS OpenUri".to_string(),
                )));
            }
            _ => {
                tracing::warn!(
                    "MPRIS OpenUri: unsupported resource type '{}' in: {}",
                    resource_type,
                    uri
                );
            }
        }

        Task::none()
    }

    // =========================================================================
    // Task Helper Methods
    // =========================================================================

    /// Load images for a list of URLs (call when navigating to views with images)
    pub(crate) fn load_images_for_urls(&self, urls: Vec<String>) -> Task<cosmic::Action<Message>> {
        let tasks: Vec<_> = urls
            .into_iter()
            .filter(|url| {
                !self.loaded_images.contains_key(url) && !self.pending_image_loads.contains(url)
            })
            .map(|url| Task::done(cosmic::Action::App(Message::LoadImage(url))))
            .collect();

        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    /// Update MPRIS D-Bus state with current playback info
    pub(crate) fn update_mpris_state(&self) -> Task<cosmic::Action<Message>> {
        if let Some(handle) = &self.mpris_handle {
            let handle = handle.clone();

            // Build current state
            let playback_status = match self.playback_state {
                PlaybackState::Playing => MprisPlaybackStatus::Playing,
                PlaybackState::Paused => MprisPlaybackStatus::Paused,
                PlaybackState::Stopped | PlaybackState::Loading => MprisPlaybackStatus::Stopped,
            };

            let metadata = if let Some(np) = &self.now_playing {
                MprisMetadata {
                    track_id: np.track_id.clone(),
                    title: np.title.clone(),
                    artists: vec![np.artist.clone()],
                    album: np.album.clone(),
                    album_artists: vec![np.artist.clone()],
                    length_us: (np.duration * 1_000_000.0) as i64,
                    art_url: np.cover_url.clone(),
                    track_number: None,
                    disc_number: None,
                }
            } else {
                MprisMetadata::default()
            };

            let position_us = (self.playback_position * 1_000_000.0) as i64;
            let can_go_next = !self.playback_queue.is_empty()
                && (self.playback_queue_index + 1 < self.playback_queue.len()
                    || self.loop_status == LoopStatus::Playlist
                    || self.loop_status == LoopStatus::Track);
            let can_go_previous = !self.playback_queue.is_empty();
            let can_play = self.now_playing.is_some() || !self.playback_queue.is_empty();
            let can_pause = self.playback_state == PlaybackState::Playing;
            let can_seek = self.now_playing.is_some();

            // Build the MPRIS tracklist from the playback queue.
            // Each entry gets a unique object path (track ID + queue index)
            // so duplicate tracks in the queue are distinguishable.
            let tracklist: Vec<MprisTrackEntry> = self
                .playback_queue
                .iter()
                .enumerate()
                .map(|(i, track)| MprisTrackEntry {
                    object_path: track_object_path(&track.id, i),
                    metadata: MprisMetadata::from_track(track),
                })
                .collect();

            let current_track_path = if !self.playback_queue.is_empty() {
                let idx = self.playback_queue_index;
                if let Some(track) = self.playback_queue.get(idx) {
                    track_object_path(&track.id, idx)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            let state = MprisState {
                playback_status,
                metadata,
                position_us,
                volume: self.volume_level as f64,
                shuffle: self.shuffle_enabled,
                loop_status: self.loop_status,
                can_go_next,
                can_go_previous,
                can_play,
                can_pause,
                can_seek,
                tracklist,
                current_track_path,
            };

            return Task::perform(
                async move {
                    handle.update_state(state).await;
                },
                |_| cosmic::Action::App(Message::ClearError), // No-op
            );
        }
        Task::none()
    }

    // =========================================================================
    // Screenshot
    // =========================================================================

    /// Initiate a screenshot capture of the applet window.
    ///
    /// In panel-applet mode this captures the popup window (if open).
    /// In standalone mode it captures the main application window.
    /// The framebuffer already includes the compositor's rounded-corner
    /// alpha, so the saved PNG has transparent corners out of the box.
    pub fn handle_take_screenshot(&mut self) -> Task<cosmic::Action<Message>> {
        // Determine which window ID to capture.
        #[cfg(feature = "panel-applet")]
        let window_id = self.popup;

        #[cfg(not(feature = "panel-applet"))]
        let window_id = self.core.main_window_id();

        let Some(id) = window_id else {
            tracing::warn!("Screenshot requested but no target window is open");
            return Task::none();
        };

        tracing::info!("Capturing screenshot of window {:?}", id);

        cosmic::iced::runtime::window::screenshot(id)
            .map(|s| cosmic::Action::App(Message::ScreenshotCaptured(s)))
    }

    /// Encode a captured screenshot as PNG (preserving alpha) and save it
    /// to `~/Pictures/` with a timestamped filename.
    pub fn handle_screenshot_captured(
        &mut self,
        screenshot: cosmic::iced::core::window::Screenshot,
    ) {
        // Spawn the (potentially slow) PNG encoding on a background thread
        // so we never block the UI.
        if let Err(e) = std::thread::Builder::new()
            .name("screenshot-save".into())
            .spawn(move || {
                Self::save_screenshot_png(&screenshot);
            })
        {
            tracing::error!("Failed to spawn screenshot-save thread: {e}");
        }
    }

    /// Encode `screenshot` as a PNG file and write it to disk.
    ///
    /// The RGBA bytes from iced's GPU read-back already contain the
    /// compositor's rounded-corner alpha, so the saved PNG faithfully
    /// preserves transparent corners with no extra processing needed.
    fn save_screenshot_png(screenshot: &cosmic::iced::core::window::Screenshot) {
        use image::{ImageBuffer, Rgba};

        let width = screenshot.size.width;
        let height = screenshot.size.height;

        let Some(buf) =
            ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, screenshot.rgba.to_vec())
        else {
            tracing::error!(
                "Screenshot RGBA buffer size mismatch: expected {}×{}×4 = {}, got {}",
                width,
                height,
                (width as usize) * (height as usize) * 4,
                screenshot.rgba.len(),
            );
            return;
        };

        // Build output path: ~/Pictures/mare-player-<timestamp>.png
        let pictures_dir = dirs::picture_dir().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join("Pictures")
        });

        if let Err(e) = std::fs::create_dir_all(&pictures_dir) {
            tracing::error!(
                "Cannot create Pictures directory {}: {e}",
                pictures_dir.display()
            );
            return;
        }

        let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let filename = format!("mare-player-{timestamp}.png");
        let path = pictures_dir.join(&filename);

        match buf.save(&path) {
            Ok(()) => {
                tracing::info!(
                    "Screenshot saved: {} ({}×{}, {:.1} KB)",
                    path.display(),
                    width,
                    height,
                    screenshot.rgba.len() as f64 / 1024.0,
                );
            }
            Err(e) => {
                tracing::error!("Failed to save screenshot to {}: {e}", path.display());
            }
        }
    }
}
