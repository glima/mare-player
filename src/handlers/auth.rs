// SPDX-License-Identifier: MIT

//! Authentication message handlers for Maré Player.
//!
//! This module handles login, OAuth flow, logout, and session restoration.

use cosmic::prelude::*;

use crate::messages::Message;
use crate::state::{AppModel, ViewState};
use crate::tidal::auth::DeviceCodeInfo;

// =============================================================================
// Task Helper Methods
// =============================================================================

impl AppModel {
    /// Attempt to restore a previous session from stored credentials
    pub(crate) fn restore_session(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let audio_quality = self.config.audio_quality.to_tidlers();
        Task::perform(
            async move {
                let mut client = client.lock().await;
                // Apply configured audio quality before restoring session
                client.set_audio_quality(audio_quality).await;
                client
                    .try_restore_session()
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::SessionRestored(result)),
        )
    }

    /// Start the OAuth device code flow
    pub(crate) fn start_oauth_flow(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let audio_quality = self.config.audio_quality.to_tidlers();
        Task::perform(
            async move {
                let mut client = client.lock().await;
                // Apply configured audio quality before starting OAuth
                client.set_audio_quality(audio_quality).await;
                client.start_oauth_flow().await.map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::LoginOAuthReceived(result)),
        )
    }

    /// Poll for OAuth completion after user authorizes
    pub(crate) fn wait_for_oauth(
        &self,
        device_code: String,
        expires_in: u64,
        interval: u64,
    ) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let mut client = client.lock().await;
                client
                    .wait_for_oauth(&device_code, expires_in, interval)
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::OAuthComplete(result)),
        )
    }
}

// =============================================================================
// Message Handlers
// =============================================================================

impl AppModel {
    /// Handle start login - begins the OAuth flow
    pub fn handle_start_login(&mut self) -> Task<cosmic::Action<Message>> {
        self.is_loading = true;
        self.start_oauth_flow()
    }

