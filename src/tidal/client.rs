// SPDX-License-Identifier: MIT

//! TIDAL client wrapper for the COSMIC applet.
//!
//! This module wraps the `tidlers` crate and provides a high-level async API
//! for interacting with TIDAL's services including:
//! - OAuth authentication
//! - Playlist and album browsing
//! - Artist and album detail pages
//! - Track search
//! - User favorites (tracks and albums)
//! - HiRes/DASH streaming support

use super::auth::{AuthManager, AuthState, DeviceCodeInfo, StoredCredentials, UserProfile};
use super::models::{
    Album, Artist, ExploreCard, ExplorePage, ExploreSection, ExploreTarget, FeedActivity, FeedItem,
    Mix, PageLink, Playlist, SearchResults, Track, TrackLyrics, tidal_cover_url,
    tidal_promo_image_url,
};
use base64::{Engine, engine::general_purpose};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use std::sync::Arc;
use tidlers::{
    TidalClient,
    auth::TidalAuth,
    client::models::{collection::favorites::FavoriteResourceType, playback::AudioQuality},
};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// Safety margin before token expiry to trigger refresh (5 minutes)
const TOKEN_REFRESH_MARGIN_SECS: u64 = 300;

/// Format a duration in seconds for human-readable log output.
/// Shows hours (e.g. "4.0h") when ≥ 60 min, otherwise minutes (e.g. "5min").
fn format_duration(secs: u64) -> String {
    let mins = secs / 60;
    if mins >= 60 {
        format!("{:.1}h", secs as f64 / 3600.0)
    } else {
        format!("{}min", mins)
    }
}

/// Check if a tidlers error is a transient network error (DNS, connect, timeout)
/// that should be retried rather than treated as an auth failure.
fn is_tidlers_network_error(e: &tidlers::error::TidalError) -> bool {
    let dbg = format!("{:?}", e);
    dbg.contains("dns error")
        || dbg.contains("name resolution")
        || dbg.contains("ConnectError")
        || dbg.contains("Timeout")
        || dbg.contains("connection reset")
        || dbg.contains("connection refused")
        || dbg.contains("NetworkUnreachable")
        || dbg.contains("No route to host")
}

/// Result of getting a playback URL - either a direct streaming URL or an
/// inline DASH manifest for FLAC/hi-res.
#[derive(Debug, Clone)]
pub enum PlaybackUrl {
    /// Direct streaming URL (for Low/High/Lossless quality)
    Direct(String, Option<f32>),
    /// Inline DASH manifest XML (for HiRes quality). Played through a `data:`
    /// URI so nothing is written to disk; its embedded segment URLs are
    /// absolute and carry short-lived tokens.
    DashManifest(String, Option<f32>),
}

impl PlaybackUrl {
    /// Get a ready-to-use GStreamer URI for playback.
    ///
    /// `Direct` is already an `http(s)` URL. `DashManifest` is base64-wrapped
    /// into a `data:application/dash+xml` URI so GStreamer's `dataurisrc` +
    /// `dashdemux` consume the manifest inline — no file on disk. TIDAL's
    /// segment URLs are absolute, so no base URI is required.
    ///
    /// This relies on TIDAL manifests being `type="static"` with a complete
    /// segment timeline: adaptivedemux never needs to *refresh* the manifest
    /// (its refresh downloader can't re-fetch a `data:` URI). Live/dynamic
    /// manifests would not work inline — but TIDAL doesn't serve those here.
    pub fn as_url(&self) -> String {
        match self {
            PlaybackUrl::Direct(url, _) => url.clone(),
            PlaybackUrl::DashManifest(manifest, _) => {
                let b64 = general_purpose::STANDARD.encode(manifest.as_bytes());
                format!("data:application/dash+xml;base64,{b64}")
            }
        }
    }

    /// Check if this is a DASH manifest (requires special handling)
    pub fn is_dash(&self) -> bool {
        matches!(self, PlaybackUrl::DashManifest(..))
    }

    /// Get the replay gain value in dB, if available from the TIDAL API.
    pub fn replay_gain_db(&self) -> Option<f32> {
        match self {
            PlaybackUrl::Direct(_, rg) | PlaybackUrl::DashManifest(_, rg) => *rg,
        }
    }
}

impl std::fmt::Display for PlaybackUrl {
    /// Concise, token-free rendering for logs. Direct URLs have their query
    /// string — which carries the short-lived auth token — stripped; DASH
    /// shows only the inline manifest size (the manifest embeds segment
    /// tokens). Never print the raw URL / manifest in logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaybackUrl::Direct(url, rg) => {
                let base = url.split('?').next().unwrap_or(url);
                write!(f, "Direct({base}")?;
                if let Some(rg) = rg {
                    write!(f, ", {rg:+.2}dB")?;
                }
                write!(f, ")")
            }
            PlaybackUrl::DashManifest(manifest, rg) => {
                write!(f, "DashManifest(<inline manifest, {} bytes>", manifest.len())?;
                if let Some(rg) = rg {
                    write!(f, ", {rg:+.2}dB")?;
                }
                write!(f, ")")
            }
        }
    }
}

// ── Unified API deserialization structs ─────────────────────────────────
//
// TIDAL uses the same track/album/artist shapes across many endpoints
// (favorites, playlist items, mix items, track radio, etc.) with minor
// differences in nullability.  These "Api*" structs use `Option` and
// `#[serde(default)]` everywhere so a single family handles all variants.

/// Generic paginated TIDAL response (works for tracks, albums, etc.)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiPaginatedResponse<T> {
    items: Vec<T>,
    #[serde(default)]
    total_number_of_items: i32,
}

/// Wrapper for endpoints that nest the real payload under `"item"`.
/// `item` is `Option` because mix endpoints can contain null entries.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiItemWrapper<T> {
    item: Option<T>,
    /// Playlist items carry the kind here (`"track"` or `"video"`); absent on
    /// other endpoints.
    #[serde(default, rename = "type")]
    item_type: Option<String>,
}

/// Lenient track data — works for playlist, favorite, mix, and radio responses.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiTrackData {
    id: u64,
    title: String,
    duration: u64,
    #[serde(default)]
    track_number: u32,
    #[serde(default)]
    explicit: bool,
    audio_quality: Option<String>,
    /// Null for some video items in playlists (and occasionally curated lists),
    /// so this must stay optional or the whole response fails to deserialize.
    /// Falls back to the first entry of `artists` when null.
    #[serde(default)]
    artist: Option<ApiTrackArtist>,
    /// Full artist list; used as a fallback when the singular `artist` is null.
    #[serde(default)]
    artists: Vec<ApiTrackArtist>,
    /// Null for video items in playlists (and occasionally curated lists), so
    /// this must stay optional or the whole response fails to deserialize.
    #[serde(default)]
    album: Option<ApiTrackAlbum>,
    /// Video items have no album cover; their thumbnail lives here (camelCase
    /// `imageId`). Used as the cover when `album` is absent.
    #[serde(default)]
    image_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiTrackArtist {
    id: u64,
    /// TIDAL sometimes returns null for artist name in curated playlists
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiTrackAlbum {
    id: u64,
    #[serde(default)]
    title: String,
    cover: Option<String>,
}

/// Convert an `ApiTrackData` into our domain `Track`.
impl From<ApiTrackData> for Track {
    fn from(t: ApiTrackData) -> Self {
        // Video items (and some curated entries) have no album; fall back to
        // the item's own `imageId` thumbnail for the cover.
        let (album_name, album_id, cover_url) = match t.album {
            Some(a) => (
                Some(a.title),
                Some(a.id.to_string()),
                a.cover.map(|c| tidal_cover_url(&c)),
            ),
            None => (None, None, t.image_id.map(|id| tidal_cover_url(&id))),
        };
        // Primary artist: the singular `artist`, falling back to the first of
        // the `artists` list when it's null (e.g. video items in playlists).
        let (artist_name, artist_id) = match t.artist.or_else(|| t.artists.into_iter().next()) {
            Some(a) => (
                a.name.unwrap_or_else(|| "Unknown Artist".to_string()),
                Some(a.id.to_string()),
            ),
            None => ("Unknown Artist".to_string(), None),
        };
        Track {
            id: t.id.to_string(),
            title: t.title,
            duration: t.duration as u32,
            track_number: t.track_number,
            artist_name,
            artist_id,
            album_name,
            album_id,
            cover_url,
            explicit: t.explicit,
            audio_quality: t.audio_quality,
            is_video: false,
        }
    }
}

/// Lenient album data — used for favorite albums responses.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiAlbumData {
    id: u64,
    title: String,
    duration: u64,
    number_of_tracks: u32,
    release_date: Option<String>,
    cover: String,
    explicit: bool,
    audio_quality: Option<String>,
    artist: ApiAlbumArtist,
}

#[derive(Debug, Deserialize)]
struct ApiAlbumArtist {
    id: u64,
    name: String,
}

/// Convert an `ApiAlbumData` into our domain `Album`.
impl From<ApiAlbumData> for Album {
    fn from(a: ApiAlbumData) -> Self {
        Album {
            id: a.id.to_string(),
            title: a.title,
            artist_name: a.artist.name,
            artist_id: Some(a.artist.id.to_string()),
            num_tracks: a.number_of_tracks,
            duration: a.duration as u32,
            release_date: a.release_date,
            cover_url: Some(tidal_cover_url(&a.cover)),
            explicit: a.explicit,
            audio_quality: a.audio_quality,
            review: None,
        }
    }
}

// ── Credential helpers returned by auth_context* ────────────────────────

/// Access token + country code (no user ID needed).
struct AuthTokenContext {
    access_token: String,
    country_code: String,
}

/// Access token + country code + user ID.
struct AuthUserContext {
    user_id: u64,
    access_token: String,
    country_code: String,
}

pub type TidalResult<T> = Result<T, TidalError>;

/// Errors that can occur during TIDAL operations
#[derive(Debug, Clone)]
pub enum TidalError {
    /// Not authenticated with TIDAL
    NotAuthenticated,
    /// Authentication failed
    AuthenticationFailed(String),
    /// API request failed
    RequestFailed(String),
    /// Failed to parse response
    ParseError(String),
    /// Session expired
    SessionExpired,
    /// Network error
    NetworkError(String),
    /// Credential storage error
    CredentialError(String),
}

impl std::fmt::Display for TidalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TidalError::NotAuthenticated => write!(f, "Not authenticated with TIDAL"),
            TidalError::AuthenticationFailed(msg) => write!(f, "Authentication failed: {}", msg),
            TidalError::RequestFailed(msg) => write!(f, "Request failed: {}", msg),
            TidalError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            TidalError::SessionExpired => write!(f, "Session expired"),
            TidalError::NetworkError(msg) => write!(f, "Network error: {}", msg),
            TidalError::CredentialError(msg) => write!(f, "Credential error: {}", msg),
        }
    }
}

impl std::error::Error for TidalError {}

/// High-level TIDAL client for the COSMIC applet
pub struct TidalAppClient {
    /// The underlying tidlers client (if authenticated)
    /// Wrapped in `Arc<Mutex>` to allow token refresh during API calls
    client: Arc<Mutex<Option<TidalClient>>>,
    /// Authentication manager
    auth_manager: AuthManager,
    /// Current audio quality setting
    audio_quality: AudioQuality,
}

impl Default for TidalAppClient {
    fn default() -> Self {
        Self::new()
    }
}

impl TidalAppClient {
    // ── Credential extraction helpers ───────────────────────────────────
    //
    // Many methods need access_token + country_code (and sometimes user_id)
    // extracted from the locked client.  These helpers eliminate the ~15-line
    // boilerplate that was previously copy-pasted into every method.

    /// Extract access token + country code from the authenticated client.
    ///
    /// The lock is acquired and released inside, so callers get owned values
    /// they can use across `.await` points without holding the mutex.
    async fn auth_context(&self) -> TidalResult<AuthTokenContext> {
        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        let access_token = client
            .session
            .auth
            .access_token
            .as_ref()
            .ok_or_else(|| {
                error!("No access token available");
                TidalError::NotAuthenticated
            })?
            .clone();

        let country_code = client
            .user_info
            .as_ref()
            .map(|u| u.country_code.clone())
            .unwrap_or_else(|| "US".to_string());

        Ok(AuthTokenContext {
            access_token,
            country_code,
        })
    }

