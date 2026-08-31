// SPDX-License-Identifier: MIT

//! Authentication message handlers for Maré Player.
//!
//! This module handles login, the OAuth PKCE flow, logout, and session
//! restoration.

use cosmic::prelude::*;

use crate::fl;
use crate::messages::Message;
use crate::state::{AppModel, ViewState};
use crate::tidal::auth::LoginRequest;

// =============================================================================
// Task Helper Methods
// =============================================================================

impl AppModel {
    /// Attempt to restore a previous session from stored credentials
    pub(crate) fn restore_session(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let audio_quality = self.config.audio_quality;
        Task::perform(
            async move {
                let mut client = client.lock().await;
                // Apply configured audio quality before restoring session
                client.set_audio_quality(audio_quality).await;
                client.try_restore_session().await.map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::SessionRestored(result)),
        )
    }

    /// Start the OAuth PKCE flow — builds the TIDAL authorize URL.
    pub(crate) fn start_login_flow(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let audio_quality = self.config.audio_quality;
        Task::perform(
            async move {
                let mut client = client.lock().await;
                // Apply configured audio quality before starting the login
                client.set_audio_quality(audio_quality).await;
                client.start_login().await.map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::LoginUrlReceived(result)),
        )
    }

    /// Exchange the redirect URL the user pasted for TIDAL tokens.
    pub(crate) fn complete_login(&self, redirect_url: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let mut client = client.lock().await;
                client.complete_login(&redirect_url).await.map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::LoginComplete(result)),
        )
    }
}

// =============================================================================
// Message Handlers
// =============================================================================

impl AppModel {
    /// Handle start login - begins the PKCE flow
    pub fn handle_start_login(&mut self) -> Task<cosmic::Action<Message>> {
        self.is_loading = true;
        self.login_redirect_url.clear();
        self.start_login_flow()
    }

    /// Handle the PKCE authorize URL being ready.
    ///
    /// Shows the view that walks the user through the browser sign-in and takes
    /// the redirect URL back. The browser is opened on demand rather than
    /// automatically: in applet mode the popup closes the moment the browser
    /// takes focus, so the user needs to read the instructions first.
    pub fn handle_login_url_received(&mut self, result: Result<LoginRequest, String>) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(request) => {
                self.login_request = Some(request);
                self.view_state = ViewState::AwaitingOAuth;
                Task::none()
            }
            Err(e) => {
                tracing::error!("Login failed: {}", e);
                self.error_message = Some(format!("Login failed: {}", e));
                self.view_state = ViewState::Login;
                Task::none()
            }
        }
    }

    /// Handle opening the TIDAL authorize URL in the browser
    pub fn handle_open_login_url(&self) {
        if let Some(request) = &self.login_request {
            let _ = open::that(&request.authorize_url);
        }
    }

    /// Handle the user submitting the pasted redirect URL
    pub fn handle_submit_login_redirect_url(&mut self) -> Task<cosmic::Action<Message>> {
        let redirect_url = self.login_redirect_url.trim().to_string();
        if redirect_url.is_empty() {
            return Task::none();
        }
        self.is_loading = true;
        self.complete_login(redirect_url)
    }

    /// Handle the sign-in callback service coming up.
    ///
    /// A failure here only costs the automatic return: the login view falls
    /// back to asking for the URL, so it is logged rather than surfaced.
    pub fn handle_login_uri_service_started(
        &mut self,
        result: Result<std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<String>>>, String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(rx) => self.login_uri_rx = Some(rx),
            Err(e) => tracing::warn!("Sign-in callbacks unavailable: {e}"),
        }
        Task::none()
    }

    /// Handle a `tidal://login/auth?code=…` URI handed over by the browser.
    ///
    /// This is the whole point of registering the scheme: the user signs in and
    /// the code arrives on its own, with nothing to copy.
    pub fn handle_login_callback_uri(&mut self, uri: String) -> Task<cosmic::Action<Message>> {
        if self.login_request.is_none() {
            // Nothing asked for this — a stale link clicked out of a browser's
            // history, most likely. Its code is long dead either way.
            tracing::warn!("Ignoring a sign-in callback with no login in progress");
            return Task::none();
        }
        tracing::info!("Sign-in callback received from the browser");
        self.error_message = None;
        self.login_redirect_url = uri;
        self.handle_submit_login_redirect_url()
    }

    /// Handle the login flow completing
    pub fn handle_login_complete(&mut self, result: Result<(), String>) -> Task<cosmic::Action<Message>> {
        tracing::info!("LoginComplete received with result: {:?}", result.is_ok());
        self.is_loading = false;
        match result {
            Ok(()) => {
                tracing::info!("Login successful! Transitioning to Main view");
                self.login_request = None;
                self.login_redirect_url.clear();
                self.enter_main_view()
            }
            Err(e) => {
                // Stay on the login view. Authorization codes are single-use and
                // expire in minutes, so the common failure is a URL that was
                // already spent — the sign-in itself is still live, and going
                // through the browser again yields a fresh code.
                tracing::error!("Login failed: {}", e);
                self.login_redirect_url.clear();
                self.error_message = Some(fl!("login-retry"));
                Task::none()
            }
        }
    }

    /// Transition to the main view after successful authentication.
    ///
    /// Restores cached API data for instant UI population, then kicks off
    /// background refreshes from the TIDAL API so content stays current.
    /// Used by both [`Self::handle_login_complete`] and
    /// [`Self::handle_session_restored`].
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

    /// Populate the UI with the last-seen library from the cache database
    /// (playlists, albums, favorite tracks, mixes, profiles) so the user sees
    /// content instantly on startup. Reads run through the same view-cache path
    /// the navigation handlers use; the parallel network loads in
    /// [`Self::enter_main_view`] then refresh everything.
    fn restore_cached_api_data(&self) -> Task<cosmic::Action<Message>> {
        use crate::tidal::models::{Album, Artist, Mix, Playlist, Track};
        Task::batch([
            self.read_view_cache::<Vec<Playlist>, _>("library:playlists", |p| Message::PlaylistsLoaded(Ok(p))),
            self.read_view_cache::<Vec<Album>, _>("library:albums", |a| Message::AlbumsLoaded(Ok(a))),
            self.read_view_cache::<Vec<Track>, _>("favorites:tracks", |t| Message::FavoriteTracksLoaded(Ok(t))),
            self.read_view_cache::<Vec<Mix>, _>("library:mixes", |m| Message::MixesLoaded(Ok(m))),
            self.read_view_cache::<Vec<Artist>, _>("profiles", |a| Message::ProfilesLoaded(Ok(a))),
        ])
    }

    /// Handle session restored result
    pub fn handle_session_restored(&mut self, result: Result<bool, String>) -> Task<cosmic::Action<Message>> {
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
                let aq = self.config.audio_quality;
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