    /// Handle OAuth device code info received
    pub fn handle_login_oauth_received(
        &mut self,
        result: Result<DeviceCodeInfo, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(device_info) => {
                // Store device info and switch to awaiting view
                let device_code = device_info.device_code.clone();
                let expires_in = device_info.expires_in;
                let interval = device_info.interval;
                self.device_code_info = Some(device_info);
                self.view_state = ViewState::AwaitingOAuth;

                // Auto-start polling immediately in the background
                // This way the user doesn't need to click "I've Signed In"
                tracing::info!("Auto-starting OAuth polling in background");
                self.wait_for_oauth(device_code, expires_in, interval)
            }
            Err(e) => {
                tracing::error!("Login failed: {}", e);
                self.error_message = Some(format!("Login failed: {}", e));
                self.view_state = ViewState::Login;
                Task::none()
            }
        }
    }

    /// Handle open OAuth URL in browser
    pub fn handle_open_oauth_url(&self) {
        if let Some(info) = &self.device_code_info {
            let _ = open::that(&info.verification_uri_complete);
        }
    }

    /// Handle OAuth flow completed
    pub fn handle_oauth_complete(
        &mut self,
        result: Result<(), String>,
    ) -> Task<cosmic::Action<Message>> {
        tracing::info!("OAuthComplete received with result: {:?}", result.is_ok());
        self.is_loading = false;
        self.device_code_info = None;
        match result {
            Ok(()) => {
                tracing::info!("OAuth successful! Transitioning to Main view");
                self.enter_main_view()
            }
            Err(e) => {
                tracing::error!("OAuth failed: {}", e);
                self.error_message = Some(format!("Authentication failed: {}", e));
                self.view_state = ViewState::Login;
                Task::none()
            }
        }
    }

    /// Transition to the main view after successful authentication.
    ///
    /// Restores cached API data for instant UI population, then kicks off
    /// background refreshes from the TIDAL API so content stays current.
    /// Used by both [`handle_oauth_complete`] and [`handle_session_restored`].
    fn enter_main_view(&mut self) -> Task<cosmic::Action<Message>> {
        self.view_state = ViewState::Main;

        let cache_task = self.restore_cached_api_data();

        Task::batch(vec![
            cache_task,
            self.load_playlists(),
            self.load_albums(),
            self.load_favorite_tracks(),
            self.load_profiles(),
            self.load_mixes(),
        ])
    }

    /// Populate the UI with cached API data (playlists, albums, favorite tracks)
    /// so the user sees content instantly on startup. Returns a Task that loads
    /// cover images for any cached data that was restored.
    fn restore_cached_api_data(&mut self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.blocking_lock();
        let mut image_urls: Vec<String> = Vec::new();

        // Restore cached playlists
        if let Some(playlists) = client.get_cached_playlists() {
            tracing::info!("Restored {} playlists from API cache", playlists.len());
            for p in &playlists {
                if let Some(url) = &p.image_url {
                    image_urls.push(url.clone());
                }
            }
            self.user_playlists = playlists;
        }

        // Restore cached albums
        if let Some(albums) = client.get_cached_albums() {
            tracing::info!("Restored {} albums from API cache", albums.len());
            for a in &albums {
                if let Some(url) = &a.cover_url {
                    image_urls.push(url.clone());
                }
            }
            self.favorite_album_ids = albums.iter().map(|a| a.id.clone()).collect();
            self.user_albums = albums;
        }

        // Restore cached favorite tracks
        if let Some(tracks) = client.get_cached_favorite_tracks() {
            tracing::info!("Restored {} favorite tracks from API cache", tracks.len());
            for t in &tracks {
                if let Some(url) = &t.cover_url {
                    image_urls.push(url.clone());
                }
            }
            self.favorite_track_ids = tracks.iter().map(|t| t.id.clone()).collect();
            self.user_favorite_tracks = tracks;
        }

        // Restore cached mixes
        if let Some(mixes) = client.get_cached_mixes() {
            tracing::info!("Restored {} mixes from API cache", mixes.len());
            for m in &mixes {
                if let Some(url) = &m.image_url {
                    image_urls.push(url.clone());
                }
            }
            self.user_mixes = mixes;
        }

        // Restore cached followed artists
        if let Some(artists) = client.get_cached_followed_artists() {
            tracing::info!("Restored {} followed artists from API cache", artists.len());
            for a in &artists {
                if let Some(url) = &a.picture_url {
                    image_urls.push(url.clone());
                }
            }
            self.followed_artist_ids = artists.iter().map(|a| a.id.clone()).collect();
            self.user_followed_artists = artists;
        }

        drop(client);

        // Load cover images for the cached data
        let img_task = self.load_images_for_urls(image_urls);
        // Generate 2×2 grid thumbnails for any playlists that were restored
        let thumb_task = if !self.user_playlists.is_empty() {
            Task::done(cosmic::Action::App(Message::GeneratePlaylistThumbnails))
        } else {
            Task::none()
        };
        Task::batch([img_task, thumb_task])
    }

    /// Handle session restored result
    pub fn handle_session_restored(
        &mut self,
        result: Result<bool, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(true) => {
                self.error_message = None;
                self.enter_main_view()
            }
            Ok(false) => {
                self.view_state = ViewState::Login;
                Task::none()
            }
            Err(ref e) if e.contains("Network error") && self.error_message.is_none() => {
                // First network failure — likely resuming from suspend / lid-open.
                // try_restore_session already retried internally with backoff;
                // schedule one more attempt so we cover slower reconnects.
                tracing::info!("Session restore hit a network error, scheduling retry in 5s");
                self.error_message = Some("Network unavailable, retrying\u{2026}".into());
                self.is_loading = true;
                let client = self.tidal_client.clone();
                let aq = self.config.audio_quality.to_tidlers();
                Task::perform(
                    async move {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        let mut c = client.lock().await;
                        c.set_audio_quality(aq).await;
                        c.try_restore_session().await.map_err(|e| e.to_string())
                    },
                    |r| cosmic::Action::App(Message::SessionRestored(r)),
                )
            }
            Err(e) => {
                self.view_state = ViewState::Login;
                self.error_message = Some(e);
                Task::none()
            }
        }
    }

    /// Handle logout
    pub fn handle_logout(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let mut client = client.lock().await;
                client.logout().await;
            },
            |_| cosmic::Action::App(Message::ShowMain),
        )
        .chain(Task::done(cosmic::Action::App(Message::StartLogin)))
    }
}