    /// Extract access token + country code + user ID from the authenticated client.
    async fn auth_context_with_user(&self) -> TidalResult<AuthUserContext> {
        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        let user_id = client.session.auth.user_id.ok_or_else(|| {
            error!("No user ID available");
            TidalError::NotAuthenticated
        })?;

        let access_token = client
            .session
            .auth
            .access_token
            .as_ref()
            .ok_or_else(|| {
                error!("No access token available");
                TidalError::NotAuthenticated
            })?
            .clone();

        let country_code = client
            .user_info
            .as_ref()
            .map(|u| u.country_code.clone())
            .unwrap_or_else(|| "US".to_string());

        Ok(AuthUserContext {
            user_id,
            access_token,
            country_code,
        })
    }

    /// Add `resource_id` to the user's favorites via tidlers.
    async fn add_to_favorites(
        &self,
        resource: FavoriteResourceType,
        resource_id: &str,
    ) -> TidalResult<()> {
        self.ensure_valid_token().await?;
        let id: u32 = resource_id
            .parse()
            .map_err(|e| TidalError::ParseError(format!("bad id `{resource_id}`: {e}")))?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
        client
            .add_to_favorites(resource, id)
            .await
            .map_err(|e| TidalError::RequestFailed(format!("{e:?}")))
    }

    /// Remove `resource_id` from the user's favorites via tidlers.
    async fn remove_from_favorites(
        &self,
        resource: FavoriteResourceType,
        resource_id: &str,
    ) -> TidalResult<()> {
        self.ensure_valid_token().await?;
        let id: u32 = resource_id
            .parse()
            .map_err(|e| TidalError::ParseError(format!("bad id `{resource_id}`: {e}")))?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
        client
            .remove_from_favorites(resource, id)
            .await
            .map_err(|e| TidalError::RequestFailed(format!("{e:?}")))
    }

    /// Create a new TidalAppClient
    pub fn new() -> Self {
        Self {
            client: Arc::new(Mutex::new(None)),
            auth_manager: AuthManager::new(),
            audio_quality: AudioQuality::High,
        }
    }

    /// Get the current authentication state
    pub fn auth_state(&self) -> &AuthState {
        self.auth_manager.state()
    }

    /// Snapshot the current TIDAL access token, if any.
    ///
    /// Used by `play_reporter` to stamp playback events.  Calls
    /// `try_lock` on the inner client so callers can grab the token
    /// from a sync (iced update) context without risking a deadlock
    /// against an in-flight async API request — if the lock is
    /// contended, returns `None` and the caller skips reporting.
    pub fn current_access_token(&self) -> Option<String> {
        let guard = self.client.try_lock().ok()?;
        guard.as_ref()?.session.auth.access_token.clone()
    }

    /// Set the audio quality for playback
    pub async fn set_audio_quality(&mut self, quality: AudioQuality) {
        info!("Setting audio quality to: {:?}", quality);
        self.audio_quality = quality.clone();
        let mut client_guard = self.client.lock().await;
        if let Some(client) = client_guard.as_mut() {
            client.set_audio_quality(quality);
        }
    }

    /// Ensure the access token is valid, refreshing if needed
    ///
    /// This method checks if the token is expired or close to expiring,
    /// and refreshes it proactively to avoid API failures.
    ///
    /// Returns Ok(true) if token was refreshed, Ok(false) if no refresh needed.
    async fn ensure_valid_token(&self) -> TidalResult<bool> {
        let mut client_guard = self.client.lock().await;
        let client = client_guard.as_mut().ok_or(TidalError::NotAuthenticated)?;

        // Check if token is expired or will expire soon
        let needs_refresh = self.check_token_needs_refresh(client);

        if needs_refresh {
            info!("Access token expired or expiring soon, attempting refresh");
            match client.refresh_access_token(false).await {
                Ok(refreshed) => {
                    if refreshed {
                        info!("Successfully refreshed access token");

                        // Log token expiry info for debugging
                        if let (Some(expiry), Some(last_refresh)) = (
                            client.session.auth.refresh_expiry,
                            client.session.auth.last_refresh_time,
                        ) {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            let expires_at = last_refresh + expiry;
                            let remaining = expires_at.saturating_sub(now);
                            info!(
                                "Token refreshed - expires_in: {}s (~{}), remaining: {}s (~{})",
                                expiry,
                                format_duration(expiry),
                                remaining,
                                format_duration(remaining),
                            );
                        }

                        // Store the refreshed session
                        self.save_session_credentials(client);
                    }
                    Ok(refreshed)
                }
                Err(e) => {
                    error!("Failed to refresh access token: {:?}", e);
                    Err(TidalError::SessionExpired)
                }
            }
        } else {
            Ok(false)
        }
    }

    /// Check if the token needs to be refreshed
    fn check_token_needs_refresh(&self, client: &TidalClient) -> bool {
        // Check token expiry based on stored refresh_expiry and last_refresh_time
        if let (Some(expiry), Some(last_refresh)) = (
            client.session.auth.refresh_expiry,
            client.session.auth.last_refresh_time,
        ) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let expires_at = last_refresh + expiry;
            let remaining = expires_at.saturating_sub(now);

            // Check if token is already expired
            if now >= expires_at {
                debug!("Token is expired (expired {}s ago)", now - expires_at);
                return true;
            }

            // Check if we're close to expiry (within safety margin)
            if remaining < TOKEN_REFRESH_MARGIN_SECS {
                debug!(
                    "Token expiring soon ({}s remaining, margin: {}s), triggering refresh",
                    remaining, TOKEN_REFRESH_MARGIN_SECS
                );
                return true;
            }

            let elapsed = now.saturating_sub(last_refresh);
            debug!(
                "Token still valid - expires_in: {}s (~{}), elapsed: {}s (~{}), remaining: {}s (~{})",
                expiry,
                format_duration(expiry),
                elapsed,
                format_duration(elapsed),
                remaining,
                format_duration(remaining),
            );

            false
        } else {
            // No expiry info available, assume we need to refresh
            debug!("No token expiry info available, assuming refresh needed");
            true
        }
    }

    /// Save session credentials after token refresh
    fn save_session_credentials(&self, client: &TidalClient) {
        let username = client.user_info.as_ref().map(|u| u.username.clone());
        let new_credentials = StoredCredentials {
            session_json: client.get_json(),
            stored_at: chrono::Utc::now(),
            user_id: client.user_info.as_ref().map(|u| u.user_id.to_string()),
            username,
        };

        if let Err(e) = AuthManager::store_credentials(&new_credentials) {
            warn!("Failed to store refreshed credentials: {}", e);
        }
    }

    /// Try to restore a session from stored credentials
    pub async fn try_restore_session(&mut self) -> TidalResult<bool> {
        info!("Attempting to restore TIDAL session from stored credentials");

        let credentials = match AuthManager::load_credentials() {
            Ok(Some(creds)) => creds,
            Ok(None) => {
                debug!("No stored credentials found");
                return Ok(false);
            }
            Err(e) => {
                warn!("Failed to load credentials: {}", e);
                return Err(TidalError::CredentialError(e));
            }
        };

        // Try to restore the session from the stored JSON
        match TidalClient::from_json(&credentials.session_json) {
            Ok(mut client) => {
                // Log current token state
                if let (Some(expiry), Some(last_refresh)) = (
                    client.session.auth.refresh_expiry,
                    client.session.auth.last_refresh_time,
                ) {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let expires_at = last_refresh + expiry;
                    let elapsed = now.saturating_sub(last_refresh);
                    let remaining = expires_at.saturating_sub(now);
                    info!(
                        "Stored token state - expires_in: {}s (~{}), elapsed since refresh: {}s (~{}), remaining: {}s (~{})",
                        expiry,
                        format_duration(expiry),
                        elapsed,
                        format_duration(elapsed),
                        remaining,
                        format_duration(remaining),
                    );
                }

                // Try to refresh the access token, retrying on transient network
                // errors (e.g. DNS not ready yet after lid-open / resume from suspend).
                let refresh_result = {
                    let mut result = client.refresh_access_token(false).await;
                    for attempt in 1..=3u32 {
                        match &result {
                            Err(e) if is_tidlers_network_error(e) => {
                                let delay = 2u64 << (attempt - 1); // 2s, 4s, 8s
                                warn!(
                                    "Network error on token refresh (attempt {}/4), retrying in {}s: {}",
                                    attempt, delay, e
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                                result = client.refresh_access_token(false).await;
                            }
                            _ => break,
                        }
                    }
                    result
                };
                match refresh_result {
                    Ok(refreshed) => {
                        if refreshed {
                            info!("Successfully refreshed TIDAL access token");
                        } else {
                            info!("TIDAL session restored (token still valid, no refresh needed)");
                        }

                        // Update user info
                        if let Err(e) = client.refresh_user_info().await {
                            warn!("Failed to refresh user info: {:?}", e);
                        }

                        let username = client.user_info.as_ref().map(|u| u.username.clone());

                        // Build full user profile from tidlers User struct
                        let profile = {
                            use crate::tidal::auth::UserProfile;
                            if let Some(u) = &client.user_info {
                                info!(
                                    "TIDAL user fields — username: {:?}, first_name: {:?}, last_name: {:?}, full_name: {:?}, nickname: {:?}, email: {:?}",
                                    u.username,
                                    u.first_name,
                                    u.last_name,
                                    u.full_name,
                                    u.nickname,
                                    u.email
                                );
                                UserProfile {
                                    username: Some(u.username.clone()),
                                    first_name: u.first_name.clone(),
                                    last_name: u.last_name.clone(),
                                    full_name: u.full_name.clone(),
                                    nickname: u.nickname.clone(),
                                    email: Some(u.email.clone()),
                                    picture_url: None, // fetched separately below
                                    subscription_plan: None, // fetched separately below
                                }
                            } else {
                                UserProfile {
                                    username: username.clone(),
                                    ..Default::default()
                                }
                            }
                        };

                        // Store the refreshed session
                        let new_credentials = StoredCredentials {
                            session_json: client.get_json(),
                            stored_at: chrono::Utc::now(),
                            user_id: client.user_info.as_ref().map(|u| u.user_id.to_string()),
                            username: username.clone(),
                        };

                        if let Err(e) = AuthManager::store_credentials(&new_credentials) {
                            warn!("Failed to store refreshed credentials: {}", e);
                        }

                        // Log new token expiry info
                        if let (Some(expiry), Some(last_refresh)) = (
                            client.session.auth.refresh_expiry,
                            client.session.auth.last_refresh_time,
                        ) {
                            info!(
                                "Token valid for {}s (~{}) from last refresh",
                                expiry,
                                format_duration(expiry),
                            );
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            let remaining = (last_refresh + expiry).saturating_sub(now);
                            info!(
                                "Token will expire in {}s (~{})",
                                remaining,
                                format_duration(remaining),
                            );
                        }

                        client.set_audio_quality(self.audio_quality.clone());
                        *self.client.lock().await = Some(client);
                        self.auth_manager
                            .set_state(AuthState::Authenticated { profile });

                        // Fetch subscription plan + profile picture (best-effort)
                        self.fetch_and_set_profile_extras().await;

                        Ok(true)
                    }
                    Err(e) => {
                        if is_tidlers_network_error(&e) {
                            // Network errors are transient — keep credentials for next attempt
                            warn!(
                                "Token refresh failed after retries (network error, credentials preserved): {}",
                                e
                            );
                            Err(TidalError::NetworkError(format!("{}", e)))
                        } else {
                            // Auth / protocol error — credentials are likely invalid
                            warn!("Failed to refresh access token: {:?}", e);
                            let _ = AuthManager::delete_credentials();
                            self.auth_manager.set_state(AuthState::NotAuthenticated);
                            Err(TidalError::SessionExpired)
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to deserialize stored session: {:?}", e);
                // Clear invalid credentials
                let _ = AuthManager::delete_credentials();
                self.auth_manager.set_state(AuthState::NotAuthenticated);
                Err(TidalError::CredentialError(format!(
                    "Invalid stored session: {:?}",
                    e
                )))
            }
        }
    }

    /// Start the OAuth device code flow
    pub async fn start_oauth_flow(&mut self) -> TidalResult<DeviceCodeInfo> {
        info!("Starting OAuth device code flow");

        // tidlers' default OAuth client is entitled to lossless/hi-res playback
        // (playbackinfopostpaywall returns FLAC). Some TIDAL clients are capped
        // at HIGH/AAC regardless of the account tier, so which client we
        // authenticate as matters -- don't override it.
        let auth = TidalAuth::with_oauth();
        let client = TidalClient::new(&auth);

        match client.get_oauth_link().await {
            Ok(oauth) => {
                let device_info = DeviceCodeInfo {
                    verification_uri_complete: format!(
                        "https://{}",
                        oauth.verification_uri_complete
                    ),
                    user_code: oauth.user_code.clone(),
                    device_code: oauth.device_code.clone(),
                    expires_in: oauth.expires_in,
                    interval: oauth.interval,
                };

                self.auth_manager.set_state(AuthState::AwaitingUserAuth {
                    verification_uri: device_info.verification_uri_complete.clone(),
                    user_code: device_info.user_code.clone(),
                });

                // Store the client for later completion
                *self.client.lock().await = Some(client);

                info!("OAuth flow started, awaiting user authorization");
                Ok(device_info)
            }
            Err(e) => {
                error!("Failed to get OAuth link: {:?}", e);
                self.auth_manager
                    .set_state(AuthState::Failed(format!("{:?}", e)));
                Err(TidalError::AuthenticationFailed(format!("{:?}", e)))
            }
        }
    }

    /// Wait for the user to complete OAuth authorization
    pub async fn wait_for_oauth(
        &mut self,
        device_code: &str,
        expires_in: u64,
        interval: u64,
    ) -> TidalResult<()> {
        info!(
            "Waiting for user to complete OAuth authorization (device_code: {}..., expires_in: {}s, interval: {}s)",
            &device_code[..8.min(device_code.len())],
            expires_in,
            interval
        );

        let mut client_guard = self.client.lock().await;
        let client = client_guard.as_mut().ok_or_else(|| {
            error!("wait_for_oauth called but self.client is None!");
            TidalError::NotAuthenticated
        })?;

        info!("Calling tidlers wait_for_oauth...");
        match client
            .wait_for_oauth(device_code, expires_in, interval, None)
            .await
        {
            Ok(auth_response) => {
                info!("OAuth authorization completed successfully!");
                debug!("Auth response received: user_id={}", auth_response.user_id);

                // Log token expiry info
                if let (Some(expiry), Some(last_refresh)) = (
                    client.session.auth.refresh_expiry,
                    client.session.auth.last_refresh_time,
                ) {
                    info!(
                        "New OAuth token received - expires_in: {}s (~{})",
                        expiry,
                        format_duration(expiry),
                    );
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let remaining = (last_refresh + expiry).saturating_sub(now);
                    info!(
                        "Token will expire in {}s (~{})",
                        remaining,
                        format_duration(remaining),
                    );
                }

                // Refresh user info
                if let Err(e) = client.refresh_user_info().await {
                    warn!("Failed to refresh user info: {:?}", e);
                }

                let username = client.user_info.as_ref().map(|u| u.username.clone());
                let user_id = client.user_info.as_ref().map(|u| u.user_id.to_string());

                // Build full user profile from tidlers User struct
                let profile = {
                    use crate::tidal::auth::UserProfile;
                    if let Some(u) = &client.user_info {
                        info!(
                            "TIDAL user fields — username: {:?}, first_name: {:?}, last_name: {:?}, full_name: {:?}, nickname: {:?}, email: {:?}",
                            u.username, u.first_name, u.last_name, u.full_name, u.nickname, u.email
                        );
                        UserProfile {
                            username: Some(u.username.clone()),
                            first_name: u.first_name.clone(),
                            last_name: u.last_name.clone(),
                            full_name: u.full_name.clone(),
                            nickname: u.nickname.clone(),
                            email: Some(u.email.clone()),
                            picture_url: None,       // fetched separately below
                            subscription_plan: None, // fetched separately below
                        }
                    } else {
                        UserProfile {
                            username: username.clone(),
                            ..Default::default()
                        }
                    }
                };

                // Store credentials for future sessions
                let credentials = StoredCredentials {
                    session_json: client.get_json(),
                    stored_at: chrono::Utc::now(),
                    user_id,
                    username: username.clone(),
                };

                if let Err(e) = AuthManager::store_credentials(&credentials) {
                    warn!("Failed to store credentials: {}", e);
                }

                client.set_audio_quality(self.audio_quality.clone());
                // Drop the lock before calling fetch_and_set_subscription_plan
                // which needs &mut self (and internally re-acquires the lock).
                drop(client_guard);

                self.auth_manager
                    .set_state(AuthState::Authenticated { profile });

                // Fetch subscription plan + profile picture (best-effort)
                self.fetch_and_set_profile_extras().await;

                Ok(())
            }
            Err(e) => {
                error!("OAuth authorization failed with error: {:?}", e);
                self.auth_manager
                    .set_state(AuthState::Failed(format!("{:?}", e)));
                *client_guard = None;
                Err(TidalError::AuthenticationFailed(format!("{:?}", e)))
            }
        }
    }

    /// Logout and clear stored credentials
    pub async fn logout(&mut self) {
        info!("Logging out of TIDAL");
        *self.client.lock().await = None;
        self.auth_manager.set_state(AuthState::NotAuthenticated);
        let _ = AuthManager::delete_credentials();
    }

    /// Search for tracks, albums, artists, and playlists
    pub async fn search(&self, query: &str, limit: u32) -> TidalResult<SearchResults> {
        // Ensure token is valid before the operation
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Searching for: {}", query);

        use tidlers::client::models::search::config::{SearchConfig, SearchType};

        let config = SearchConfig {
            query: query.to_string(),
            types: vec![
                SearchType::Tracks,
                SearchType::Albums,
                SearchType::Artists,
                SearchType::Playlists,
                SearchType::Videos,
            ],
            limit,
            ..Default::default()
        };

        match client.search(config).await {
            Ok(results) => {
                let mut search_results = SearchResults::default();

                // Convert tracks from SearchTrackHit
                if let Some(tracks) = results.tracks {
                    search_results.tracks = tracks.items.into_iter().map(Track::from).collect();
                }

                // Convert albums from SearchAlbumHit
                if let Some(albums) = results.albums {
                    search_results.albums = albums.items.into_iter().map(Album::from).collect();
                }

                // Convert artists from SearchArtistHit
                if let Some(artists) = results.artists {
                    search_results.artists = artists.items.into_iter().map(Artist::from).collect();
                }

                // Convert playlists from SearchPlaylistHit
                if let Some(playlists) = results.playlists {
                    search_results.playlists =
                        playlists.items.into_iter().map(Playlist::from).collect();
                }

                // Convert videos into playable tracks (is_video = true). Videos
                // have no album; their thumbnail is the `image` UUID, mirroring
                // how playlist/Explore video items get their cover.
                if let Some(videos) = results.videos {
                    search_results.videos = videos
                        .items
                        .into_iter()
                        .map(|v| {
                            let artist = v.artists.first();
                            Track {
                                id: v.id.to_string(),
                                title: v.title,
                                duration: v.duration as u32,
                                track_number: v.track_number.unwrap_or(0),
                                artist_name: artist
                                    .and_then(|a| a.name.clone())
                                    .unwrap_or_else(|| "Unknown Artist".to_string()),
                                artist_id: artist.and_then(|a| a.id).map(|id| id.to_string()),
                                album_name: v.album.as_ref().map(|a| a.title.clone()),
                                album_id: v.album.as_ref().map(|a| a.id.to_string()),
                                cover_url: v.image.as_deref().map(tidal_cover_url),
                                explicit: v.explicit,
                                audio_quality: None,
                                is_video: true,
                            }
                        })
                        .collect();
                }

                Ok(search_results)
            }
            Err(e) => {
                error!("Search failed: {:?}", e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    pub async fn get_user_playlists(
        &self,
        _limit: Option<u32>,
        _offset: Option<u32>,
    ) -> TidalResult<Vec<Playlist>> {
        // Ensure token is valid before the operation
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting user playlists");

        match client.list_playlists().await {
            Ok(response) => {
                let playlists: Vec<Playlist> =
                    response.items.into_iter().map(Playlist::from).collect();
                Ok(playlists)
            }
            Err(e) => {
                error!("Failed to get playlists: {:?}", e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    /// Get user's favorite tracks (paginated — fetches all pages)
    pub async fn get_user_favorite_tracks(&self, _limit: Option<u32>) -> TidalResult<Vec<Track>> {
        self.ensure_valid_token().await?;

        let ctx = self.auth_context_with_user().await?;

        debug!("Getting user favorite tracks (paginated)");

        let http_client = reqwest::Client::new();
        let page_size: u32 = 100;
        let mut offset: u32 = 0;
        let mut all_tracks: Vec<Track> = Vec::new();

        loop {
            let url = format!(
                "https://api.tidal.com/v1/users/{}/favorites/tracks?countryCode={}&limit={}&offset={}",
                ctx.user_id, ctx.country_code, page_size, offset
            );

            let response = http_client
                .get(&url)
                .header(AUTHORIZATION, format!("Bearer {}", ctx.access_token))
                .send()
                .await
                .map_err(|e| TidalError::NetworkError(format!("HTTP request failed: {}", e)))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                error!("Favorite tracks request failed: {} - {}", status, body);
                return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
            }

            let body = response
                .text()
                .await
                .map_err(|e| TidalError::NetworkError(format!("reading favorites body: {}", e)))?;

            let parsed: ApiPaginatedResponse<ApiItemWrapper<ApiTrackData>> =
                serde_json::from_str(&body)
                    .map_err(|e| TidalError::ParseError(format!("favorite tracks JSON: {}", e)))?;

            let total = parsed.total_number_of_items as u32;
            let page_items = parsed.items.len() as u32;

            all_tracks.extend(
                parsed
                    .items
                    .into_iter()
                    .filter_map(|w| w.item)
                    .map(Track::from),
            );

            offset += page_items;
            info!(
                "Fetched favorite tracks page: {} / {} total",
                all_tracks.len(),
                total
            );

            if page_items == 0 || offset >= total {
                break;
            }
        }

        Ok(all_tracks)
    }

    /// Get user's favorite albums (paginated — fetches all pages)
    pub async fn get_user_favorite_albums(&self, _limit: Option<u32>) -> TidalResult<Vec<Album>> {
        self.ensure_valid_token().await?;

        let ctx = self.auth_context_with_user().await?;

        debug!("Getting user favorite albums (paginated)");

        let http_client = reqwest::Client::new();
        let page_size: u32 = 100;
        let mut offset: u32 = 0;
        let mut all_albums: Vec<Album> = Vec::new();

        loop {
            let url = format!(
                "https://api.tidal.com/v1/users/{}/favorites/albums?countryCode={}&limit={}&offset={}",
                ctx.user_id, ctx.country_code, page_size, offset
            );

            let response = http_client
                .get(&url)
                .header(AUTHORIZATION, format!("Bearer {}", ctx.access_token))
                .send()
                .await
                .map_err(|e| TidalError::NetworkError(format!("HTTP request failed: {}", e)))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                error!("Favorite albums request failed: {} - {}", status, body);
                return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
            }

            let body = response.text().await.map_err(|e| {
                TidalError::NetworkError(format!("reading favorite albums body: {}", e))
            })?;

            let parsed: ApiPaginatedResponse<ApiItemWrapper<ApiAlbumData>> =
                serde_json::from_str(&body)
                    .map_err(|e| TidalError::ParseError(format!("favorite albums JSON: {}", e)))?;

            let total = parsed.total_number_of_items as u32;
            let page_items = parsed.items.len() as u32;

            all_albums.extend(
                parsed
                    .items
                    .into_iter()
                    .filter_map(|w| w.item)
                    .map(Album::from),
            );

            offset += page_items;
            info!(
                "Fetched favorite albums page: {} / {} total",
                all_albums.len(),
                total
            );

            if page_items == 0 || offset >= total {
                break;
            }
        }

        Ok(all_albums)
    }

    /// Get playlist items (tracks).
    ///
    /// Paginates through `GET /v1/playlists/{uuid}/items` with a **hand-rolled**
    /// request and our lenient [`ApiTrackData`] parser, rather than tidlers'
    /// `get_playlist_items()`. TIDAL playlists can contain video items whose
    /// `album` field is `null`; tidlers' strict deserializer rejects those and
    /// fails the entire playlist. Our parser tolerates the null album, so video
    /// playlists load (video entries surface as tracks with no album/cover).
    ///
    /// `limit` is the page size (capped at 100); `_offset` is ignored (we always
    /// start from 0 and walk to the end).
    pub async fn get_playlist_tracks(
        &self,
        playlist_uuid: &str,
        limit: Option<u32>,
        _offset: Option<u32>,
    ) -> TidalResult<Vec<Track>> {
        self.ensure_valid_token().await?;
        debug!("Getting playlist tracks for: {}", playlist_uuid);

        let ctx = self.auth_context_with_user().await?;
        let http_client = reqwest::Client::new();
        let page_size: u32 = limit.unwrap_or(100).min(100);
        let mut offset: u32 = 0;
        let mut all_tracks: Vec<Track> = Vec::new();

        loop {
            let url = format!(
                "https://api.tidal.com/v1/playlists/{}/items?countryCode={}&limit={}&offset={}&order=INDEX&orderDirection=ASC",
                playlist_uuid, ctx.country_code, page_size, offset
            );

            let response = http_client
                .get(&url)
                .header(AUTHORIZATION, format!("Bearer {}", ctx.access_token))
                .send()
                .await
                .map_err(|e| {
                    TidalError::NetworkError(format!("playlist items request failed: {}", e))
                })?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                error!("Playlist items request failed: {} - {}", status, body);
                return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
            }

            let body = response.text().await.map_err(|e| {
                TidalError::NetworkError(format!("reading playlist items body: {}", e))
            })?;

            let parsed: ApiPaginatedResponse<ApiItemWrapper<ApiTrackData>> =
                serde_json::from_str(&body)
                    .map_err(|e| TidalError::ParseError(format!("playlist items JSON: {}", e)))?;

            let total = parsed.total_number_of_items.max(0) as u32;
            let page_items = parsed.items.len() as u32;

            all_tracks.extend(parsed.items.into_iter().filter_map(|w| {
                let is_video = w.item_type.as_deref() == Some("video");
                w.item.map(|it| {
                    let mut track = Track::from(it);
                    track.is_video = is_video;
                    track
                })
            }));

            offset += page_items;
            info!(
                "Fetched playlist tracks page: {} / {} total",
                all_tracks.len(),
                total
            );

            if page_items == 0 || offset >= total {
                break;
            }
        }

        Ok(all_tracks)
    }

    /// Resolve the playable HLS (`.m3u8`) URL for a music **video**.
    ///
    /// TIDAL videos are DRM-free HLS: `GET /v1/videos/{id}/playbackinfopostpaywall`
    /// returns a base64 "EMU" manifest that simply wraps the HLS master URL.
    /// We decode it and hand the URL to the GStreamer pipeline. (Verified the
    /// inner HLS carries no `EXT-X-KEY`/Widevine, so no CDM is needed.)
    pub async fn get_video_hls_url(&self, video_id: &str) -> TidalResult<String> {
        self.ensure_valid_token().await?;
        let ctx = self.auth_context().await?;

        let url = format!(
            "https://api.tidal.com/v1/videos/{}/playbackinfopostpaywall?videoquality=HIGH&playbackmode=STREAM&assetpresentation=FULL&countryCode={}",
            video_id, ctx.country_code
        );
        debug!("Fetching video playback info for: {}", video_id);

        let http_client = reqwest::Client::new();
        let response = http_client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", ctx.access_token))
            .send()
            .await
            .map_err(|e| {
                TidalError::NetworkError(format!("video playback request failed: {}", e))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Video playback info failed: {} - {}", status, body);
            return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
        }

        let body = response
            .text()
            .await
            .map_err(|e| TidalError::NetworkError(format!("reading video playback body: {}", e)))?;

        #[derive(Deserialize)]
        struct VideoPlaybackInfo {
            manifest: String,
        }
        #[derive(Deserialize)]
        struct EmuManifest {
            urls: Vec<String>,
        }

        let info: VideoPlaybackInfo = serde_json::from_str(&body)
            .map_err(|e| TidalError::ParseError(format!("video playback JSON: {}", e)))?;
        let manifest_bytes = general_purpose::STANDARD
            .decode(info.manifest.as_bytes())
            .map_err(|e| TidalError::ParseError(format!("video manifest base64: {}", e)))?;
        let emu: EmuManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| TidalError::ParseError(format!("video EMU manifest JSON: {}", e)))?;

        emu.urls
            .into_iter()
            .next()
            .ok_or_else(|| TidalError::ParseError("video manifest contained no URLs".to_string()))
    }

    /// Get album tracks
    pub async fn get_album_tracks(
        &self,
        album_id: &str,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> TidalResult<Vec<Track>> {
        // Ensure token is valid before the operation
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting album tracks for: {}", album_id);

        match client
            .get_album_items(
                album_id.to_string(),
                Some(limit.unwrap_or(100) as u64),
                offset.map(|o| o as u64),
            )
            .await
        {
            Ok(response) => {
                let tracks: Vec<Track> = response
                    .items
                    .into_iter()
                    .map(|item| Track::from(item.item))
                    .collect();
                Ok(tracks)
            }
            Err(e) => {
                error!("Failed to get album tracks: {:?}", e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    /// Get a single track's metadata by ID.
    ///
    /// Wraps tidlers' `get_track` and converts to our domain `Track`.
    pub async fn get_track_by_id(&self, track_id: &str) -> TidalResult<Track> {
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting track info for: {}", track_id);

        match client.get_track(track_id.to_string()).await {
            Ok(response) => Ok(Track::from(response)),
            Err(e) => {
                error!("Failed to get track info: {:?}", e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    /// Get track playback URL with full DASH support for HiRes quality
    ///
    /// For HiRes quality, TIDAL returns DASH manifests. This function writes
    /// the DASH manifest to a temporary file and returns the path.
    ///
    /// For Low/High/Lossless quality, returns a direct streaming URL.
    pub async fn get_track_playback_url(&self, track_id: &str) -> TidalResult<PlaybackUrl> {
        // Ensure token is valid before the operation
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        info!(
            "Getting playback URL for track: {} with quality: {:?} (cache miss)",
            track_id, self.audio_quality
        );

        // Get auth info for our own request (we need the raw manifest)
        let access_token = client.session.auth.access_token.as_ref().ok_or_else(|| {
            error!("No access token available");
            TidalError::NotAuthenticated
        })?;

        let country_code = client
            .user_info
            .as_ref()
            .map(|u| u.country_code.as_str())
            .unwrap_or("US");

        let url = format!(
            "https://api.tidal.com/v1/tracks/{}/playbackinfopostpaywall?audioquality={}&playbackmode=STREAM&assetpresentation=FULL&countryCode={}",
            track_id, self.audio_quality, country_code
        );

        let http_client = reqwest::Client::new();
        let response = http_client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", access_token))
            .send()
            .await
            .map_err(|e| TidalError::RequestFailed(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Playback info request failed: {} - {}", status, body);
            return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
        }

        let body = response
            .text()
            .await
            .map_err(|e| TidalError::RequestFailed(format!("Failed to read response: {}", e)))?;

        // Parse the response
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| TidalError::ParseError(format!("Failed to parse JSON: {}", e)))?;

        let manifest_mime_type = parsed
            .get("manifestMimeType")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let audio_quality = parsed
            .get("audioQuality")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let audio_mode = parsed
            .get("audioMode")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let replay_gain_db = parsed
            .get("albumReplayGain")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32);

        let peak_amplitude = parsed
            .get("albumPeakAmplitude")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32);

        info!(
            "Playback info received - audio_quality: {}, audio_mode: {}, manifest_mime_type: {}, replay_gain: {:?} dB, peak: {:?}",
            audio_quality, audio_mode, manifest_mime_type, replay_gain_db, peak_amplitude
        );

        let manifest_b64 = parsed
            .get("manifest")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TidalError::ParseError("No manifest in response".to_string()))?;

        let manifest_bytes = general_purpose::STANDARD
            .decode(manifest_b64)
            .map_err(|e| TidalError::ParseError(format!("Failed to decode manifest: {}", e)))?;

        let manifest_str = String::from_utf8(manifest_bytes)
            .map_err(|e| TidalError::ParseError(format!("Invalid UTF-8 in manifest: {}", e)))?;

        // Check if this is a DASH manifest (used for HiRes)
        if manifest_mime_type.contains("dash") {
            info!("DASH manifest detected for HiRes quality - playing inline");
            let preview_len = manifest_str.len().min(500);
            let preview: String = manifest_str.chars().take(preview_len).collect();
            debug!("DASH manifest content:\n{}", preview);

            // Hand the manifest to GStreamer inline (as a data: URI) rather than
            // writing it to disk — see `PlaybackUrl::as_url`. The manifest is
            // single-use anyway (its segment URLs carry short-lived tokens), so
            // there is nothing worth persisting.
            return Ok(PlaybackUrl::DashManifest(manifest_str, replay_gain_db));
        }

        // For non-DASH (JSON manifest with direct URLs)
        let manifest: serde_json::Value = serde_json::from_str(&manifest_str)
            .map_err(|e| TidalError::ParseError(format!("Failed to parse manifest JSON: {}", e)))?;

        if let Some(urls) = manifest.get("urls").and_then(|v| v.as_array())
            && let Some(first_url) = urls.first()
            && let Some(url_str) = first_url.as_str()
        {
            info!("Got direct playback URL");
            return Ok(PlaybackUrl::Direct(url_str.to_string(), replay_gain_db));
        }

        Err(TidalError::RequestFailed(
            "No playback URL available".to_string(),
        ))
    }

    /// Add a track to user's favorites
    pub async fn add_favorite_track(&self, track_id: &str) -> TidalResult<()> {
        debug!("Adding track {} to favorites", track_id);
        self.add_to_favorites(FavoriteResourceType::Tracks, track_id)
            .await
    }

    /// Remove a track from user's favorites
    pub async fn remove_favorite_track(&self, track_id: &str) -> TidalResult<()> {
        debug!("Removing track {} from favorites", track_id);
        self.remove_from_favorites(FavoriteResourceType::Tracks, track_id)
            .await
    }

    // =========================================================================
    // Artist Detail
    // =========================================================================

    /// Get full artist information (picture, popularity, roles, etc.)
    pub async fn get_artist_info(&self, artist_id: &str) -> TidalResult<Artist> {
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting artist info for: {}", artist_id);

        match client.get_artist(artist_id.to_string()).await {
            Ok(response) => {
                let mut artist = Artist::from(response);
                // Try to fetch bio separately (it may fail for some artists)
                drop(client_guard);
                if let Ok(bio) = self.get_artist_bio(artist_id).await {
                    artist.bio = Some(bio);
                }
                Ok(artist)
            }
            Err(e) => {
                error!("Failed to get artist info: {:?}", e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    /// Get artist biography text
    async fn get_artist_bio(&self, artist_id: &str) -> TidalResult<String> {
        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting artist bio for: {}", artist_id);

        match client.get_artist_bio(artist_id.to_string()).await {
            Ok(response) => {
                // Prefer summary over full text for the applet UI
                if response.summary.is_empty() {
                    Ok(response.text)
                } else {
                    Ok(response.summary)
                }
            }
            Err(e) => {
                debug!("No bio available for artist {}: {:?}", artist_id, e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    /// Get an artist's top tracks
    pub async fn get_artist_top_tracks(
        &self,
        artist_id: &str,
        limit: Option<u32>,
    ) -> TidalResult<Vec<Track>> {
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting top tracks for artist: {}", artist_id);

        match client
            .get_artist_tracks(artist_id.to_string(), limit.map(|l| l as u64), None)
            .await
        {
            Ok(response) => {
                let tracks = response.items.into_iter().map(Track::from).collect();
                Ok(tracks)
            }
            Err(e) => {
                error!("Failed to get artist top tracks: {:?}", e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    /// Get an artist's albums (discography)
    pub async fn get_artist_albums(
        &self,
        artist_id: &str,
        limit: Option<u32>,
    ) -> TidalResult<Vec<Album>> {
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting albums for artist: {}", artist_id);

        match client
            .get_artist_albums(artist_id.to_string(), limit.map(|l| l as u64), None)
            .await
        {
            Ok(response) => {
                let albums = response.items.into_iter().map(Album::from).collect();
                Ok(albums)
            }
            Err(e) => {
                error!("Failed to get artist albums: {:?}", e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    /// Get an artist's music videos as playable tracks (`is_video = true`).
    ///
    /// The video thumbnail comes from `imageId` via [`tidal_cover_url`], the
    /// same cover path playlist/Explore/search video items use.
    pub async fn get_artist_videos(
        &self,
        artist_id: &str,
        limit: Option<u32>,
    ) -> TidalResult<Vec<Track>> {
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting videos for artist: {}", artist_id);

        match client
            .get_artist_videos(artist_id.to_string(), limit.map(|l| l as u64), None)
            .await
        {
            Ok(response) => {
                let videos = response
                    .items
                    .into_iter()
                    .map(|v| Track {
                        id: v.id.to_string(),
                        title: v.title,
                        duration: v.duration,
                        track_number: v.track_number,
                        artist_name: v.artist.name,
                        artist_id: Some(v.artist.id.to_string()),
                        album_name: v.album.as_ref().map(|a| a.title.clone()),
                        album_id: v.album.as_ref().map(|a| a.id.to_string()),
                        cover_url: v.image_id.as_deref().map(tidal_cover_url),
                        explicit: v.explicit,
                        audio_quality: None,
                        is_video: true,
                    })
                    .collect();
                Ok(videos)
            }
            Err(e) => {
                error!("Failed to get artist videos: {:?}", e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    // =========================================================================
    // Album Detail (by ID)
    // =========================================================================

    /// Get full album information by ID (for navigating from now-playing bar).
    ///
    /// Also attempts to fetch the album review text from the TIDAL editorial
    /// endpoint (`/v1/albums/{id}/review`).  The review is optional — if the
    /// request fails (many albums have no review) we silently ignore it.
    pub async fn get_album_info(&self, album_id: &str) -> TidalResult<Album> {
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting album info for: {}", album_id);

        match client.get_album(album_id.to_string()).await {
            Ok(response) => {
                let mut album = Album::from(response);
                // Try to fetch review separately (it may fail for most albums)
                drop(client_guard);
                if let Ok(review) = self.get_album_review(album_id).await {
                    album.review = Some(review);
                }
                Ok(album)
            }
            Err(e) => {
                error!("Failed to get album info: {:?}", e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    /// Get album review / editorial text from TIDAL.
    ///
    /// Delegates to tidlers' `get_album_review` (`GET /v1/albums/{id}/review`).
    /// Many albums have no review, so callers treat any error as "no review".
    pub async fn get_album_review(&self, album_id: &str) -> TidalResult<String> {
        self.ensure_valid_token().await?;
        debug!("Fetching album review for: {}", album_id);

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        let review = client
            .get_album_review(album_id.to_string())
            .await
            .map_err(|e| TidalError::RequestFailed(format!("album review: {e:?}")))?;

        if review.text.is_empty() {
            return Err(TidalError::RequestFailed(
                "Review text is empty".to_string(),
            ));
        }

        Ok(review.text)
    }

    // =========================================================================
    // Track Lyrics
    // =========================================================================

    /// Fetch lyrics for a track from TIDAL.
    ///
    /// Hits the TIDAL v1 API endpoint `GET /v1/tracks/{id}/lyrics` directly
    /// (tidlers' v2/OpenAPI lyrics surface needs a different OAuth flow
    /// than mare's internal client; this v1 path works with the access
    /// token we already hold).
    ///
    /// The endpoint returns plain `lyrics` and LRC-format `subtitles`
    /// in parallel; we surface both via [`TrackLyrics`].  A `404`
    /// (TIDAL has no lyrics for this track) is mapped to an empty
    /// `TrackLyrics`, not an error — the UI distinguishes "loading·
    /// vs no-lyrics·vs error" by inspecting the result.
    pub async fn get_track_lyrics(&self, track_id: &str) -> TidalResult<TrackLyrics> {
        let ctx = self.auth_context().await?;

        let url = format!(
            "https://api.tidal.com/v1/tracks/{}/lyrics?countryCode={}",
            track_id, ctx.country_code
        );

        debug!("Fetching lyrics for track {}", track_id);

        let http_client = reqwest::Client::new();
        let response = http_client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", ctx.access_token))
            .send()
            .await
            .map_err(|e| TidalError::NetworkError(format!("{:?}", e)))?;

        // 404 / 401 with empty lyrics: "no lyrics available" for this
        // track.  Not an error — just an empty result.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            debug!("No lyrics found for track {}", track_id);
            return Ok(TrackLyrics::default());
        }

        if !response.status().is_success() {
            return Err(TidalError::RequestFailed(format!(
                "HTTP {} fetching lyrics for track {}",
                response.status(),
                track_id
            )));
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct LyricsResponse {
            #[serde(default)]
            lyrics: Option<String>,
            #[serde(default)]
            subtitles: Option<String>,
            #[serde(default)]
            lyrics_provider: Option<String>,
            #[serde(default)]
            is_right_to_left: bool,
        }

        let raw: LyricsResponse = response
            .json()
            .await
            .map_err(|e| TidalError::ParseError(format!("{:?}", e)))?;

        let plain_text = raw.lyrics.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() { None } else { Some(s) }
        });
        let lrc_lines = raw
            .subtitles
            .as_deref()
            .map(crate::tidal::models::parse_lrc)
            .unwrap_or_default();

        info!(
            "Loaded lyrics for track {}: provider={:?} plain={} synced_lines={}",
            track_id,
            raw.lyrics_provider,
            plain_text.is_some(),
            lrc_lines.len()
        );

        Ok(TrackLyrics {
            provider: raw.lyrics_provider,
            plain_text,
            lrc_lines,
            is_right_to_left: raw.is_right_to_left,
        })
    }

    // =========================================================================
    // Album Favorites
    // =========================================================================

    /// Add an album to user's favorites
    pub async fn add_favorite_album(&self, album_id: &str) -> TidalResult<()> {
        debug!("Adding album {} to favorites", album_id);
        self.add_to_favorites(FavoriteResourceType::Albums, album_id)
            .await
    }

    /// Remove an album from user's favorites
    pub async fn remove_favorite_album(&self, album_id: &str) -> TidalResult<()> {
        debug!("Removing album {} from favorites", album_id);
        self.remove_from_favorites(FavoriteResourceType::Albums, album_id)
            .await
    }
    /// Fetch the user's subscription plan.
    ///
    /// Tries tidlers' built-in `client.subscription()` first (uses the v1
    /// endpoint internally). If that fails (e.g. because of a type mismatch
    /// on `premiumAccess`), falls back to a raw HTTP call with lenient JSON
    /// parsing.
    ///
    /// Returns a human-readable label such as "HiFi Plus", "HiFi", or "Free".
    /// On any failure the method returns `Ok(None)` so callers can treat the
    /// plan badge as optional.
    async fn get_user_subscription(&self) -> TidalResult<Option<String>> {
        self.ensure_valid_token().await?;

        // --- Attempt 1: tidlers built-in subscription() -----------------------
        {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

            match client.subscription().await {
                Ok(sub) => {
                    info!(
                        "tidlers subscription() — type: {:?}, highest_quality: {:?}",
                        sub.subscription.subscription_type, sub.highest_sound_quality
                    );
                    let label = Self::derive_plan_label_from_type_and_quality(
                        &sub.subscription.subscription_type,
                        &sub.highest_sound_quality,
                    );
                    if let Some(l) = &label {
                        info!("User subscription plan (via tidlers): {}", l);
                    }
                    return Ok(label);
                }
                Err(e) => {
                    warn!(
                        "tidlers subscription() failed ({}), falling back to raw HTTP",
                        e
                    );
                }
            }
        } // client_guard dropped

        // --- Attempt 2: raw HTTP with lenient JSON parsing --------------------
        let (user_id, access_token) = {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
            let uid = match client.session.auth.user_id {
                Some(id) => id,
                None => {
                    warn!("No user ID available – cannot fetch subscription");
                    return Ok(None);
                }
            };
            let token = match client.session.auth.access_token.as_ref() {
                Some(t) => t.clone(),
                None => {
                    warn!("No access token available – cannot fetch subscription");
                    return Ok(None);
                }
            };
            (uid, token)
        }; // client_guard dropped

        let url = format!("https://api.tidal.com/v1/users/{}/subscription", user_id);

        let http_client = reqwest::Client::new();
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", access_token))
                .map_err(|e| TidalError::RequestFailed(format!("Invalid auth header: {}", e)))?,
        );

        match http_client.get(&url).headers(headers).send().await {
            Ok(response) => {
                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    warn!("Subscription endpoint returned HTTP {}: {}", status, body);
                    return Ok(None);
                }

                let body = response.text().await.unwrap_or_default();
                debug!("Subscription raw response: {}", body);

                // Parse with serde_json::Value first for maximum flexibility —
                // `premiumAccess` can be a string OR a bool depending on TIDAL
                // API version / account type.
                match serde_json::from_str::<serde_json::Value>(&body) {
                    Ok(v) => {
                        let premium_access = v
                            .get("premiumAccess")
                            .and_then(|p| p.as_str().map(String::from));
                        let sub_type = v
                            .get("subscription")
                            .and_then(|s| s.get("type"))
                            .and_then(|t| t.as_str().map(String::from));
                        let highest_quality = v
                            .get("highestSoundQuality")
                            .and_then(|h| h.as_str().map(String::from));

                        let label = Self::derive_plan_label(
                            premium_access.as_deref(),
                            sub_type.as_deref(),
                            highest_quality.as_deref(),
                        );
                        if let Some(l) = &label {
                            info!("User subscription plan (via raw HTTP): {}", l);
                        }
                        Ok(label)
                    }
                    Err(e) => {
                        warn!("Failed to parse subscription JSON: {}", e);
                        Ok(None)
                    }
                }
            }
            Err(e) => {
                warn!("Failed to fetch subscription info: {:?}", e);
                Ok(None)
            }
        }
    }

    /// Derive a human-readable plan label from `subscription.type` and
    /// `highestSoundQuality` (used when we have tidlers' typed response).
    pub fn derive_plan_label_from_type_and_quality(
        sub_type: &str,
        highest_quality: &str,
    ) -> Option<String> {
        Self::derive_plan_label(None, Some(sub_type), Some(highest_quality))
    }

    /// Derive a human-readable plan label from the three possible indicators.
    ///
    /// Priority: `premiumAccess` > `subscription.type` > `highestSoundQuality`.
    ///
    /// Special case: when `sub_type` is `"PREMIUM"`, we still check
    /// `highestSoundQuality` — TIDAL Family accounts report type `"PREMIUM"`
    /// but actually have full HiFi Plus capabilities (HI_RES_LOSSLESS).
    pub fn derive_plan_label(
        premium_access: Option<&str>,
        sub_type: Option<&str>,
        highest_quality: Option<&str>,
    ) -> Option<String> {
        // 1. premiumAccess (string, clearest when present)
        match premium_access {
            Some("HIFI_PLUS") => return Some("HiFi Plus".to_string()),
            Some("HIFI") => return Some("HiFi".to_string()),
            Some(other) if !other.is_empty() => return Some(Self::title_case(other)),
            _ => {}
        }

        // 2. subscription.type — but for "PREMIUM", also check sound quality
        //    because Family plans report type "PREMIUM" while actually
        //    supporting HI_RES_LOSSLESS (HiFi Plus).
        match sub_type {
            Some("HIFI") => return Some("HiFi".to_string()),
            Some("PREMIUM") => {
                // Let highestSoundQuality override when it indicates a
                // higher tier than "Premium" (e.g. Family accounts).
                match highest_quality {
                    Some("HI_RES_LOSSLESS") | Some("HI_RES") => {
                        return Some("HiFi Plus".to_string());
                    }
                    Some("LOSSLESS") => return Some("HiFi".to_string()),
                    _ => return Some("Premium".to_string()),
                }
            }
            Some("FREE") => return Some("Free".to_string()),
            Some(other) if !other.is_empty() => return Some(Self::title_case(other)),
            _ => {}
        }

        // 3. highestSoundQuality (last resort, when sub_type is absent)
        match highest_quality {
            Some("HI_RES_LOSSLESS") | Some("HI_RES") => Some("HiFi Plus".to_string()),
            Some("LOSSLESS") => Some("HiFi".to_string()),
            Some("HIGH") => Some("High".to_string()),
            Some("LOW") => Some("Free".to_string()),
            _ => None,
        }
    }

    /// Title-case an UPPER_SNAKE value: "HIFI_PLUS" → "Hifi Plus"
    pub fn title_case(s: &str) -> String {
        s.replace('_', " ")
            .split_whitespace()
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().to_string() + &c.as_str().to_lowercase(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Fetch enriched user profile data from TIDAL's API endpoints.
    ///
    /// The tidlers `User` struct (populated during OAuth / refresh_user_info)
    /// often has `first_name`, `last_name`, `full_name`, and `nickname` as
    /// `None`, and never includes a profile picture. This method queries:
    ///
    /// 1. `GET /v1/users/{id}` — returns `firstName`, `lastName`, and
    ///    sometimes a `picture` UUID.
    /// 2. `GET /v2/profiles/{id}` — returns `name`, `handle`, and a nested
    ///    `picture.url` UUID.
    ///
    /// Calls tidlers' `get_user_v1` (firstName / lastName) and `get_user_v2`
    /// (display name + picture URL).
    ///
    /// Returns `(picture_url, display_name, first_name, last_name)` — each
    /// `Option` so callers can merge into the existing profile.
    async fn get_user_profile_extras(
        &self,
    ) -> TidalResult<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> {
        self.ensure_valid_token().await?;

        let user_id = {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
            match client.session.auth.user_id {
                Some(id) => id,
                None => return Ok((None, None, None, None)),
            }
        };

        let mut picture_url: Option<String> = None;
        let mut display_name: Option<String> = None;
        let mut first_name: Option<String> = None;
        let mut last_name: Option<String> = None;

        // --- v1 ---------------------------------------------------------
        // tidlers' UserV1Response only exposes id / firstName / lastName.
        // (The v1 endpoint also returns a `picture` UUID, but tidlers
        // doesn't deserialize it; v2 below is the primary source.)
        {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
            match client.get_user_v1(user_id.to_string()).await {
                Ok(v1) => {
                    if let Some(f) = v1.first_name {
                        let f = f.trim().to_string();
                        if !f.is_empty() {
                            info!("v1 firstName: {:?}", f);
                            first_name = Some(f);
                        }
                    }
                    if let Some(l) = v1.last_name {
                        let l = l.trim().to_string();
                        if !l.is_empty() {
                            info!("v1 lastName: {:?}", l);
                            last_name = Some(l);
                        }
                    }
                }
                Err(e) => debug!("get_user_v1 failed: {e:?}"),
            }
        }

        // --- v2 ---------------------------------------------------------
        // Display name + profile picture URL.
        {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
            match client.get_user_v2(user_id.to_string()).await {
                Ok(v2) => {
                    if let Some(name) = v2.name {
                        let name = name.trim().to_string();
                        if !name.is_empty() {
                            info!("v2 profile name: {:?}", name);
                            display_name = Some(name);
                        }
                    }
                    if let Some(pic) = v2.picture {
                        if pic.url.starts_with("http") {
                            picture_url = Some(pic.url);
                        } else if !pic.url.is_empty() {
                            picture_url = Some(tidal_cover_url(&pic.url));
                        }
                    }
                }
                Err(e) => debug!("get_user_v2 failed: {e:?}"),
            }
        }

        if let Some(url) = &picture_url {
            info!("Resolved profile picture URL: {}", url);
        } else {
            info!("No profile picture found for user {}", user_id);
        }

        Ok((picture_url, display_name, first_name, last_name))
    }

    /// Try to extract a picture URL from a JSON value that might contain
    /// picture fields in various TIDAL API formats.
    pub fn extract_picture_url_from_json(v: &serde_json::Value) -> Option<String> {
        for field in &[
            "profilePicture",
            "picture",
            "pictureUrl",
            "profilePictureUrl",
        ] {
            if let Some(val) = v.get(*field) {
                // Direct string — could be a URL or a UUID
                if let Some(url_str) = val.as_str()
                    && !url_str.is_empty()
                {
                    if url_str.starts_with("http") {
                        return Some(url_str.to_string());
                    }
                    // Treat as UUID
                    return Some(tidal_cover_url(url_str));
                }

                // Nested object — e.g. { "url": "uuid" } or { "320x320": "https://..." }
                if let Some(obj) = val.as_object() {
                    // First check for a "url" key (TIDAL v2 profile format)
                    if let Some(url_val) = obj.get("url").and_then(|u| u.as_str())
                        && !url_val.is_empty()
                    {
                        if url_val.starts_with("http") {
                            return Some(url_val.to_string());
                        }
                        // Treat as UUID
                        return Some(tidal_cover_url(url_val));
                    }
                    // Then try size keys
                    for size_key in &["320x320", "640x640", "750x750", "medium", "large", "small"] {
                        if let Some(url_str) = obj.get(*size_key).and_then(|u| u.as_str())
                            && !url_str.is_empty()
                        {
                            if url_str.starts_with("http") {
                                return Some(url_str.to_string());
                            }
                            return Some(tidal_cover_url(url_str));
                        }
                    }
                }
            }
        }
        None
    }

    /// Fetch and attach extra profile info (subscription plan, profile picture,
    /// and display name) to the current auth profile.
    ///
    /// Called after session restore or OAuth completion. All fetches are
    /// best-effort — failures are logged but do not affect authentication.
    async fn fetch_and_set_profile_extras(&mut self) {
        let mut plan: Option<String> = None;
        let mut picture: Option<String> = None;
        let mut api_name: Option<String> = None;
        let mut api_first: Option<String> = None;
        let mut api_last: Option<String> = None;

        // Fetch subscription plan
        match self.get_user_subscription().await {
            Ok(Some(p)) => plan = Some(p),
            Ok(None) => debug!("No subscription plan info available"),
            Err(e) => warn!("Error fetching subscription plan: {}", e),
        }

        // Fetch profile picture + name from API
        match self.get_user_profile_extras().await {
            Ok((pic, name, first, last)) => {
                picture = pic;
                api_name = name;
                api_first = first;
                api_last = last;
            }
            Err(e) => warn!("Error fetching profile extras: {}", e),
        }

        // Apply to the stored profile
        let has_updates =
            plan.is_some() || picture.is_some() || api_name.is_some() || api_first.is_some();

        if has_updates
            && let AuthState::Authenticated { profile } = self.auth_manager.state().clone()
        {
            // Merge: API-provided values take precedence over None, but
            // don't overwrite existing non-None values with None.
            let new_first = api_first.or(profile.first_name.clone());
            let new_last = api_last.or(profile.last_name.clone());
            let new_full = api_name.or(profile.full_name.clone());

            self.auth_manager.set_state(AuthState::Authenticated {
                profile: UserProfile {
                    first_name: new_first,
                    last_name: new_last,
                    full_name: new_full,
                    subscription_plan: plan.or(profile.subscription_plan.clone()),
                    picture_url: picture.or(profile.picture_url.clone()),
                    ..profile
                },
            });
        }
    }

    // =========================================================================
    // Mixes & Radio
    // =========================================================================

    /// Fetch the user's personalized mixes from the TIDAL home feed.
    ///
    /// Parses the home feed response and extracts all `MixData` items from
    /// the various list types (ShortcutList, HorizontalList, etc.).
    pub async fn get_mixes(&self) -> TidalResult<Vec<Mix>> {
        self.ensure_valid_token().await?;

        let (access_token, country_code, locale, time_offset) = {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

            let token = client
                .session
                .auth
                .access_token
                .as_ref()
                .ok_or(TidalError::NotAuthenticated)?
                .clone();

            let cc = client
                .user_info
                .as_ref()
                .map(|u| u.country_code.clone())
                .unwrap_or_else(|| "US".to_string());

            let loc = client.session.locale.clone();
            let to = client.session.time_offset.clone();

            // Note: get_mixes needs locale and time_offset which aren't in
            // AuthTokenContext, so we extract them inline here.
            (token, cc, loc, to)
        };

        debug!("Fetching home feed for mixes (raw JSON)");

        let http_client = reqwest::Client::new();
        let url = format!(
            "https://tidal.com/v2/home/feed/static?countryCode={}&locale={}&limit=20&deviceType=BROWSER&platform=WEB&timeOffset={}",
            country_code, locale, time_offset
        );

        let response = http_client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", access_token))
            .header("x-tidal-client-version", "2026.1.5")
            .header("User-Agent", "Mozilla/5.0 (Linux; Android 12; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/91.0.4472.114 Safari/537.36")
            .send()
            .await
            .map_err(|e| TidalError::NetworkError(format!("home feed request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Home feed request failed: HTTP {} — {}", status, body);
            return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
        }

        let body = response
            .text()
            .await
            .map_err(|e| TidalError::NetworkError(format!("reading home feed body: {}", e)))?;

        let feed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| TidalError::ParseError(format!("parsing home feed JSON: {}", e)))?;

        let mut mixes = Vec::new();

        // Walk through the feed items array and extract any MIX-type entries
        // from all the different list types (ShortcutList, HorizontalList, etc.)
        if let Some(items) = feed.get("items").and_then(|v| v.as_array()) {
            debug!("Home feed has {} top-level items", items.len());

            for section in items {
                let section_type = section
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("UNKNOWN");
                let section_title = section.get("title").and_then(|t| t.as_str()).unwrap_or("");

                // Gather all sub-items from this section (could be "items",
                // or "header" for HorizontalListWithContext)
                let mut sub_items: Vec<&serde_json::Value> = Vec::new();

                if let Some(arr) = section.get("items").and_then(|v| v.as_array()) {
                    sub_items.extend(arr.iter());
                }
                // HorizontalListWithContext has a "header" item too
                if let Some(header) = section.get("header") {
                    sub_items.push(header);
                }

                let mut section_mix_count = 0;
                for sub in &sub_items {
                    let item_type = sub.get("type").and_then(|t| t.as_str()).unwrap_or("");

                    if item_type == "MIX"
                        && let Some(data) = sub.get("data")
                        && let Some(mix) = Self::parse_mix_from_json(data)
                    {
                        mixes.push(mix);
                        section_mix_count += 1;
                    }
                }

                if section_mix_count > 0 {
                    debug!(
                        "Section '{}' ({}): extracted {} mixes",
                        section_title, section_type, section_mix_count
                    );
                }
            }
        }

        // Deduplicate by ID (mixes can appear in multiple sections)
        let mut seen = std::collections::HashSet::new();
        mixes.retain(|m| seen.insert(m.id.clone()));

        info!("Found {} unique mixes from home feed", mixes.len());
        Ok(mixes)
    }

    /// Parse a single Mix from a raw JSON `data` object within the home feed.
    pub fn parse_mix_from_json(data: &serde_json::Value) -> Option<Mix> {
        let id = data.get("id").and_then(|v| v.as_str())?.to_string();
        let mix_type = data
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("MIX")
            .to_string();

        let title = data
            .get("titleTextInfo")
            .and_then(|v| v.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("Mix")
            .to_string();

        let subtitle = data
            .get("shortSubtitleTextInfo")
            .and_then(|v| v.get("text"))
            .and_then(|v| v.as_str())
            .or_else(|| {
                data.get("subtitleTextInfo")
                    .and_then(|v| v.get("text"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_string();

        // Pick the largest image from mixImages
        let image_url = data
            .get("mixImages")
            .and_then(|v| v.as_array())
            .and_then(|imgs| {
                imgs.iter()
                    .filter_map(|img| {
                        let w = img.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                        let url = img.get("url").and_then(|v| v.as_str())?;
                        Some((w, url.to_string()))
                    })
                    .max_by_key(|(w, _)| *w)
                    .map(|(_, url)| url)
            });

        debug!(
            "Parsed mix from raw JSON: id={}, type={}, title={:?}",
            id, mix_type, title
        );

        Some(Mix {
            id,
            title,
            subtitle,
            mix_type,
            image_url,
        })
    }

    // =========================================================================
    // Explore (TIDAL browse pages: /v1/pages/{path})
    // =========================================================================

    /// Fetch and parse a TIDAL browse page.
    ///
    /// `path` is the page slug — `"explore"` for the root Explore view, or a
    /// sub-page slug (genre/mood/decade) obtained from a [`PageLink`].  A
    /// full `apiPath` like `pages/genre_hip_hop` is normalised to its slug.
    ///
    /// tidlers now exposes a pages API (`TidalClient::get_page`), but it
    /// deserializes into a strict `PageResponse` whose `PageModule` makes
    /// `description`, `width`, and `pagedList` non-optional — so one unexpected
    /// module (e.g. a promo banner without a paged list) would fail the whole
    /// page, unlike this defensive hand-built parse. So we keep mirroring the
    /// official web client (`GET /v1/pages/{path}?deviceType=BROWSER&...`).
    ///
    /// TODO: adopt `client.get_page(slug)` once tidlers makes those page-module
    /// fields optional (or otherwise degrades gracefully), dropping this
    /// hand-rolled request + header spoofing + slug normalisation.
    pub async fn get_explore_page(&self, path: &str) -> TidalResult<ExplorePage> {
        self.ensure_valid_token().await?;

        let (access_token, country_code, locale) = {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
            let token = client
                .session
                .auth
                .access_token
                .as_ref()
                .ok_or(TidalError::NotAuthenticated)?
                .clone();
            let cc = client
                .user_info
                .as_ref()
                .map(|u| u.country_code.clone())
                .unwrap_or_else(|| "US".to_string());
            let loc = client.session.locale.clone();
            (token, cc, loc)
        };

        // Normalise `pages/foo` / `/v1/pages/foo` down to the bare slug.
        let slug = path
            .trim_start_matches('/')
            .trim_start_matches("v1/")
            .trim_start_matches("pages/");

        let url = format!(
            "https://api.tidal.com/v1/pages/{slug}?countryCode={country_code}&locale={locale}&deviceType=BROWSER&platform=WEB"
        );
        debug!("Fetching explore page: {}", slug);

        let http_client = reqwest::Client::new();
        let response = http_client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", access_token))
            .header("x-tidal-client-version", "2026.1.5")
            .header(
                "User-Agent",
                "Mozilla/5.0 (X11; Linux x86_64; rv:150.0) Gecko/20100101 Firefox/150.0",
            )
            .send()
            .await
            .map_err(|e| TidalError::NetworkError(format!("explore request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Explore page '{}' failed: HTTP {} — {}", slug, status, body);
            return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
        }

        let body = response
            .text()
            .await
            .map_err(|e| TidalError::NetworkError(format!("reading explore body: {}", e)))?;
        let page: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| TidalError::ParseError(format!("parsing explore JSON: {}", e)))?;

        let parsed = Self::parse_explore_page(&page);
        info!("Explore '{}': {} sections", slug, parsed.sections.len());
        Ok(parsed)
    }

    /// Parse a `/v1/pages/{path}` JSON body into an [`ExplorePage`].
    ///
    /// Defensive throughout: unknown module types are skipped, missing
    /// fields fall back to sensible defaults, so a partial/changed payload
    /// degrades gracefully instead of erroring.
    fn parse_explore_page(page: &serde_json::Value) -> ExplorePage {
        let title = page
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Explore")
            .to_string();

        let mut sections: Vec<ExploreSection> = Vec::new();

        let rows = page.get("rows").and_then(|v| v.as_array());
        for row in rows.into_iter().flatten() {
            let modules = row.get("modules").and_then(|v| v.as_array());
            for module in modules.into_iter().flatten() {
                if let Some(section) = Self::parse_explore_module(module) {
                    sections.push(section);
                }
            }
        }

        ExplorePage { title, sections }
    }

    /// Parse a single module into an [`ExploreSection`], or `None` if it is
    /// empty or an unsupported type (e.g. videos).
    fn parse_explore_module(module: &serde_json::Value) -> Option<ExploreSection> {
        let module_type = module.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let title = module
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        match module_type {
            "FEATURED_PROMOTIONS" => {
                let items: Vec<ExploreCard> = module
                    .get("items")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(Self::parse_promo_card).collect())
                    .unwrap_or_default();
                (!items.is_empty()).then_some(ExploreSection::Featured { title, items })
            }
            "PAGE_LINKS" | "PAGE_LINKS_CLOUD" => {
                let links: Vec<PageLink> = module
                    .get("pagedList")
                    .and_then(|v| v.get("items"))
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(Self::parse_page_link).collect())
                    .unwrap_or_default();
                (!links.is_empty()).then_some(ExploreSection::Links { title, links })
            }
            "ALBUM_LIST" => {
                let albums: Vec<Album> = Self::paged_items(module)
                    .iter()
                    .filter_map(Self::parse_explore_album)
                    .collect();
                (!albums.is_empty()).then_some(ExploreSection::Albums { title, albums })
            }
            "PLAYLIST_LIST" => {
                let playlists: Vec<Playlist> = Self::paged_items(module)
                    .iter()
                    .filter_map(Self::parse_explore_playlist)
                    .collect();
                (!playlists.is_empty()).then_some(ExploreSection::Playlists { title, playlists })
            }
            "ARTIST_LIST" => {
                let artists: Vec<Artist> = Self::paged_items(module)
                    .iter()
                    .filter_map(Self::parse_explore_artist)
                    .collect();
                (!artists.is_empty()).then_some(ExploreSection::Artists { title, artists })
            }
            _ => None,
        }
    }

    fn paged_items(module: &serde_json::Value) -> Vec<serde_json::Value> {
        module
            .get("pagedList")
            .and_then(|v| v.get("items"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    }

    /// Parse a FEATURED_PROMOTIONS item into a card with a nav target.
    fn parse_promo_card(item: &serde_json::Value) -> Option<ExploreCard> {
        let title = item
            .get("header")
            .and_then(|v| v.as_str())
            .or_else(|| item.get("shortHeader").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        let subtitle = item
            .get("shortSubHeader")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let image_url = item
            .get("imageId")
            .and_then(|v| v.as_str())
            .map(tidal_promo_image_url);

        let artifact_id = item
            .get("artifactId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let target = match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "PLAYLIST" => ExploreTarget::Playlist(artifact_id),
            "ALBUM" => ExploreTarget::Album(artifact_id),
            "ARTIST" => ExploreTarget::Artist(artifact_id),
            "MIX" => ExploreTarget::Mix(artifact_id),
            "CATEGORY_PAGES" | "PAGE" => ExploreTarget::Page(artifact_id),
            _ => ExploreTarget::None,
        };

        // Skip promo cards we can't act on in-app: editorial / external-link
        // promos (e.g. the "TIDAL MAGAZINE" card, an EXTURL to the web magazine),
        // videos, etc. all resolve to `None`. This is a music player, not a
        // magazine reader — a card that opens nothing is just noise.
        if matches!(target, ExploreTarget::None) {
            return None;
        }

        if title.is_empty() && image_url.is_none() {
            return None;
        }
        Some(ExploreCard {
            title,
            subtitle,
            image_url,
            target,
        })
    }

    /// Parse a PAGE_LINKS item (genre/mood/decade button).
    fn parse_page_link(item: &serde_json::Value) -> Option<PageLink> {
        let text = item
            .get("title")
            .and_then(|v| v.as_str())
            .or_else(|| item.get("text").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        // The link target lives in `apiPath` (preferred) or `path`.
        let path = item
            .get("apiPath")
            .and_then(|v| v.as_str())
            .or_else(|| item.get("path").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if text.is_empty() || path.is_empty() {
            return None;
        }
        Some(PageLink { text, path })
    }

    fn parse_explore_album(it: &serde_json::Value) -> Option<Album> {
        let id = Self::json_id(it.get("id"))?;
        Some(Album {
            id,
            title: it.get("title").and_then(|v| v.as_str())?.to_string(),
            artist_name: it
                .get("artists")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            artist_id: it
                .get("artists")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|a| Self::json_id(a.get("id"))),
            num_tracks: it
                .get("numberOfTracks")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            duration: it.get("duration").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            release_date: it
                .get("releaseDate")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            cover_url: it
                .get("cover")
                .and_then(|v| v.as_str())
                .map(tidal_cover_url),
            explicit: it
                .get("explicit")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            audio_quality: it
                .get("audioQuality")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            review: None,
        })
    }

    fn parse_explore_playlist(it: &serde_json::Value) -> Option<Playlist> {
        let uuid = it.get("uuid").and_then(|v| v.as_str())?.to_string();
        let image_id = it
            .get("squareImage")
            .and_then(|v| v.as_str())
            .or_else(|| it.get("image").and_then(|v| v.as_str()));
        Some(Playlist {
            uuid,
            title: it.get("title").and_then(|v| v.as_str())?.to_string(),
            description: it
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            creator_name: None,
            num_tracks: it
                .get("numberOfTracks")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            duration: it.get("duration").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            last_updated: None,
            image_url: image_id.map(tidal_cover_url),
            is_user_playlist: false,
        })
    }

    fn parse_explore_artist(it: &serde_json::Value) -> Option<Artist> {
        let id = Self::json_id(it.get("id"))?;
        Some(Artist {
            id,
            name: it.get("name").and_then(|v| v.as_str())?.to_string(),
            picture_url: it
                .get("picture")
                .and_then(|v| v.as_str())
                .map(tidal_cover_url),
            bio: None,
            popularity: None,
            roles: Vec::new(),
            url: None,
        })
    }

    /// TIDAL ids arrive as either JSON numbers or strings; coerce to String.
    fn json_id(v: Option<&serde_json::Value>) -> Option<String> {
        match v {
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            _ => None,
        }
    }

    /// Fetch the tracks for a specific mix by its ID.
    ///
    /// Uses the TIDAL v1 API endpoint `GET /v1/mixes/{mix_id}/items` via tidlers.
    pub async fn get_mix_tracks(&self, mix_id: &str) -> TidalResult<Vec<Track>> {
        self.ensure_valid_token().await?;
        info!("Fetching tracks for mix: {}", mix_id);

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
        let response = client
            .get_mix_tracks(mix_id.to_string(), None, None)
            .await
            .map_err(|e| TidalError::RequestFailed(format!("mix tracks: {e:?}")))?;
        drop(client_guard);

        let tracks: Vec<Track> = response.items.into_iter().map(Track::from).collect();
        info!("Loaded {} tracks for mix {}", tracks.len(), mix_id);
        Ok(tracks)
    }

    // =========================================================================
    // Track Radio (delivered as a track-seeded Mix)
    // =========================================================================

    /// Fetch a track-seeded mix and its tracks.
    ///
    /// TIDAL's "track radio" is internally a Mix: `GET /v1/tracks/{id}/mix`
    /// returns a mix id (`mixType=TRACK_MIX`), whose items we then fetch
    /// via `GET /v1/mixes/{mix_id}/items`.  Both hops go through tidlers.
    ///
    /// Returns `(mix_id, tracks)`.  The mix id is what lets plays from
    /// this view report as `sourceType=MIX, sourceId=<mix_id>` — the
    /// ONLY attribution that actually surfaces track-radio listening in
    /// TIDAL's Recently Played (empirically confirmed; the older
    /// `/tracks/{id}/radio` flat-list endpoint carries no mix id, so
    /// its plays could only be reported as the dead-end `TRACK_RADIO`
    /// sourceType that TIDAL's play_log silently drops).
    pub async fn get_track_mix(&self, track_id: &str) -> TidalResult<(String, Vec<Track>)> {
        self.ensure_valid_token().await?;
        info!("Fetching track mix for track {}", track_id);

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
        let mix_response = client
            .get_track_mix(track_id, None, None)
            .await
            .map_err(|e| TidalError::RequestFailed(format!("track mix: {e:?}")))?;
        let mix_id = mix_response.id;

        let items_response = client
            .get_mix_tracks(mix_id.clone(), None, None)
            .await
            .map_err(|e| TidalError::RequestFailed(format!("track mix items: {e:?}")))?;
        drop(client_guard);

        let tracks: Vec<Track> = items_response.items.into_iter().map(Track::from).collect();
        info!(
            "Loaded track mix {} with {} tracks for seed track {}",
            mix_id,
            tracks.len(),
            track_id
        );
        Ok((mix_id, tracks))
    }

    // =========================================================================
    // Similar Artists
    // =========================================================================

    /// Fetch artists similar to the given artist from TIDAL's recommendation
    /// engine (`/v1/artists/{id}/similar`) via tidlers.
    ///
    /// Returns up to `limit` (default 20) [`Artist`] entries. Note: the
    /// `popularity` and `roles` fields are not populated because tidlers'
    /// embedded `Artist` model doesn't expose them.
    pub async fn get_similar_artists(
        &self,
        artist_id: &str,
        limit: Option<u32>,
    ) -> TidalResult<Vec<Artist>> {
        self.ensure_valid_token().await?;
        let limit_param = limit.unwrap_or(20);
        info!(
            "Fetching similar artists for artist {} (limit {})",
            artist_id, limit_param
        );

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
        let response = client
            .get_similar_artists(artist_id, Some(limit_param))
            .await
            .map_err(|e| TidalError::RequestFailed(format!("similar artists: {e:?}")))?;
        drop(client_guard);

        let artists: Vec<Artist> = response.items.into_iter().map(Artist::from).collect();
        info!(
            "Loaded {} similar artists for artist {}",
            artists.len(),
            artist_id
        );
        Ok(artists)
    }

    // =========================================================================
    // Followed Artists (Profiles)
    // =========================================================================

    /// Fetch the user's followed/favorite artists from their collection.
    ///
    /// Makes a direct HTTP request to the TIDAL v2 collection API, bypassing
    /// the tidlers `CollectionArtistsResponse` struct which requires a
    /// `lastModifiedAt` field that the API no longer always returns.
    pub async fn get_followed_artists(&self) -> TidalResult<Vec<Artist>> {
        self.ensure_valid_token().await?;

        let (access_token, country_code, locale) = {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

            let token = client
                .session
                .auth
                .access_token
                .as_ref()
                .ok_or(TidalError::NotAuthenticated)?
                .clone();

            let cc = client
                .user_info
                .as_ref()
                .map(|u| u.country_code.clone())
                .unwrap_or_else(|| "US".to_string());

            let loc = client.session.locale.clone();

            // Note: get_followed_artists needs locale which isn't in
            // AuthTokenContext, so we extract inline here.
            (token, cc, loc)
        };

        debug!("Fetching followed artists (raw JSON)");

        let http_client = reqwest::Client::new();
        let mut artists = Vec::new();
        let mut cursor: Option<String> = None;
        let page_limit = 50;

        loop {
            let mut url = format!(
                "https://api.tidal.com/v2/my-collection/artists/folders?countryCode={}&locale={}&limit={}&order=DATE&folderId=root",
                country_code, locale, page_limit
            );
            if let Some(ref c) = cursor {
                url.push_str(&format!("&cursor={}", c));
            }

            let response = http_client
                .get(&url)
                .header(AUTHORIZATION, format!("Bearer {}", access_token))
                .send()
                .await
                .map_err(|e| {
                    TidalError::NetworkError(format!("followed artists request failed: {}", e))
                })?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                error!(
                    "Followed artists request failed: HTTP {} — {}",
                    status, body
                );
                return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
            }

            let body = response.text().await.map_err(|e| {
                TidalError::NetworkError(format!("reading followed artists body: {}", e))
            })?;

            let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
                TidalError::ParseError(format!("parsing followed artists JSON: {}", e))
            })?;

            let page_count = if let Some(items) = parsed.get("items").and_then(|v| v.as_array()) {
                for item in items {
                    if let Some(data) = item.get("data") {
                        let id = data
                            .get("id")
                            .and_then(|v| {
                                v.as_u64()
                                    .map(|n| n.to_string())
                                    .or_else(|| v.as_str().map(String::from))
                            })
                            .unwrap_or_default();

                        let name = data
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        let picture_url = data
                            .get("picture")
                            .and_then(|v| v.as_str())
                            .filter(|p| !p.is_empty())
                            .map(tidal_cover_url);

                        let popularity = data
                            .get("popularity")
                            .and_then(|v| v.as_u64())
                            .map(|p| p as u32);

                        let roles: Vec<String> = data
                            .get("artistRoles")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|r| {
                                        r.get("category").and_then(|c| c.as_str()).map(String::from)
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();

                        let url = data.get("url").and_then(|v| v.as_str()).map(String::from);

                        if !id.is_empty() && !name.is_empty() {
                            artists.push(Artist {
                                id,
                                name,
                                picture_url,
                                bio: None,
                                popularity,
                                roles,
                                url,
                            });
                        }
                    }
                }
                items.len()
            } else {
                0
            };

            // Check for cursor-based pagination
            let next_cursor = parsed
                .get("cursor")
                .and_then(|v| v.as_str())
                .map(String::from);

            debug!(
                "Followed artists page: got {} items, cursor: {:?}",
                page_count, next_cursor
            );

            // Stop if we got fewer items than the limit (last page) or no cursor
            if page_count < page_limit || next_cursor.is_none() {
                break;
            }
            cursor = next_cursor;
        }

        info!("Loaded {} followed artists", artists.len());
        Ok(artists)
    }

    /// Follow (add to favorites) an artist by ID.
    ///
    /// Uses the TIDAL v1 API endpoint `PUT /v1/users/{userId}/favorites/artists`.
    pub async fn follow_artist(&self, artist_id: &str) -> TidalResult<()> {
        debug!("Following artist {}", artist_id);
        self.add_to_favorites(FavoriteResourceType::Artists, artist_id)
            .await
    }

    /// Unfollow (remove from favorites) an artist by ID.
    ///
    /// Uses the TIDAL v1 API endpoint `DELETE /v1/users/{userId}/favorites/artists/{artistId}`.
    pub async fn unfollow_artist(&self, artist_id: &str) -> TidalResult<()> {
        debug!("Unfollowing artist {}", artist_id);
        self.remove_from_favorites(FavoriteResourceType::Artists, artist_id)
            .await
    }

    /// Fetch the user's feed (new releases from followed artists).
    ///
    /// Calls `GET /v2/feed/activities` (via tidlers) and returns a list of
    /// activities sorted newest-first by `occurredAt`.
    pub async fn get_feed(&self) -> TidalResult<Vec<FeedActivity>> {
        self.ensure_valid_token().await?;
        debug!("Fetching feed activities");

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
        let raw = client
            .get_activity_feed()
            .await
            .map_err(|e| TidalError::RequestFailed(format!("feed: {e:?}")))?;
        drop(client_guard);

        let activities: Vec<FeedActivity> =
            raw.into_iter().map(Self::from_tidlers_activity).collect();

        info!("Feed: loaded {} activities", activities.len());
        Ok(activities)
    }

    /// Convert a tidlers `FeedActivity` into mare-player's `FeedActivity`.
    fn from_tidlers_activity(a: tidlers::client::models::feed::FeedActivity) -> FeedActivity {
        use tidlers::client::models::feed::FeedItem as TItem;
        let item = match a.item {
            TItem::AlbumRelease(album) => FeedItem::AlbumRelease(Album {
                id: album.id,
                title: album.title,
                artist_name: album.artist_name,
                artist_id: album.artist_id,
                num_tracks: album.num_tracks,
                duration: album.duration,
                release_date: album.release_date,
                cover_url: album.cover.as_deref().map(tidal_cover_url),
                explicit: album.explicit,
                audio_quality: album.audio_quality,
                review: None,
            }),
            TItem::HistoryMix(mix) => FeedItem::HistoryMix {
                id: mix.id,
                title: mix.title,
                subtitle: mix.subtitle,
                image_url: mix.image_url,
            },
        };
        FeedActivity {
            item,
            occurred_at: a.occurred_at,
            seen: a.seen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_items_with_a_null_album_video_still_parse() {
        // A trimmed `GET /v1/playlists/{uuid}/items` page: one regular track and
        // one music-video item whose `album` is null — the case that used to
        // fail deserialization for the whole playlist.
        let json = r#"{
            "totalNumberOfItems": 2,
            "items": [
                {
                    "item": {
                        "id": 123,
                        "title": "A Song",
                        "duration": 200,
                        "trackNumber": 1,
                        "explicit": false,
                        "audioQuality": "LOSSLESS",
                        "artist": { "id": 1, "name": "An Artist" },
                        "album": { "id": 9, "title": "An Album", "cover": "ab/cd/ef" }
                    },
                    "type": "track"
                },
                {
                    "item": {
                        "id": 456,
                        "title": "A Music Video",
                        "duration": 240,
                        "artist": { "id": 2, "name": "Another Artist" },
                        "album": null,
                        "imageId": "7bd9a4c2-424a-49cf-afd9-31f6e526a71e"
                    },
                    "type": "video"
                }
            ]
        }"#;

        let parsed: ApiPaginatedResponse<ApiItemWrapper<ApiTrackData>> =
            serde_json::from_str(json).expect("video playlist item should parse");

        let tracks: Vec<Track> = parsed
            .items
            .into_iter()
            .filter_map(|w| {
                let is_video = w.item_type.as_deref() == Some("video");
                w.item.map(|it| {
                    let mut t = Track::from(it);
                    t.is_video = is_video;
                    t
                })
            })
            .collect();

        assert_eq!(tracks.len(), 2);

        // Regular track keeps its album metadata and is not a video.
        assert_eq!(tracks[0].title, "A Song");
        assert_eq!(tracks[0].album_name.as_deref(), Some("An Album"));
        assert!(tracks[0].cover_url.is_some());
        assert!(!tracks[0].is_video);

        // Video item loads with no album, but its `imageId` provides a cover,
        // and it's flagged as a video.
        assert_eq!(tracks[1].title, "A Music Video");
        assert_eq!(tracks[1].album_name, None);
        assert_eq!(tracks[1].album_id, None);
        assert!(tracks[1].is_video);
        assert_eq!(
            tracks[1].cover_url.as_deref(),
            Some(
                "https://resources.tidal.com/images/7bd9a4c2/424a/49cf/afd9/31f6e526a71e/320x320.jpg"
            )
        );
        assert_eq!(tracks[1].artist_name, "Another Artist");
    }

    #[test]
    fn playlist_items_with_a_null_artist_video_still_parse() {
        // The real-world failure from "Classic Hip-Hop Videos" under Explore:
        // a video item whose singular `artist` is null. It must fall back to
        // the `artists` list, and an item with neither must still parse.
        let json = r#"{
            "totalNumberOfItems": 2,
            "items": [
                {
                    "item": {
                        "id": 456,
                        "title": "A Music Video",
                        "duration": 240,
                        "artist": null,
                        "artists": [{ "id": 7, "name": "Video Artist" }],
                        "album": null,
                        "imageId": "7bd9a4c2-424a-49cf-afd9-31f6e526a71e"
                    },
                    "type": "video"
                },
                {
                    "item": {
                        "id": 789,
                        "title": "An Artist-less Video",
                        "duration": 100,
                        "artist": null,
                        "album": null
                    },
                    "type": "video"
                }
            ]
        }"#;

        let parsed: ApiPaginatedResponse<ApiItemWrapper<ApiTrackData>> =
            serde_json::from_str(json).expect("null-artist video item should parse");

        let tracks: Vec<Track> = parsed
            .items
            .into_iter()
            .filter_map(|w| {
                let is_video = w.item_type.as_deref() == Some("video");
                w.item.map(|it| {
                    let mut t = Track::from(it);
                    t.is_video = is_video;
                    t
                })
            })
            .collect();

        assert_eq!(tracks.len(), 2);

        // Null `artist` falls back to the first of `artists`.
        assert_eq!(tracks[0].title, "A Music Video");
        assert_eq!(tracks[0].artist_name, "Video Artist");
        assert_eq!(tracks[0].artist_id.as_deref(), Some("7"));
        assert!(tracks[0].is_video);

        // No artist at all degrades gracefully rather than failing the parse.
        assert_eq!(tracks[1].title, "An Artist-less Video");
        assert_eq!(tracks[1].artist_name, "Unknown Artist");
        assert_eq!(tracks[1].artist_id, None);
    }
}
