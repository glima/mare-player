// SPDX-License-Identifier: GPL-3.0-only

//! TIDAL client wrapper for the COSMIC applet.
//!
//! This module wraps the `tidlers` crate and provides a high-level async API
//! for interacting with TIDAL's services including:
//! - OAuth PKCE authentication
//! - Playlist and album browsing
//! - Artist and album detail pages
//! - Track search
//! - User favorites (tracks and albums)
//! - HiRes/DASH streaming support

use super::auth::{AuthManager, AuthState, LoginRequest, StoredCredentials, UserProfile};
use super::client_identity;
use super::models::{
    Album, Artist, CreditContributor, CreditRole, ExploreCard, ExplorePage, ExploreSection, ExploreTarget, FeedActivity,
    FeedItem, Mix, PageLink, Playlist, SearchCategory, SearchResults, StreamQuality, Track, TrackCredits, TrackLyrics,
    tidal_cover_url, tidal_promo_image_url,
};
use base64::{Engine, engine::general_purpose};
use reqwest::header::AUTHORIZATION;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use tidlers::client::models::collection::album::CollectionFavoriteAlbumsResponse;
use tidlers::client::models::collection::track::CollectionFavoriteTracksResponse;
use tidlers::client::models::page::{PageModule, PageResponse};
use tidlers::client::models::playback::{AssetPresentation, PlaybackMode, VideoQuality};
use tidlers::client::models::track::config::TrackPlaybackInfoConfig;
use tidlers::client::models::video::config::VideoPlaybackInfoConfig;
use tidlers::{TidalClient, auth::TidalAuth, client::models::collection::favorites::FavoriteResourceType};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// Safety margin before token expiry to trigger refresh (5 minutes)
const TOKEN_REFRESH_MARGIN_SECS: u64 = 300;

/// Format a duration in seconds for human-readable log output.
/// Shows hours (e.g. "4.0h") when ≥ 60 min, otherwise minutes (e.g. "5min").
fn format_duration(secs: u64) -> String {
    let mins = secs / 60;
    if mins >= 60 { format!("{:.1}h", secs as f64 / 3600.0) } else { format!("{}min", mins) }
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
    /// Direct streaming URL, from a `vnd.tidal.bts` manifest.
    ///
    /// Which tiers arrive this way is the client's choice, not ours: the PKCE
    /// client we authenticate as serves DASH for both lossless tiers, leaving
    /// this for the AAC ones.
    Direct(String, Option<f32>, Option<StreamQuality>),
    /// Inline DASH manifest XML — both FLAC tiers, hi-res and lossless alike.
    /// Played through a `data:` URI so nothing is written to disk; its embedded
    /// segment URLs are absolute and carry short-lived tokens.
    DashManifest(String, Option<f32>, Option<StreamQuality>),
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
            PlaybackUrl::Direct(url, _, _) => url.clone(),
            PlaybackUrl::DashManifest(manifest, _, _) => {
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
            PlaybackUrl::Direct(_, rg, _) | PlaybackUrl::DashManifest(_, rg, _) => *rg,
        }
    }

    /// What TIDAL actually served for this stream, when the response said so.
    /// See [`StreamQuality`] for why the response is the only trustworthy
    /// source of that.
    pub fn stream_quality(&self) -> Option<StreamQuality> {
        match self {
            PlaybackUrl::Direct(_, _, q) | PlaybackUrl::DashManifest(_, _, q) => q.clone(),
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
            PlaybackUrl::Direct(url, rg, _) => {
                let base = url.split('?').next().unwrap_or(url);
                write!(f, "Direct({base}")?;
                if let Some(rg) = rg {
                    write!(f, ", {rg:+.2}dB")?;
                }
                write!(f, ")")
            }
            PlaybackUrl::DashManifest(manifest, rg, _) => {
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
    offset: i32,
    #[serde(default)]
    total_number_of_items: i32,
}

impl From<CollectionFavoriteTracksResponse> for ApiPaginatedResponse<Track> {
    fn from(page: CollectionFavoriteTracksResponse) -> Self {
        Self {
            items: page.items.into_iter().map(|entry| Track::from(entry.item)).collect(),
            offset: page.offset,
            total_number_of_items: page.total_number_of_items,
        }
    }
}

impl From<CollectionFavoriteAlbumsResponse> for ApiPaginatedResponse<Album> {
    fn from(page: CollectionFavoriteAlbumsResponse) -> Self {
        Self {
            items: page.items.into_iter().map(|entry| Album::from(entry.item)).collect(),
            offset: page.offset,
            total_number_of_items: page.total_number_of_items,
        }
    }
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
            Some(a) => (Some(a.title), Some(a.id.to_string()), a.cover.map(|c| tidal_cover_url(&c))),
            None => (None, None, t.image_id.map(|id| tidal_cover_url(&id))),
        };
        // Primary artist: the singular `artist`, falling back to the first of
        // the `artists` list when it's null (e.g. video items in playlists).
        let (artist_name, artist_id) = match t.artist.or_else(|| t.artists.into_iter().next()) {
            Some(a) => (a.name.unwrap_or_else(|| "Unknown Artist".to_string()), Some(a.id.to_string())),
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

// ── Credentials for endpoints requiring direct requests ─────────────────

/// Access token + country code (no user ID needed).
struct AuthTokenContext {
    access_token: String,
    country_code: String,
}

pub type TidalResult<T> = Result<T, TidalError>;

/// A playback-resolution failure. Only confirmed unavailable assets may be
/// skipped automatically; authentication and transport failures stop playback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackFailure {
    /// TIDAL reports `subStatus: 4005` (asset not ready for playback).
    Unavailable,
    /// The server returned 401 without a usable application error code.
    Rejected,
    /// Anything else, with the message to show.
    Failed(String),
}

impl std::fmt::Display for PlaybackFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaybackFailure::Unavailable => write!(f, "This item is not available for playback on TIDAL"),
            PlaybackFailure::Rejected => {
                write!(f, "TIDAL refused playback (HTTP 401). The item may be unavailable, or your session may need renewing")
            }
            PlaybackFailure::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

/// Interpret a playbackinfo error without inferring availability from HTTP
/// status alone. TIDAL uses 401 for both authentication and asset failures;
/// local token-expiry bookkeeping cannot distinguish them.
pub(crate) fn classify_playback_error(error: &tidlers::error::TidalError) -> PlaybackFailure {
    use tidlers::error::TidalError as ApiError;
    use tidlers::requests::RequestClientError;

    match error {
        ApiError::RequestClient(RequestClientError::Unauthorized) => PlaybackFailure::Rejected,
        ApiError::RequestClient(RequestClientError::StatusCode { status, body_snippet, .. }) => {
            #[derive(Deserialize)]
            struct ApiPlaybackError {
                #[serde(rename = "subStatus")]
                sub_status: u32,
            }
            if *status == reqwest::StatusCode::UNAUTHORIZED {
                if serde_json::from_str::<ApiPlaybackError>(body_snippet).is_ok_and(|body| body.sub_status == 4005) {
                    PlaybackFailure::Unavailable
                } else {
                    PlaybackFailure::Rejected
                }
            } else {
                // StatusCode's Display includes the request URL and body, which
                // can contain credentials. Neither belongs in the UI or journal.
                PlaybackFailure::Failed(format!("TIDAL playback request failed (HTTP {})", status.as_u16()))
            }
        }
        other => PlaybackFailure::Failed(other.to_string()),
    }
}

impl From<TidalError> for PlaybackFailure {
    fn from(error: TidalError) -> Self {
        match error {
            TidalError::Playback(failure) => failure,
            other => Self::Failed(other.to_string()),
        }
    }
}

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
    /// A track or video could not be played. See [`PlaybackFailure`].
    Playback(PlaybackFailure),
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
            TidalError::Playback(failure) => write!(f, "{failure}"),
        }
    }
}

impl std::error::Error for TidalError {}

/// TIDAL's authorize endpoint, which the PKCE flow starts at.
const AUTHORIZE_URL: &str = "https://login.tidal.com/authorize";

/// The `appMode` to ask the login page to render as.
///
/// This decides which sign-in methods the page offers. `web` shows email,
/// *Continue with Google* and *Continue with Apple* — for a linked account,
/// one click against a session the browser already has. `android`, which
/// tidlers sends, shows email alone: an address, then a code mailed to it.
///
/// Nothing else about the flow changes; the token exchange never sees it.
const LOGIN_APP_MODE: &str = "web";

/// Build the URL that starts the sign-in, from the PKCE parameters tidlers
/// generated.
///
/// Built here rather than by tidlers' `initiate_pkce_login` so the login page
/// is ours to choose (see [`LOGIN_APP_MODE`]). Everything the token exchange
/// later verifies — client id, redirect URI, challenge, unique key — is taken
/// straight from the same `PkceConfig` tidlers will exchange with, keeping the
/// two halves in step.
fn authorize_url(pkce: &tidlers::auth::pkce::PkceConfig) -> Result<String, serde_urlencoded::ser::Error> {
    let query = serde_urlencoded::to_string([
        ("response_type", "code"),
        ("redirect_uri", pkce.redirect_uri.as_str()),
        ("client_id", pkce.client_id.as_str()),
        ("lang", "EN"),
        ("appMode", LOGIN_APP_MODE),
        ("client_unique_key", pkce.client_unique_key.as_str()),
        ("code_challenge", pkce.code_challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("restrict_signup", "true"),
    ])?;
    Ok(format!("{AUTHORIZE_URL}?{query}"))
}

/// The authorize URL built from a throwaway PKCE config, for tests.
///
/// Exists so the choice of login page — the difference between "type the code
/// we emailed you" and "Continue with Google" — is covered without a login.
#[doc(hidden)]
pub fn authorize_url_for_test() -> String {
    let auth = TidalAuth::with_pkce();
    authorize_url(&auth.pkce_config).unwrap_or_default()
}

/// High-level TIDAL client for the COSMIC applet
pub struct TidalAppClient {
    /// The underlying tidlers client (if authenticated)
    /// Wrapped in `Arc<Mutex>` to allow token refresh during API calls
    client: Arc<Mutex<Option<TidalClient>>>,
    /// Authentication manager
    auth_manager: AuthManager,
    /// Current audio quality setting
    audio_quality: crate::config::AudioQuality,
    /// Track ids we've already logged a lower-than-requested tier for.
    ///
    /// Which tiers a track exists in is a property of the recording, not an
    /// event: most catalogue is 16-bit/44.1 kHz and simply has no hi-res master
    /// to serve. Every track also resolves twice (once to play, once to preload
    /// for gapless), so the note is logged once per track id rather than once
    /// per resolution.
    warned_downgrades: Arc<std::sync::Mutex<HashSet<String>>>,
}

impl Default for TidalAppClient {
    fn default() -> Self {
        Self::new()
    }
}

/// The signed-in account's profile, as far as the session's user info has it.
/// The picture and subscription plan are fetched separately.
///
/// This is personal data (name, email): show it in the UI, never log it.
fn user_profile(client: &TidalClient) -> UserProfile {
    match &client.user_info {
        Some(u) => UserProfile {
            username: Some(u.username.clone()),
            first_name: u.first_name.clone(),
            last_name: u.last_name.clone(),
            full_name: u.full_name.clone(),
            nickname: u.nickname.clone(),
            email: Some(u.email.clone()),
            picture_url: None,
            subscription_plan: None,
        },
        None => UserProfile::default(),
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

        let country_code = client.user_info.as_ref().map(|u| u.country_code.clone()).unwrap_or_else(|| "US".to_string());

        Ok(AuthTokenContext { access_token, country_code })
    }

    /// Add `resource_id` to the user's favorites via tidlers.
    async fn add_to_favorites(&self, resource: FavoriteResourceType, resource_id: &str) -> TidalResult<()> {
        self.ensure_valid_token().await?;
        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
        client.add_to_favorites(resource, resource_id).await.map_err(|e| TidalError::RequestFailed(format!("{e:?}")))
    }

    /// Remove `resource_id` from the user's favorites via tidlers.
    async fn remove_from_favorites(&self, resource: FavoriteResourceType, resource_id: &str) -> TidalResult<()> {
        self.ensure_valid_token().await?;
        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
        client.remove_from_favorites(resource, resource_id).await.map_err(|e| TidalError::RequestFailed(format!("{e:?}")))
    }

    /// Create a new TidalAppClient
    pub fn new() -> Self {
        Self {
            client: Arc::new(Mutex::new(None)),
            auth_manager: AuthManager::new(),
            audio_quality: crate::config::AudioQuality::High,
            warned_downgrades: Arc::new(std::sync::Mutex::new(HashSet::new())),
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
    pub async fn set_audio_quality(&mut self, quality: crate::config::AudioQuality) {
        info!("Setting audio quality to: {:?}", quality);
        self.audio_quality = quality;
        // A different tier may or may not be entitled, so let the downgrade
        // warning speak once more per track under the new setting.
        if let Ok(mut seen) = self.warned_downgrades.lock() {
            seen.clear();
        }
        let mut client_guard = self.client.lock().await;
        if let Some(client) = client_guard.as_mut() {
            client.set_audio_quality(quality.to_tidlers());
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
                        if let (Some(expiry), Some(last_refresh)) =
                            (client.session.auth.refresh_expiry, client.session.auth.last_refresh_time)
                        {
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
        if let (Some(expiry), Some(last_refresh)) = (client.session.auth.refresh_expiry, client.session.auth.last_refresh_time) {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);

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
                // Sessions minted by the old device-code flow are capped at
                // LOSSLESS whatever the account is entitled to, and no request
                // parameter lifts that — so there is nothing worth restoring
                // them for. Drop it and send the user through PKCE once.
                if !client.session.auth.pkce_login {
                    info!(
                        "Stored session predates PKCE login (client id {}); discarding it so the user can sign in for hi-res",
                        client.session.auth.client_id
                    );
                    let _ = AuthManager::delete_credentials();
                    self.auth_manager.set_state(AuthState::NotAuthenticated);
                    return Ok(false);
                }
                client_identity::verify(&client.session.auth.pkce_config.client_id);

                // Log current token state
                if let (Some(expiry), Some(last_refresh)) =
                    (client.session.auth.refresh_expiry, client.session.auth.last_refresh_time)
                {
                    let now =
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
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
                                warn!("Network error on token refresh (attempt {}/4), retrying in {}s: {}", attempt, delay, e);
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

                        let profile = user_profile(&client);

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
                        if let (Some(expiry), Some(last_refresh)) =
                            (client.session.auth.refresh_expiry, client.session.auth.last_refresh_time)
                        {
                            info!("Token valid for {}s (~{}) from last refresh", expiry, format_duration(expiry),);
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            let remaining = (last_refresh + expiry).saturating_sub(now);
                            info!("Token will expire in {}s (~{})", remaining, format_duration(remaining),);
                        }

                        client.set_audio_quality(self.audio_quality.to_tidlers());
                        *self.client.lock().await = Some(client);
                        self.auth_manager.set_state(AuthState::Authenticated { profile });

                        // Fetch subscription plan + profile picture (best-effort)
                        self.fetch_and_set_profile_extras().await;

                        Ok(true)
                    }
                    Err(e) => {
                        if is_tidlers_network_error(&e) {
                            // Network errors are transient — keep credentials for next attempt
                            warn!("Token refresh failed after retries (network error, credentials preserved): {}", e);
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
                Err(TidalError::CredentialError(format!("Invalid stored session: {:?}", e)))
            }
        }
    }

    /// Start the OAuth **PKCE** login flow.
    ///
    /// Returns the TIDAL authorize URL the user has to open; the login is
    /// finished by handing the browser's redirect URL to
    /// [`Self::complete_login`]. Which client we authenticate as decides the
    /// stream ceiling independently of the subscription, and PKCE is the only
    /// flow whose client is granted the hi-res tier — see
    /// [`super::client_identity`] for the measurements.
    ///
    /// The half-finished client is parked in `self.client` because it holds the
    /// PKCE code verifier that the redirect's `code` is exchanged against.
    pub async fn start_login(&mut self) -> TidalResult<LoginRequest> {
        info!("Starting OAuth PKCE login flow");

        let mut auth = TidalAuth::with_pkce();
        // tidlers defaults to the https redirect, which only the browser can
        // receive. When this desktop routes `tidal://` to us, ask for that one
        // instead and the code comes home by itself — see `login_uri`.
        let redirect_uri = super::login_uri::redirect_uri();
        auth.pkce_config.redirect_uri = redirect_uri.to_string();

        let client = TidalClient::new(&auth);
        client_identity::verify(&client.session.auth.pkce_config.client_id);

        match authorize_url(&client.session.auth.pkce_config) {
            Ok(authorize_url) => {
                self.auth_manager.set_state(AuthState::AwaitingUserAuth { authorize_url: authorize_url.clone() });

                // Park the client (and with it the code verifier) for `complete_login`.
                *self.client.lock().await = Some(client);

                info!(redirect_uri, "PKCE login started, awaiting user authorization");
                Ok(LoginRequest { authorize_url, delivers_itself: redirect_uri == super::login_uri::CALLBACK_REDIRECT_URI })
            }
            Err(e) => {
                error!("Failed to build the PKCE authorize URL: {:?}", e);
                self.auth_manager.set_state(AuthState::Failed(format!("{:?}", e)));
                Err(TidalError::AuthenticationFailed(format!("{:?}", e)))
            }
        }
    }

    /// Finish the PKCE login with the URL the browser was redirected to.
    ///
    /// `redirect_url` is the full `https://tidal.com/android/login/auth?code=…`
    /// address the user pasted back; only its `code` parameter is used.
    pub async fn complete_login(&mut self, redirect_url: &str) -> TidalResult<()> {
        info!("Completing PKCE login from the pasted redirect URL");

        let mut client_guard = self.client.lock().await;
        let client = client_guard.as_mut().ok_or_else(|| {
            error!("complete_login called without a login in progress!");
            TidalError::NotAuthenticated
        })?;

        match client.finish_pkce_login(redirect_url.trim()).await {
            Ok(()) => {
                info!("PKCE authorization completed successfully!");
                client_identity::verify(&client.session.auth.pkce_config.client_id);
                debug!("Signed in as TIDAL client {:?}", client.session.auth.client_name);

                // Log token expiry info
                if let (Some(expiry), Some(last_refresh)) =
                    (client.session.auth.refresh_expiry, client.session.auth.last_refresh_time)
                {
                    info!("New OAuth token received - expires_in: {}s (~{})", expiry, format_duration(expiry),);
                    let now =
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                    let remaining = (last_refresh + expiry).saturating_sub(now);
                    info!("Token will expire in {}s (~{})", remaining, format_duration(remaining),);
                }

                // Refresh user info
                if let Err(e) = client.refresh_user_info().await {
                    warn!("Failed to refresh user info: {:?}", e);
                }

                let username = client.user_info.as_ref().map(|u| u.username.clone());
                let user_id = client.user_info.as_ref().map(|u| u.user_id.to_string());

                let profile = user_profile(client);

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

                client.set_audio_quality(self.audio_quality.to_tidlers());
                // Drop the lock before calling fetch_and_set_subscription_plan
                // which needs &mut self (and internally re-acquires the lock).
                drop(client_guard);

                self.auth_manager.set_state(AuthState::Authenticated { profile });

                // Fetch subscription plan + profile picture (best-effort)
                self.fetch_and_set_profile_extras().await;

                Ok(())
            }
            Err(e) => {
                // Keep the client: it holds the code verifier, and the authorize
                // URL we already handed the user stays valid. A failed exchange
                // is nearly always a code that was spent or has expired (they
                // are single-use and short-lived), so the way out is another
                // trip through the browser with the *same* URL.
                error!("PKCE authorization failed with error: {:?}", e);
                self.auth_manager.set_state(AuthState::Failed(format!("{:?}", e)));
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

    /// Search `categories`, returning up to `limit` results per category
    /// starting at `offset`, plus TIDAL's total match count for each.
    pub async fn search(
        &self,
        query: &str,
        categories: &[SearchCategory],
        limit: u32,
        offset: u32,
    ) -> TidalResult<SearchResults> {
        // Ensure token is valid before the operation
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Searching for: {} ({:?}, offset {})", query, categories, offset);

        use tidlers::client::models::search::config::{SearchConfig, SearchType};

        let config = SearchConfig {
            query: query.to_string(),
            types: categories
                .iter()
                .map(|c| match c {
                    SearchCategory::Tracks => SearchType::Tracks,
                    SearchCategory::Artists => SearchType::Artists,
                    SearchCategory::Albums => SearchType::Albums,
                    SearchCategory::Playlists => SearchType::Playlists,
                    SearchCategory::Videos => SearchType::Videos,
                })
                .collect(),
            limit,
            offset,
            ..Default::default()
        };

        match client.search(config).await {
            Ok(results) => {
                let mut search_results = SearchResults::default();

                // Convert tracks from SearchTrackHit
                if let Some(tracks) = results.tracks {
                    search_results.totals.tracks = tracks.total_number_of_items;
                    search_results.tracks = tracks.items.into_iter().map(Track::from).collect();
                }

                // Convert albums from SearchAlbumHit
                if let Some(albums) = results.albums {
                    search_results.totals.albums = albums.total_number_of_items;
                    search_results.albums = albums.items.into_iter().map(Album::from).collect();
                }

                // Convert artists from SearchArtistHit
                if let Some(artists) = results.artists {
                    search_results.totals.artists = artists.total_number_of_items;
                    search_results.artists = artists.items.into_iter().map(Artist::from).collect();
                }

                // Convert playlists from SearchPlaylistHit
                if let Some(playlists) = results.playlists {
                    search_results.totals.playlists = playlists.total_number_of_items;
                    search_results.playlists = playlists.items.into_iter().map(Playlist::from).collect();
                }

                // Convert videos into playable tracks (is_video = true). Videos
                // have no album; their thumbnail is the `image` UUID, mirroring
                // how playlist/Explore video items get their cover.
                if let Some(videos) = results.videos {
                    search_results.totals.videos = videos.total_number_of_items;
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
                                artist_name: artist.and_then(|a| a.name.clone()).unwrap_or_else(|| "Unknown Artist".to_string()),
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

    pub async fn get_user_playlists(&self, _limit: Option<u32>, _offset: Option<u32>) -> TidalResult<Vec<Playlist>> {
        // Ensure token is valid before the operation
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting user playlists");

        match client.list_playlists().await {
            Ok(response) => {
                let playlists: Vec<Playlist> = response.items.into_iter().map(Playlist::from).collect();
                Ok(playlists)
            }
            Err(e) => {
                error!("Failed to get playlists: {:?}", e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    /// Get all favourite tracks through tidlers, preserving their server order.
    /// `_limit` is not a cap on the collection; requests use 100-item pages.
    pub async fn get_user_favorite_tracks(&self, _limit: Option<u32>) -> TidalResult<Vec<Track>> {
        self.ensure_valid_token().await?;
        debug!("Getting user favorite tracks (paginated)");
        Self::collect_favorites("Favorite tracks", |limit, offset| async move {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(tidlers::error::TidalError::NotAuthenticated)?;
            client.get_collection_track_favorites(Some(limit), Some(offset)).await.map(ApiPaginatedResponse::from)
        })
        .await
    }

    /// Walk the SDK's pages until exhausted. Offsets count returned entries,
    /// not the requested page size, so short intermediate pages are not skipped.
    async fn collect_favorites<T, F, Fut>(context: &str, mut fetch_page: F) -> TidalResult<Vec<T>>
    where
        F: FnMut(u32, u32) -> Fut,
        Fut: std::future::Future<Output = Result<ApiPaginatedResponse<T>, tidlers::error::TidalError>>,
    {
        let mut offset = 0u32;
        let mut items = Vec::new();
        loop {
            let page = fetch_page(100, offset).await.map_err(|e| Self::request_error(context, e))?;
            let total = u32::try_from(page.total_number_of_items)
                .map_err(|_| TidalError::ParseError(format!("{context} returned a negative total")))?;
            if u32::try_from(page.offset).ok() != Some(offset) {
                return Err(TidalError::ParseError(format!("{context} returned an unexpected page offset")));
            }
            let count =
                u32::try_from(page.items.len()).map_err(|_| TidalError::ParseError(format!("{context} page is too large")))?;
            items.extend(page.items);
            offset = offset.checked_add(count).ok_or_else(|| TidalError::ParseError(format!("{context} pagination overflow")))?;
            info!("Fetched {context} page: {} / {} total", items.len(), total);
            if count == 0 || offset >= total {
                return Ok(items);
            }
        }
    }

    /// Get all favourite albums through tidlers without per-album detail requests.
    /// `_limit` is not a cap on the collection; requests use 100-item pages.
    pub async fn get_user_favorite_albums(&self, _limit: Option<u32>) -> TidalResult<Vec<Album>> {
        self.ensure_valid_token().await?;
        debug!("Getting user favorite albums (paginated)");
        Self::collect_favorites("Favorite albums", |limit, offset| async move {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(tidlers::error::TidalError::NotAuthenticated)?;
            client.get_collection_album_favorites(Some(limit), Some(offset)).await.map(ApiPaginatedResponse::from)
        })
        .await
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

        let ctx = self.auth_context().await?;
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
                .map_err(|e| TidalError::NetworkError(format!("playlist items request failed: {}", e)))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                error!("Playlist items request failed: {} - {}", status, body);
                return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
            }

            let body =
                response.text().await.map_err(|e| TidalError::NetworkError(format!("reading playlist items body: {}", e)))?;

            let parsed: ApiPaginatedResponse<ApiItemWrapper<ApiTrackData>> =
                serde_json::from_str(&body).map_err(|e| TidalError::ParseError(format!("playlist items JSON: {}", e)))?;

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
            info!("Fetched playlist tracks page: {} / {} total", all_tracks.len(), total);

            if page_items == 0 || offset >= total {
                break;
            }
        }

        Ok(all_tracks)
    }

    /// Resolve the playable HLS (`.m3u8`) URL for a music **video**.
    ///
    /// TIDAL videos are DRM-free HLS: the playbackinfo endpoint returns a
    /// base64 "EMU" manifest that wraps the HLS master URL, which tidlers
    /// decodes into [`EmuVideoManifest`]. (Verified the inner HLS carries no
    /// `EXT-X-KEY`/Widevine, so no CDM is needed.)
    pub async fn get_video_hls_url(&self, video_id: &str) -> TidalResult<String> {
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Fetching video playback info for: {}", video_id);

        let config = VideoPlaybackInfoConfig {
            video_quality: Some(VideoQuality::High),
            playback_mode: Some(PlaybackMode::Stream),
            asset_presentation: Some(AssetPresentation::Full),
        };

        let info = client
            .get_video_postpaywall_playback_info(video_id, Some(config))
            .await
            .map_err(|e| TidalError::Playback(classify_playback_error(&e)))?;

        info.manifest
            .and_then(|manifest| manifest.urls.into_iter().next())
            .ok_or_else(|| TidalError::ParseError("video manifest contained no URLs".to_string()))
    }

    /// Get album tracks
    pub async fn get_album_tracks(&self, album_id: &str, limit: Option<u32>, offset: Option<u32>) -> TidalResult<Vec<Track>> {
        // Ensure token is valid before the operation
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting album tracks for: {}", album_id);

        match client.get_album_items(album_id.to_string(), Some(limit.unwrap_or(100)), offset).await {
            Ok(response) => {
                let tracks: Vec<Track> = response.items.into_iter().map(|item| Track::from(item.item)).collect();
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
    /// For HiRes quality, TIDAL returns DASH manifests, which are handed to
    /// GStreamer inline. For Low/High/Lossless quality, returns a direct
    /// streaming URL.
    pub async fn get_track_playback_url(&self, track_id: &str) -> TidalResult<PlaybackUrl> {
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        info!("Getting playback URL for track: {} with quality: {:?}", track_id, self.audio_quality);

        let config = TrackPlaybackInfoConfig {
            audio_quality: Some(self.audio_quality.tidlers_quality()),
            playback_mode: Some(PlaybackMode::Stream),
            asset_presentation: Some(AssetPresentation::Full),
        };

        let info = client
            .get_track_postpaywall_playback_info(track_id, Some(config))
            .await
            .map_err(|e| TidalError::Playback(classify_playback_error(&e)))?;

        // What TIDAL *actually served*, which is not necessarily what we asked
        // for — the backend answers an out-of-reach tier with a lower one
        // instead of erroring. The only trustworthy source for the badge the
        // now-playing bar shows; see `StreamQuality` for why.
        let stream_quality = (!info.audio_quality.is_empty()).then(|| StreamQuality {
            quality: info.audio_quality.clone(),
            sample_rate: info.sample_rate,
            bit_depth: info.bit_depth,
        });

        if let Some(served) = &stream_quality {
            let requested = self.audio_quality.tidal_param();
            if served.quality != requested {
                // Once per track id — see `warned_downgrades`.
                let first_time = self.warned_downgrades.lock().map(|mut seen| seen.insert(track_id.to_string())).unwrap_or(true);
                if first_time {
                    info!(
                        "TIDAL served {} for a {} request — this track isn't available in the requested tier",
                        served.quality, requested
                    );
                }
            }
        }

        let replay_gain_db = Some(info.album_replay_gain as f32);

        info!(
            "Playback info received - audio_quality: {}, audio_mode: {}, manifest_mime_type: {}, sample_rate: {:?}, bit_depth: {:?}, replay_gain: {:?} dB, peak: {}",
            info.audio_quality,
            info.audio_mode,
            info.manifest_mime_type,
            info.sample_rate,
            info.bit_depth,
            replay_gain_db,
            info.album_peak_amplitude
        );

        // DASH (both FLAC tiers): the manifest goes to GStreamer verbatim, so
        // the raw XML is what matters, not tidlers' parse of it.
        if info.manifest_mime_type.contains("dash") {
            let manifest =
                info.manifest_raw.ok_or_else(|| TidalError::ParseError("DASH response carried no manifest".to_string()))?;
            info!("DASH manifest detected - playing inline");
            debug!("DASH manifest content:\n{}", manifest.chars().take(500).collect::<String>());
            return Ok(PlaybackUrl::DashManifest(manifest, replay_gain_db, stream_quality));
        }

        let url = info.get_primary_url().ok_or_else(|| TidalError::RequestFailed("No playback URL available".to_string()))?;
        info!("Got direct playback URL");
        Ok(PlaybackUrl::Direct(url, replay_gain_db, stream_quality))
    }

    /// Add a track to user's favorites
    pub async fn add_favorite_track(&self, track_id: &str) -> TidalResult<()> {
        debug!("Adding track {} to favorites", track_id);
        self.add_to_favorites(FavoriteResourceType::Tracks, track_id).await
    }

    /// Remove a track from user's favorites
    pub async fn remove_favorite_track(&self, track_id: &str) -> TidalResult<()> {
        debug!("Removing track {} from favorites", track_id);
        self.remove_from_favorites(FavoriteResourceType::Tracks, track_id).await
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
                if response.summary.is_empty() { Ok(response.text) } else { Ok(response.summary) }
            }
            Err(e) => {
                debug!("No bio available for artist {}: {:?}", artist_id, e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    /// Get an artist's top tracks
    pub async fn get_artist_top_tracks(&self, artist_id: &str, limit: Option<u32>) -> TidalResult<Vec<Track>> {
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting top tracks for artist: {}", artist_id);

        match client.get_artist_tracks(artist_id.to_string(), limit, None).await {
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
    pub async fn get_artist_albums(&self, artist_id: &str, limit: Option<u32>) -> TidalResult<Vec<Album>> {
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting albums for artist: {}", artist_id);

        match client.get_artist_albums(artist_id.to_string(), limit, None).await {
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
    pub async fn get_artist_videos(&self, artist_id: &str, limit: Option<u32>) -> TidalResult<Vec<Track>> {
        self.ensure_valid_token().await?;

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        debug!("Getting videos for artist: {}", artist_id);

        match client.get_artist_videos(artist_id.to_string(), limit, None).await {
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
            return Err(TidalError::RequestFailed("Review text is empty".to_string()));
        }

        Ok(review.text)
    }

    // =========================================================================
    // Track Lyrics
    // =========================================================================

    /// Fetch lyrics through tidlers' authenticated v1 endpoint.
    ///
    /// Plain lyrics and timed subtitles are independent representations. A
    /// missing lyrics resource is an empty result; authentication, transport
    /// and malformed-response failures remain errors.
    pub async fn get_track_lyrics(&self, track_id: &str) -> TidalResult<TrackLyrics> {
        self.ensure_valid_token().await?;
        debug!("Fetching lyrics for track {}", track_id);
        let result = {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
            client.get_track_lyrics(track_id).await
        };
        let lyrics = Self::lyrics_from_response(result)?;
        info!(
            "Loaded lyrics for track {}: provider={:?} plain={} synced_lines={}",
            track_id,
            lyrics.provider,
            lyrics.plain_text.is_some(),
            lyrics.lrc_lines.len()
        );
        Ok(lyrics)
    }

    /// Preserve the no-lyrics/404 contract without masking other failures.
    fn lyrics_from_response(
        result: Result<tidlers::client::models::track::LyricsResponse, tidlers::error::TidalError>,
    ) -> TidalResult<TrackLyrics> {
        match result {
            Ok(response) => Ok(TrackLyrics::from(response)),
            Err(tidlers::error::TidalError::NotFound) => Ok(TrackLyrics::default()),
            Err(error) => Err(Self::request_error("Lyrics", error)),
        }
    }

    // =========================================================================
    // Track Credits
    // =========================================================================

    /// Fetch the credits (per-role contributors) for a track, plus the catalog
    /// extras TIDAL's own credits panel shows alongside them.
    ///
    /// Two legs, issued concurrently:
    ///
    /// * **Roles** — tidlers' `get_track_credits`
    ///   (`GET /v1/tracks/{id}/credits`, `includeContributors=true`), which
    ///   yields `{ type, contributors: [{ name, id }] }` groups. A track TIDAL
    ///   doesn't know surfaces as [`tidlers::error::TidalError::NotFound`],
    ///   which is "no credits" rather than a failure — the same contract as
    ///   the lyrics endpoint.
    /// * **Catalog extras** — a raw `GET /v1/tracks/{id}` for copyright/label,
    ///   stream start date, ISRC and BPM. None of those appear in the credits
    ///   payload, and tidlers' typed `Track` doesn't carry them either, so this
    ///   leg stays hand-rolled. Best-effort: if it fails we still return the
    ///   roles.
    ///
    /// The result is cached by the caller under `credits:{track_id}`, so
    /// re-opening the view paints instantly.
    pub async fn get_track_credits(&self, track_id: &str) -> TidalResult<TrackCredits> {
        self.ensure_valid_token().await?;

        debug!("Fetching credits for track {}", track_id);

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;

        // Two legs, run together: the roles, and the catalog extras the
        // credits endpoint doesn't carry (copyright, ISRC, BPM, release date).
        let credits_req = client.get_track_credits(track_id.to_string(), true);
        let meta_req = client.get_track(track_id);

        let (credits_res, meta_res) = tokio::join!(credits_req, meta_req);

        // ── Roles (required leg) ──────────────────────────────────────────
        let raw = match credits_res {
            Ok(raw) => raw,
            Err(tidlers::error::TidalError::NotFound) => {
                debug!("No credits found for track {}", track_id);
                return Ok(TrackCredits::default());
            }
            Err(e) => return Err(TidalError::RequestFailed(format!("track credits: {e:?}"))),
        };

        let roles: Vec<CreditRole> = raw
            .into_iter()
            .filter(|c| !c.credit_type.trim().is_empty())
            .map(|c| CreditRole {
                role: c.credit_type,
                contributors: c
                    .contributors
                    .into_iter()
                    .filter(|p| !p.name.trim().is_empty())
                    .map(|p| CreditContributor { id: p.id.map(|id| id.to_string()), name: p.name })
                    .collect(),
            })
            .filter(|r| !r.contributors.is_empty())
            .collect();

        // ── Catalog extras (best-effort leg) ──────────────────────────────
        // Missing extras cost a line in the view; they never fail the credits.
        let meta = match meta_res {
            Ok(track) => Some(track),
            Err(e) => {
                debug!("Track metadata fetch failed for {}: {:?}", track_id, e);
                None
            }
        };

        /// Trim a value and drop it when empty, so the view can rely on
        /// `Some(_)` meaning "has something to show".
        fn non_empty(value: Option<String>) -> Option<String> {
            value.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        }

        let (copyright, released, isrc, bpm) = match meta {
            Some(m) => (
                non_empty(m.copyright),
                // `2014-10-27T00:00:00.000+0000` → `2014-10-27`
                non_empty(m.stream_start_date).map(|d| d.split('T').next().unwrap_or(&d).to_string()),
                non_empty(m.isrc),
                // tidlers types BPM as f32; the view shows a whole number.
                m.bpm.filter(|b| *b > 0.0).map(|b| b.round() as u32),
            ),
            None => (None, None, None, None),
        };

        info!(
            "Loaded credits for track {}: roles={} label={} isrc={}",
            track_id,
            roles.len(),
            copyright.is_some(),
            isrc.is_some()
        );

        Ok(TrackCredits { roles, copyright, released, isrc, bpm })
    }

    // =========================================================================
    // Album Favorites
    // =========================================================================

    /// Add an album to user's favorites
    pub async fn add_favorite_album(&self, album_id: &str) -> TidalResult<()> {
        debug!("Adding album {} to favorites", album_id);
        self.add_to_favorites(FavoriteResourceType::Albums, album_id).await
    }

    /// Remove an album from user's favorites
    pub async fn remove_favorite_album(&self, album_id: &str) -> TidalResult<()> {
        debug!("Removing album {} from favorites", album_id);
        self.remove_from_favorites(FavoriteResourceType::Albums, album_id).await
    }
    /// Fetch the user's subscription plan.
    ///
    /// Asks tidlers, which wraps the v1 endpoint.
    ///
    /// Returns a human-readable label such as "HiFi Plus", "HiFi", or "Free".
    /// On any failure the method returns `Ok(None)` so callers can treat the
    /// plan badge as optional.
    async fn get_user_subscription(&self) -> TidalResult<Option<String>> {
        self.ensure_valid_token().await?;

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
                    warn!("subscription lookup failed ({e}); the plan badge stays hidden");
                }
            }
        } // client_guard dropped

        Ok(None)
    }

    /// Derive a human-readable plan label from `subscription.type` and
    /// `highestSoundQuality` (used when we have tidlers' typed response).
    pub fn derive_plan_label_from_type_and_quality(sub_type: &str, highest_quality: &str) -> Option<String> {
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
    async fn get_user_profile_extras(&self) -> TidalResult<(Option<String>, Option<String>, Option<String>, Option<String>)> {
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
        for field in &["profilePicture", "picture", "pictureUrl", "profilePictureUrl"] {
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
        let has_updates = plan.is_some() || picture.is_some() || api_name.is_some() || api_first.is_some();

        if has_updates && let AuthState::Authenticated { profile } = self.auth_manager.state().clone() {
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

            let token = client.session.auth.access_token.as_ref().ok_or(TidalError::NotAuthenticated)?.clone();

            let cc = client.user_info.as_ref().map(|u| u.country_code.clone()).unwrap_or_else(|| "US".to_string());

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

        let body = response.text().await.map_err(|e| TidalError::NetworkError(format!("reading home feed body: {}", e)))?;

        let feed: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| TidalError::ParseError(format!("parsing home feed JSON: {}", e)))?;

        let mut mixes = Vec::new();

        // Walk through the feed items array and extract any MIX-type entries
        // from all the different list types (ShortcutList, HorizontalList, etc.)
        if let Some(items) = feed.get("items").and_then(|v| v.as_array()) {
            debug!("Home feed has {} top-level items", items.len());

            for section in items {
                let section_type = section.get("type").and_then(|t| t.as_str()).unwrap_or("UNKNOWN");
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
                    debug!("Section '{}' ({}): extracted {} mixes", section_title, section_type, section_mix_count);
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
        let mix_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("MIX").to_string();

        let title = data.get("titleTextInfo").and_then(|v| v.get("text")).and_then(|v| v.as_str()).unwrap_or("Mix").to_string();

        let subtitle = data
            .get("shortSubtitleTextInfo")
            .and_then(|v| v.get("text"))
            .and_then(|v| v.as_str())
            .or_else(|| data.get("subtitleTextInfo").and_then(|v| v.get("text")).and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();

        // Pick the largest image from mixImages
        let image_url = data.get("mixImages").and_then(|v| v.as_array()).and_then(|imgs| {
            imgs.iter()
                .filter_map(|img| {
                    let w = img.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                    let url = img.get("url").and_then(|v| v.as_str())?;
                    Some((w, url.to_string()))
                })
                .max_by_key(|(w, _)| *w)
                .map(|(_, url)| url)
        });

        debug!("Parsed mix from raw JSON: id={}, type={}, title={:?}", id, mix_type, title);

        Some(Mix { id, title, subtitle, mix_type, image_url })
    }

    // =========================================================================
    // Explore (TIDAL browse pages: /v1/pages/{path})
    // =========================================================================

    /// Fetch a browse page through tidlers and convert its modules for the UI.
    ///
    /// Accepts a bare slug (`"explore"`) or a page-link `apiPath`
    /// (`"pages/genre_hip_hop"`). Request parameters, authentication and web
    /// client headers are owned by tidlers.
    pub async fn get_explore_page(&self, path: &str) -> TidalResult<ExplorePage> {
        self.ensure_valid_token().await?;
        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
        let slug = client.normalize_page_slug(path);
        debug!("Fetching explore page: {}", slug);
        let page = client.get_page(&slug).await.map_err(|e| Self::request_error("Explore", e))?;
        drop(client_guard);

        let parsed = Self::parse_explore_page(&page);
        info!("Explore '{}': {} sections", slug, parsed.sections.len());
        Ok(parsed)
    }

    /// Keep request URLs and response bodies out of the error banner and logs.
    fn request_error(context: &str, error: tidlers::error::TidalError) -> TidalError {
        use tidlers::error::TidalError as ApiError;
        use tidlers::requests::RequestClientError;
        match error {
            ApiError::NotAuthenticated => TidalError::NotAuthenticated,
            ApiError::RequestClient(RequestClientError::StatusCode { status, .. }) => {
                TidalError::RequestFailed(format!("{context} request failed (HTTP {})", status.as_u16()))
            }
            ApiError::RequestClient(RequestClientError::Unauthorized) => {
                TidalError::RequestFailed(format!("TIDAL refused the {context} request (HTTP 401)"))
            }
            other if is_tidlers_network_error(&other) => {
                TidalError::NetworkError(format!("Could not reach TIDAL to load {context}. Please try again"))
            }
            _ => TidalError::RequestFailed(format!("Could not load {context} from TIDAL. Please try again")),
        }
    }

    /// Convert a typed browse page, skipping unsupported or empty modules and
    /// malformed individual items without discarding the usable sections.
    fn parse_explore_page(page: &PageResponse) -> ExplorePage {
        let sections = page.rows.iter().flat_map(|row| &row.modules).filter_map(Self::parse_explore_module).collect();
        ExplorePage { title: page.title.clone(), sections }
    }

    /// Parse a single module into an [`ExploreSection`], or `None` if it is
    /// empty or an unsupported type (e.g. videos).
    fn parse_explore_module(module: &PageModule) -> Option<ExploreSection> {
        let title = module.title.clone().unwrap_or_default();

        match module.page_type.as_str() {
            "FEATURED_PROMOTIONS" => {
                let items: Vec<ExploreCard> = module.items.iter().flatten().filter_map(Self::parse_promo_card).collect();
                (!items.is_empty()).then_some(ExploreSection::Featured { title, items })
            }
            "PAGE_LINKS" | "PAGE_LINKS_CLOUD" => {
                let links: Vec<PageLink> = Self::paged_items(module).iter().filter_map(Self::parse_page_link).collect();
                (!links.is_empty()).then_some(ExploreSection::Links { title, links })
            }
            "ALBUM_LIST" => {
                let albums: Vec<Album> = Self::paged_items(module).iter().filter_map(Self::parse_explore_album).collect();
                (!albums.is_empty()).then_some(ExploreSection::Albums { title, albums })
            }
            "PLAYLIST_LIST" => {
                let playlists: Vec<Playlist> =
                    Self::paged_items(module).iter().filter_map(Self::parse_explore_playlist).collect();
                (!playlists.is_empty()).then_some(ExploreSection::Playlists { title, playlists })
            }
            "ARTIST_LIST" => {
                let artists: Vec<Artist> = Self::paged_items(module).iter().filter_map(Self::parse_explore_artist).collect();
                (!artists.is_empty()).then_some(ExploreSection::Artists { title, artists })
            }
            _ => None,
        }
    }

    fn paged_items(module: &PageModule) -> &[serde_json::Value] {
        module.paged_list.as_ref().and_then(|list| list.items.as_deref()).unwrap_or(&[])
    }

    /// Parse a FEATURED_PROMOTIONS item into a card with a nav target.
    fn parse_promo_card(item: &serde_json::Value) -> Option<ExploreCard> {
        let title = item
            .get("header")
            .and_then(|v| v.as_str())
            .or_else(|| item.get("shortHeader").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        let subtitle = item.get("shortSubHeader").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
        let image_url = item.get("imageId").and_then(|v| v.as_str()).map(tidal_promo_image_url);

        let artifact_id = item.get("artifactId").and_then(|v| v.as_str()).unwrap_or("").to_string();
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
        Some(ExploreCard { title, subtitle, image_url, target })
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
            num_tracks: it.get("numberOfTracks").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            duration: it.get("duration").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            release_date: it.get("releaseDate").and_then(|v| v.as_str()).map(|s| s.to_string()),
            cover_url: it.get("cover").and_then(|v| v.as_str()).map(tidal_cover_url),
            explicit: it.get("explicit").and_then(|v| v.as_bool()).unwrap_or(false),
            audio_quality: it.get("audioQuality").and_then(|v| v.as_str()).map(|s| s.to_string()),
            quality_tags: it
                .get("mediaMetadata")
                .and_then(|m| m.get("tags"))
                .and_then(|t| t.as_array())
                .map(|tags| tags.iter().filter_map(|t| t.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            review: None,
        })
    }

    fn parse_explore_playlist(it: &serde_json::Value) -> Option<Playlist> {
        let uuid = it.get("uuid").and_then(|v| v.as_str())?.to_string();
        let image_id = it.get("squareImage").and_then(|v| v.as_str()).or_else(|| it.get("image").and_then(|v| v.as_str()));
        Some(Playlist {
            uuid,
            title: it.get("title").and_then(|v| v.as_str())?.to_string(),
            description: it.get("description").and_then(|v| v.as_str()).map(|s| s.to_string()),
            creator_name: None,
            num_tracks: it.get("numberOfTracks").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
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
            picture_url: it.get("picture").and_then(|v| v.as_str()).map(tidal_cover_url),
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
    /// Returns `None` when the seed has no radio mix (404), otherwise
    /// `Some((mix_id, tracks))`. The mix id is what lets plays from
    /// this view report as `sourceType=MIX, sourceId=<mix_id>` — the
    /// ONLY attribution that actually surfaces track-radio listening in
    /// TIDAL's Recently Played (empirically confirmed; the older
    /// `/tracks/{id}/radio` flat-list endpoint carries no mix id, so
    /// its plays could only be reported as the dead-end `TRACK_RADIO`
    /// sourceType that TIDAL's play_log silently drops).
    pub async fn get_track_mix(&self, track_id: &str) -> TidalResult<Option<(String, Vec<Track>)>> {
        self.ensure_valid_token().await?;
        info!("Fetching track mix for track {}", track_id);

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
        let Some(mix_response) = Self::track_radio_lookup(client.get_track_mix(track_id, None, None).await)? else {
            info!("No track radio available for seed {}", track_id);
            return Ok(None);
        };
        let mix_id = mix_response.id;

        let items_response = client.get_mix_tracks(mix_id.clone(), None, None).await.map_err(Self::track_radio_error)?;
        drop(client_guard);

        let tracks: Vec<Track> = items_response.items.into_iter().map(Track::from).collect();
        info!("Loaded track mix {} with {} tracks for seed track {}", mix_id, tracks.len(), track_id);
        Ok(Some((mix_id, tracks)))
    }

    /// Only a missing seed mix is an ordinary absence. A later failure to
    /// fetch its items is retryable and must not disable the seed's radio.
    fn track_radio_lookup<T>(result: Result<T, tidlers::error::TidalError>) -> TidalResult<Option<T>> {
        use tidlers::error::TidalError as ApiError;
        use tidlers::requests::RequestClientError;
        match result {
            Ok(mix) => Ok(Some(mix)),
            Err(ApiError::NotFound) => Ok(None),
            Err(ApiError::RequestClient(RequestClientError::StatusCode { status, .. }))
                if status == reqwest::StatusCode::NOT_FOUND =>
            {
                Ok(None)
            }
            Err(error) => Err(Self::track_radio_error(error)),
        }
    }

    /// Radio errors shown in the UI contain neither Rust variant names nor
    /// server URLs/bodies. Those are not useful instructions for a listener.
    fn track_radio_error(error: tidlers::error::TidalError) -> TidalError {
        use tidlers::error::TidalError as ApiError;
        use tidlers::requests::RequestClientError;
        let message = match error {
            ApiError::NotAuthenticated => return TidalError::NotAuthenticated,
            ApiError::NotFound => "This radio mix is no longer available on TIDAL".to_string(),
            ApiError::RequestClient(RequestClientError::StatusCode { status, .. }) => match status.as_u16() {
                401 => "TIDAL refused the radio request (HTTP 401). Try again or sign in again".to_string(),
                403 => "Track radio is not accessible on TIDAL (HTTP 403)".to_string(),
                404 => "This radio mix is no longer available on TIDAL. Please try again".to_string(),
                429 => "TIDAL's request limit was reached. Please try again shortly".to_string(),
                500..=599 => "TIDAL is having trouble loading radio. Please try again shortly".to_string(),
                other => format!("TIDAL could not load radio (HTTP {other})"),
            },
            ApiError::RequestClient(RequestClientError::Unauthorized) => {
                "TIDAL refused the radio request (HTTP 401). Try again or sign in again".to_string()
            }
            ApiError::RequestClient(RequestClientError::Timeout) => {
                "The track radio request timed out. Please try again".to_string()
            }
            ApiError::RequestClient(RequestClientError::RequestError(error)) | ApiError::Request(error) => {
                if error.is_timeout() {
                    "The track radio request timed out. Please try again".to_string()
                } else {
                    "Could not load track radio from TIDAL. Check your connection and try again".to_string()
                }
            }
            _ => "TIDAL returned an unexpected track radio response. Please try again".to_string(),
        };
        TidalError::RequestFailed(message)
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
    pub async fn get_similar_artists(&self, artist_id: &str, limit: Option<u32>) -> TidalResult<Vec<Artist>> {
        self.ensure_valid_token().await?;
        let limit_param = limit.unwrap_or(20);
        info!("Fetching similar artists for artist {} (limit {})", artist_id, limit_param);

        let client_guard = self.client.lock().await;
        let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
        let response = client
            .get_similar_artists(artist_id, Some(limit_param))
            .await
            .map_err(|e| TidalError::RequestFailed(format!("similar artists: {e:?}")))?;
        drop(client_guard);

        let artists: Vec<Artist> = response.items.into_iter().map(Artist::from).collect();
        info!("Loaded {} similar artists for artist {}", artists.len(), artist_id);
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

            let token = client.session.auth.access_token.as_ref().ok_or(TidalError::NotAuthenticated)?.clone();

            let cc = client.user_info.as_ref().map(|u| u.country_code.clone()).unwrap_or_else(|| "US".to_string());

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
                .map_err(|e| TidalError::NetworkError(format!("followed artists request failed: {}", e)))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                error!("Followed artists request failed: HTTP {} — {}", status, body);
                return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
            }

            let body =
                response.text().await.map_err(|e| TidalError::NetworkError(format!("reading followed artists body: {}", e)))?;

            let parsed: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| TidalError::ParseError(format!("parsing followed artists JSON: {}", e)))?;

            let page_count = if let Some(items) = parsed.get("items").and_then(|v| v.as_array()) {
                for item in items {
                    if let Some(data) = item.get("data") {
                        let id = data
                            .get("id")
                            .and_then(|v| v.as_u64().map(|n| n.to_string()).or_else(|| v.as_str().map(String::from)))
                            .unwrap_or_default();

                        let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();

                        let picture_url =
                            data.get("picture").and_then(|v| v.as_str()).filter(|p| !p.is_empty()).map(tidal_cover_url);

                        let popularity = data.get("popularity").and_then(|v| v.as_u64()).map(|p| p as u32);

                        let roles: Vec<String> = data
                            .get("artistRoles")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter().filter_map(|r| r.get("category").and_then(|c| c.as_str()).map(String::from)).collect()
                            })
                            .unwrap_or_default();

                        let url = data.get("url").and_then(|v| v.as_str()).map(String::from);

                        if !id.is_empty() && !name.is_empty() {
                            artists.push(Artist { id, name, picture_url, bio: None, popularity, roles, url });
                        }
                    }
                }
                items.len()
            } else {
                0
            };

            // Check for cursor-based pagination
            let next_cursor = parsed.get("cursor").and_then(|v| v.as_str()).map(String::from);

            debug!("Followed artists page: got {} items, cursor: {:?}", page_count, next_cursor);

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
        self.add_to_favorites(FavoriteResourceType::Artists, artist_id).await
    }

    /// Unfollow (remove from favorites) an artist by ID.
    ///
    /// Uses the TIDAL v1 API endpoint `DELETE /v1/users/{userId}/favorites/artists/{artistId}`.
    pub async fn unfollow_artist(&self, artist_id: &str) -> TidalResult<()> {
        debug!("Unfollowing artist {}", artist_id);
        self.remove_from_favorites(FavoriteResourceType::Artists, artist_id).await
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
        let raw = client.get_activity_feed().await.map_err(|e| TidalError::RequestFailed(format!("feed: {e:?}")))?;
        drop(client_guard);

        let activities: Vec<FeedActivity> = raw.into_iter().map(Self::from_tidlers_activity).collect();

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
                quality_tags: Vec::new(),
                review: None,
            }),
            TItem::HistoryMix(mix) => {
                FeedItem::HistoryMix { id: mix.id, title: mix.title, subtitle: mix.subtitle, image_url: mix.image_url }
            }
        };
        FeedActivity { item, occurred_at: a.occurred_at, seen: a.seen }
    }
}

#[cfg(test)]
mod tests {
    fn status_error(status: reqwest::StatusCode, body: &str) -> tidlers::error::TidalError {
        tidlers::error::TidalError::RequestClient(tidlers::requests::RequestClientError::StatusCode {
            status,
            url: "https://example.invalid/playback?token=do-not-log".to_string(),
            body_snippet: body.to_string(),
        })
    }

    mod lyrics {
        use super::super::{TidalAppClient, TidalError, TrackLyrics};
        use serde_json::{Value, json};
        use tidlers::client::models::track::LyricsResponse;

        fn convert(value: Value) -> TrackLyrics {
            let response: LyricsResponse = serde_json::from_value(value).expect("SDK lyrics fixture should deserialize");
            TidalAppClient::lyrics_from_response(Ok(response)).expect("lyrics should convert")
        }

        #[test]
        fn complete_response_preserves_text_attribution_and_timing() {
            let text = "  First line\nSecond line  ";
            let lyrics = convert(json!({
                "trackId": 1, "lyricsProvider": "Provider", "providerCommontrackId": "common",
                "providerLyricsId": "lyrics", "lyrics": text,
                "subtitles": "[00:05.67]Second line\n[00:01.23]First line", "isRightToLeft": false
            }));
            assert_eq!(lyrics.provider.as_deref(), Some("Provider"));
            assert_eq!(lyrics.plain_text.as_deref(), Some(text));
            assert!(!lyrics.is_right_to_left);
            assert!(!lyrics.is_empty());
            assert!(lyrics.is_synced());
            assert_eq!(lyrics.lrc_lines.len(), 2);
            assert_eq!(lyrics.lrc_lines[0].time_ms, 1230);
            assert_eq!(lyrics.lrc_lines[0].text, "First line");
            assert_eq!(lyrics.lrc_lines[1].time_ms, 5670);
            assert_eq!(lyrics.line_index_at(1.229), None);
            assert_eq!(lyrics.line_index_at(1.230), Some(0));
            assert_eq!(lyrics.line_index_at(5.670), Some(1));
        }

        #[test]
        fn subtitles_only_response_keeps_synced_lyrics_without_provider_metadata() {
            let lyrics = convert(json!({
                "trackId": 1, "subtitles": "[00:02.00][00:04.00]Chorus"
            }));
            assert!(lyrics.plain_text.is_none());
            assert!(lyrics.provider.is_none());
            assert!(lyrics.is_synced());
            assert!(!lyrics.is_empty());
            assert!(!lyrics.is_right_to_left);
            assert_eq!(lyrics.lrc_lines.iter().map(|line| line.time_ms).collect::<Vec<_>>(), [2000, 4000]);
        }

        #[test]
        fn missing_null_and_blank_plain_text_remain_empty_without_subtitles() {
            for payload in [
                json!({"trackId": 1}),
                json!({"trackId": 1, "lyrics": null, "lyricsProvider": null,
                    "providerCommontrackId": null, "providerLyricsId": null, "subtitles": null}),
                json!({"trackId": 1, "lyrics": "", "subtitles": ""}),
                json!({"trackId": 1, "lyrics": " \t\n ", "subtitles": " "}),
            ] {
                let lyrics = convert(payload);
                assert!(lyrics.plain_text.is_none());
                assert!(lyrics.is_empty());
                assert!(!lyrics.is_synced());
            }
        }

        #[test]
        fn plain_only_rtl_text_and_original_line_breaks_are_preserved() {
            let text = "  مرحباً\nبالعالم  ";
            let lyrics = convert(json!({"trackId": 1, "lyrics": text, "isRightToLeft": true}));
            assert_eq!(lyrics.plain_text.as_deref(), Some(text));
            assert!(lyrics.is_right_to_left);
            assert!(!lyrics.is_empty());
            assert!(!lyrics.is_synced());
        }

        #[test]
        fn malformed_lrc_does_not_discard_usable_plain_text() {
            let lyrics = convert(json!({"trackId": 1, "lyrics": "Plain words", "subtitles": "[not-a-time]ignore me"}));
            assert_eq!(lyrics.plain_text.as_deref(), Some("Plain words"));
            assert!(!lyrics.is_synced());
            assert!(!lyrics.is_empty());
        }

        #[test]
        fn sdk_not_found_is_a_normal_empty_result() {
            let lyrics = TidalAppClient::lyrics_from_response(Err(tidlers::error::TidalError::NotFound)).unwrap();
            assert!(lyrics.is_empty());
            assert!(lyrics.provider.is_none());
            assert!(!lyrics.is_right_to_left);
        }

        #[test]
        fn auth_rate_limit_and_service_errors_are_not_cached_as_missing_lyrics() {
            for status in [
                reqwest::StatusCode::UNAUTHORIZED,
                reqwest::StatusCode::FORBIDDEN,
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
            ] {
                let message = TidalAppClient::lyrics_from_response(Err(super::status_error(status, "private upstream detail")))
                    .unwrap_err()
                    .to_string();
                assert!(message.contains("Lyrics"));
                assert!(message.contains(&format!("HTTP {}", status.as_u16())));
                assert!(!message.contains("private upstream detail"));
                assert!(!message.contains("do-not-log"));
                assert!(!message.contains("StatusCode"));
            }
            assert!(matches!(
                TidalAppClient::lyrics_from_response(Err(tidlers::error::TidalError::NotAuthenticated)),
                Err(TidalError::NotAuthenticated)
            ));
            let timeout = tidlers::error::TidalError::RequestClient(tidlers::requests::RequestClientError::Timeout);
            assert!(TidalAppClient::lyrics_from_response(Err(timeout)).is_err());
        }

        #[test]
        fn malformed_response_remains_an_error() {
            for payload in [json!({"status": 500}), json!({"trackId": 1, "lyrics": ["wrong type"]})] {
                let error = serde_json::from_value::<LyricsResponse>(payload).unwrap_err();
                assert!(TidalAppClient::lyrics_from_response(Err(tidlers::error::TidalError::JsonParse(error))).is_err());
            }
        }
    }

    mod favorite_albums {
        use super::super::{Album, ApiPaginatedResponse, CollectionFavoriteAlbumsResponse, TidalAppClient};
        use std::future::ready;

        fn page(offset: i32, total: i32, ids: std::ops::Range<u64>) -> ApiPaginatedResponse<Album> {
            let items: Vec<_> = ids.map(|id| serde_json::json!({
                "created": "2026-01-01T00:00:00Z",
                "item": {
                    "id": id, "title": format!("Album {id}"), "cover": "aa-bb", "releaseDate": "2026-01-01",
                    "artist": {"id": 9, "name": "Artist"}, "numberOfTracks": 12, "duration": 3600,
                    "explicit": true, "audioQuality": "LOSSLESS", "mediaMetadata": {"tags": ["LOSSLESS", "HIRES_LOSSLESS"]}
                }
            })).collect();
            let sdk: CollectionFavoriteAlbumsResponse = serde_json::from_value(serde_json::json!({
                "items": items, "offset": offset, "limit": 100, "totalNumberOfItems": total
            }))
            .expect("SDK favourite-albums fixture should deserialize");
            sdk.into()
        }

        #[tokio::test]
        async fn all_album_pages_preserve_metadata_and_order() {
            let mut calls = Vec::new();
            let albums = TidalAppClient::collect_favorites("Favorite albums", |limit, offset| {
                calls.push((limit, offset));
                ready(Ok(match offset {
                    0 => page(0, 205, 0..100),
                    100 => page(100, 205, 100..200),
                    200 => page(200, 205, 200..205),
                    _ => panic!("unexpected offset {offset}"),
                }))
            })
            .await
            .unwrap();
            assert_eq!(calls, [(100, 0), (100, 100), (100, 200)]);
            assert_eq!(albums.len(), 205);
            assert!(albums.iter().enumerate().all(|(i, album)| album.id == i.to_string()));
            let album = &albums[0];
            assert_eq!(album.title, "Album 0");
            assert_eq!(album.artist_name, "Artist");
            assert_eq!(album.artist_id.as_deref(), Some("9"));
            assert_eq!(album.num_tracks, 12);
            assert_eq!(album.duration, 3600);
            assert_eq!(album.release_date.as_deref(), Some("2026-01-01"));
            assert_eq!(album.cover_url.as_deref(), Some("https://resources.tidal.com/images/aa/bb/320x320.jpg"));
            assert!(album.explicit);
            assert_eq!(album.audio_quality.as_deref(), Some("LOSSLESS"));
            assert_eq!(album.advertised_quality().as_deref(), Some("Hi-Res Lossless"));
            assert!(album.review.is_none());
        }

        #[test]
        fn absent_and_null_album_metadata_use_domain_defaults() {
            for payload in [
                serde_json::json!({"id": 1, "title": "Minimal"}),
                serde_json::json!({"id": 1, "title": "Minimal", "artist": null, "cover": null,
                    "numberOfTracks": null, "duration": null, "explicit": null, "mediaMetadata": null,
                    "audioQuality": "HIGH"}),
            ] {
                let sdk: tidlers::client::models::album::Album = serde_json::from_value(payload).unwrap();
                let album = Album::from(sdk);
                assert_eq!(album.artist_name, "Unknown Artist");
                assert!(album.artist_id.is_none());
                assert_eq!(album.num_tracks, 0);
                assert_eq!(album.duration, 0);
                assert!(!album.explicit);
                assert!(album.cover_url.is_none());
                assert!(album.advertised_quality().is_none());
            }
        }

        #[test]
        fn explicit_zero_false_and_empty_tags_are_preserved() {
            let sdk: tidlers::client::models::album::Album = serde_json::from_value(serde_json::json!({
                "id": 1, "title": "Empty", "numberOfTracks": 0, "duration": 0,
                "explicit": false, "audioQuality": "HIGH", "mediaMetadata": {"tags": []}
            }))
            .unwrap();
            let album = Album::from(sdk);
            assert_eq!(album.num_tracks, 0);
            assert_eq!(album.duration, 0);
            assert!(!album.explicit);
            assert_eq!(album.audio_quality.as_deref(), Some("HIGH"));
            assert!(album.quality_tags.is_empty());
            assert!(album.advertised_quality().is_none());
        }

        #[tokio::test]
        async fn album_request_failure_does_not_return_a_partial_collection() {
            let result = TidalAppClient::collect_favorites("Favorite albums", |_, offset| {
                ready(if offset == 0 {
                    Ok(page(0, 101, 0..100))
                } else {
                    Err(tidlers::error::TidalError::Other("private upstream detail".into()))
                })
            })
            .await;
            let error = result.unwrap_err().to_string();
            assert!(error.contains("Favorite albums"));
            assert!(!error.contains("private upstream detail"));
        }

        #[tokio::test]
        async fn empty_album_collection_finishes_after_one_request() {
            let mut calls = 0;
            let albums = TidalAppClient::collect_favorites("Favorite albums", |limit, offset| {
                calls += 1;
                assert_eq!((limit, offset), (100, 0));
                ready(Ok(page(0, 0, 0..0)))
            })
            .await
            .unwrap();
            assert!(albums.is_empty());
            assert_eq!(calls, 1);
        }
    }

    mod favorite_tracks {
        use super::super::{ApiPaginatedResponse, CollectionFavoriteTracksResponse, TidalAppClient, Track};
        use std::future::ready;

        fn sdk_page(offset: i32, total: i32, ids: std::ops::Range<u64>) -> CollectionFavoriteTracksResponse {
            let items: Vec<_> = ids
                .map(|id| {
                    serde_json::json!({
                        "created": "2026-01-01T00:00:00Z",
                        "item": {
                            "id": id, "title": format!("Track {id}"), "duration": 123,
                            "replayGain": -3.0, "peak": 0.9, "allowStreaming": true,
                            "streamReady": true, "payToStream": false, "adSupportedStreamReady": false,
                            "djReady": false, "stemReady": false, "premiumStreamingOnly": false,
                            "trackNumber": 7, "volumeNumber": 1, "popularity": 1,
                            "url": "https://example.invalid/track", "editable": false,
                            "explicit": true, "audioQuality": "LOSSLESS", "audioModes": ["STEREO"],
                            "upload": false, "artist": {"id": 9, "name": "Artist"}, "artists": [],
                            "album": {"id": 99, "title": "Album", "cover": "aa-bb"}
                        }
                    })
                })
                .collect();
            serde_json::from_value(serde_json::json!({
                "items": items, "offset": offset, "limit": 100, "totalNumberOfItems": total
            }))
            .expect("SDK favourites fixture should deserialize")
        }

        fn page(offset: i32, total: i32, ids: std::ops::Range<u64>) -> ApiPaginatedResponse<Track> {
            sdk_page(offset, total, ids).into()
        }

        #[tokio::test]
        async fn loads_all_pages_in_order_and_preserves_track_metadata() {
            let mut calls = Vec::new();
            let tracks = TidalAppClient::collect_favorites("Favorite tracks", |limit, offset| {
                calls.push((limit, offset));
                ready(Ok(match offset {
                    0 => page(0, 205, 0..100),
                    100 => page(100, 205, 100..200),
                    200 => page(200, 205, 200..205),
                    _ => panic!("unexpected offset {offset}"),
                }))
            })
            .await
            .unwrap();
            assert_eq!(calls, [(100, 0), (100, 100), (100, 200)]);
            assert_eq!(tracks.len(), 205);
            assert!(tracks.iter().enumerate().all(|(i, track)| track.id == i.to_string()));
            let track = &tracks[0];
            assert_eq!(track.title, "Track 0");
            assert_eq!(track.artist_name, "Artist");
            assert_eq!(track.artist_id.as_deref(), Some("9"));
            assert_eq!(track.album_name.as_deref(), Some("Album"));
            assert_eq!(track.album_id.as_deref(), Some("99"));
            assert_eq!(track.cover_url.as_deref(), Some("https://resources.tidal.com/images/aa/bb/320x320.jpg"));
            assert_eq!(track.duration, 123);
            assert_eq!(track.track_number, 7);
            assert!(track.explicit);
            assert_eq!(track.audio_quality.as_deref(), Some("LOSSLESS"));
            assert!(!track.is_video);
        }

        #[tokio::test]
        async fn short_pages_advance_by_returned_count_not_requested_limit() {
            let mut offsets = Vec::new();
            let tracks = TidalAppClient::collect_favorites("Favorite tracks", |_, offset| {
                offsets.push(offset);
                ready(Ok(match offset {
                    0 => page(0, 3, 0..2),
                    2 => page(2, 3, 2..3),
                    _ => panic!("unexpected offset {offset}"),
                }))
            })
            .await
            .unwrap();
            assert_eq!(offsets, [0, 2]);
            assert_eq!(tracks.len(), 3);
        }

        #[tokio::test]
        async fn an_empty_page_stops_even_when_the_reported_total_is_stale() {
            for total in [0, 100] {
                let mut calls = 0;
                let tracks = TidalAppClient::collect_favorites("Favorite tracks", |_, offset| {
                    calls += 1;
                    assert_eq!(offset, 0);
                    ready(Ok(page(0, total, 0..0)))
                })
                .await
                .unwrap();
                assert_eq!(calls, 1);
                assert!(tracks.is_empty());
            }
        }

        #[tokio::test]
        async fn a_later_page_failure_does_not_return_a_partial_collection() {
            let mut offsets = Vec::new();
            let result = TidalAppClient::collect_favorites("Favorite tracks", |_, offset| {
                offsets.push(offset);
                ready(if offset == 0 {
                    Ok(page(0, 101, 0..100))
                } else {
                    Err(tidlers::error::TidalError::Other("private upstream detail".into()))
                })
            })
            .await;
            assert_eq!(offsets, [0, 100]);
            let error = result.unwrap_err().to_string();
            assert!(error.contains("Favorite tracks"));
            assert!(!error.contains("private upstream detail"));
        }

        #[tokio::test]
        async fn wrong_offsets_and_negative_totals_fail_explicitly() {
            // A server that ignores offset repeats the first page.
            let result = TidalAppClient::collect_favorites("Favorite tracks", |_, _| ready(Ok(page(0, 101, 0..100)))).await;
            assert!(result.unwrap_err().to_string().contains("unexpected page offset"));
            let result = TidalAppClient::collect_favorites("Favorite tracks", |_, _| ready(Ok(page(0, -1, 0..0)))).await;
            assert!(result.unwrap_err().to_string().contains("negative total"));
        }

        #[tokio::test]
        async fn a_track_without_an_album_still_converts() {
            let mut response = sdk_page(0, 1, 1..2);
            response.items[0].item.album = None;
            let mut response = Some(ApiPaginatedResponse::<Track>::from(response));
            let tracks =
                TidalAppClient::collect_favorites("Favorite tracks", |_, _| ready(Ok(response.take().unwrap()))).await.unwrap();
            assert!(tracks[0].album_id.is_none());
            assert!(tracks[0].cover_url.is_none());
        }
    }

    mod explore_pages {
        use super::super::{ExplorePage, ExploreSection, ExploreTarget, PageResponse, TidalAppClient};
        use serde_json::{Value, json};

        fn convert(modules: Vec<Value>) -> ExplorePage {
            let page: PageResponse = serde_json::from_value(json!({
                "id": "explore", "title": "Explore", "rows": [{"modules": modules}]
            }))
            .expect("tidlers should parse the page");
            TidalAppClient::parse_explore_page(&page)
        }

        #[test]
        fn sparse_featured_cards_keep_their_targets_and_artwork() {
            let page = convert(vec![json!({
                "type": "FEATURED_PROMOTIONS", "description": null, "width": null, "pagedList": null,
                "items": [
                    {"type": "ALBUM", "header": "Featured album", "artifactId": "123", "imageId": "ab-cd"},
                    {"type": "PAGE", "shortHeader": "R&B / Soul", "artifactId": "pages/genre_rnb"},
                    {"type": "EXTURL", "header": "Magazine", "artifactId": "https://example.invalid"},
                    null
                ]
            })]);
            assert_eq!(page.title, "Explore");
            let [ExploreSection::Featured { title, items }] = page.sections.as_slice() else {
                panic!("expected one Featured section");
            };
            assert!(title.is_empty());
            assert_eq!(items.len(), 2);
            assert!(matches!(&items[0].target, ExploreTarget::Album(id) if id == "123"));
            assert!(items[0].image_url.is_some());
            assert!(matches!(&items[1].target, ExploreTarget::Page(path) if path == "pages/genre_rnb"));
        }

        #[test]
        fn mixed_page_keeps_links_albums_playlists_and_artists_in_order() {
            let page = convert(vec![
                json!({"type": "PAGE_LINKS_CLOUD", "title": "Genres", "pagedList": {"items": [
                    {"title": "R&B / Soul", "apiPath": "pages/genre_rnb"}, {"title": "Missing path"}
                ]}}),
                json!({"type": "PAGE_LINKS", "pagedList": {"items": [
                    {"text": "Moods", "path": "/v1/pages/moods"}
                ]}}),
                json!({"type": "FUTURE_BANNER", "payload": {"unknown": true}}),
                json!({"type": "ALBUM_LIST", "title": "New Albums", "pagedList": {"items": [
                    {"id": 123, "title": "Album", "artists": [{"id": 7, "name": "Artist"}],
                     "mediaMetadata": {"tags": ["LOSSLESS", "HIRES_LOSSLESS"]}},
                    {"id": 456}, null
                ]}}),
                json!({"type": "PLAYLIST_LIST", "title": "Essentials", "pagedList": {"items": [
                    {"uuid": "playlist-id", "title": "Playlist", "numberOfTracks": 12}, {"title": "Missing id"}
                ]}}),
                json!({"type": "ARTIST_LIST", "pagedList": {"items": [
                    {"id": "7", "name": "Artist"}, {"id": "8"}, false
                ]}}),
            ]);
            let [
                ExploreSection::Links { links: genres, .. },
                ExploreSection::Links { links: moods, .. },
                ExploreSection::Albums { albums, .. },
                ExploreSection::Playlists { playlists, .. },
                ExploreSection::Artists { artists, .. },
            ] = page.sections.as_slice()
            else {
                panic!("expected five usable sections in source order");
            };
            assert_eq!(genres.len(), 1);
            assert_eq!(genres[0].path, "pages/genre_rnb");
            assert_eq!(moods[0].path, "/v1/pages/moods");
            assert_eq!(albums.len(), 1);
            assert_eq!(albums[0].id, "123");
            assert_eq!(albums[0].artist_name, "Artist");
            assert_eq!(albums[0].advertised_quality().as_deref(), Some("Hi-Res Lossless"));
            assert_eq!(playlists.len(), 1);
            assert_eq!(playlists[0].uuid, "playlist-id");
            assert_eq!(playlists[0].num_tracks, 12);
            assert_eq!(artists.len(), 1);
            assert_eq!(artists[0].id, "7");
        }

        #[test]
        fn missing_null_and_empty_collections_do_not_create_empty_sections() {
            let page = convert(vec![
                json!({"type": "FEATURED_PROMOTIONS"}),
                json!({"type": "FEATURED_PROMOTIONS", "items": null}),
                json!({"type": "ALBUM_LIST", "pagedList": null}),
                json!({"type": "PLAYLIST_LIST", "pagedList": {"dataApiPath": "playlists"}}),
                json!({"type": "ARTIST_LIST", "pagedList": {"items": null}}),
                json!({"type": "PAGE_LINKS", "pagedList": {"items": []}}),
            ]);
            assert!(page.sections.is_empty());
            assert!(convert(vec![]).sections.is_empty());
        }

        #[test]
        fn each_module_reads_the_correct_item_collection() {
            let page = convert(vec![
                json!({"type": "FEATURED_PROMOTIONS", "items": [
                    {"type": "ALBUM", "header": "Direct promo", "artifactId": "123"}
                ], "pagedList": {"items": [{"type": "ALBUM", "header": "Wrong collection", "artifactId": "456"}]}}),
                json!({"type": "ALBUM_LIST", "items": [{"id": 123, "title": "Wrong collection"}],
                    "pagedList": {"items": [{"id": 456, "title": "Paged album"}]}}),
            ]);
            let [ExploreSection::Featured { items, .. }, ExploreSection::Albums { albums, .. }] = page.sections.as_slice() else {
                panic!("expected promo and album sections");
            };
            assert_eq!(items[0].title, "Direct promo");
            assert_eq!(albums[0].title, "Paged album");
        }

        #[test]
        fn tidlers_normalizes_all_supported_page_link_forms() {
            let client = tidlers::TidalClient::new(&tidlers::auth::TidalAuth::with_oauth());
            for path in
                ["genre_rnb", "/genre_rnb", "pages/genre_rnb", "/pages/genre_rnb", "v1/pages/genre_rnb", "/v1/pages/genre_rnb"]
            {
                assert_eq!(client.normalize_page_slug(path), "genre_rnb");
            }
            assert_eq!(client.normalize_page_slug("explore"), "explore");
        }

        #[test]
        fn request_errors_do_not_expose_urls_bodies_or_rust_variants() {
            let error = super::status_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "private-body");
            let message = TidalAppClient::request_error("Explore", error).to_string();
            assert!(message.contains("HTTP 500"));
            assert!(!message.contains("private-body"));
            assert!(!message.contains("do-not-log"));
            assert!(!message.contains("RequestClient"));
        }
    }

    #[test]
    fn only_an_explicit_asset_error_is_unavailable() {
        let err = status_error(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#,
        );
        assert_eq!(classify_playback_error(&err), PlaybackFailure::Unavailable);
    }

    #[test]
    fn a_401_without_asset_evidence_must_not_skip() {
        let err = tidlers::error::TidalError::RequestClient(tidlers::requests::RequestClientError::Unauthorized);
        assert_eq!(classify_playback_error(&err), PlaybackFailure::Rejected);
        for body in [r#"{"status":401,"subStatus":1001}"#, "{}", "unauthorized", r#"{"subStatus":400"#] {
            assert_eq!(
                classify_playback_error(&status_error(reqwest::StatusCode::UNAUTHORIZED, body)),
                PlaybackFailure::Rejected
            );
        }
    }

    #[test]
    fn other_http_errors_do_not_skip_or_leak_response_data() {
        for status in [
            reqwest::StatusCode::FORBIDDEN,
            reqwest::StatusCode::NOT_FOUND,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let failure = classify_playback_error(&status_error(status, r#"{"subStatus":4005,"token":"private"}"#));
            assert!(matches!(failure, PlaybackFailure::Failed(_)));
            assert_eq!(failure.to_string(), format!("TIDAL playback request failed (HTTP {})", status.as_u16()));
        }
    }

    #[test]
    fn missing_track_radio_is_a_normal_absence() {
        let missing = status_error(
            reqwest::StatusCode::NOT_FOUND,
            r#"{"status":404,"subStatus":2001,"userMessage":"TrackMixId for mixId: [459351692] not found"}"#,
        );
        assert!(TidalAppClient::track_radio_lookup::<()>(Err(missing)).unwrap().is_none());
        assert!(TidalAppClient::track_radio_lookup::<()>(Err(tidlers::error::TidalError::NotFound)).unwrap().is_none());
        assert_eq!(TidalAppClient::track_radio_lookup(Ok("mix-id")).unwrap(), Some("mix-id"));
    }

    #[test]
    fn radio_auth_rate_limit_and_service_errors_remain_retryable() {
        for status in [
            reqwest::StatusCode::UNAUTHORIZED,
            reqwest::StatusCode::FORBIDDEN,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let error = TidalAppClient::track_radio_lookup::<()>(Err(status_error(status, "private-body"))).unwrap_err();
            let text = error.to_string();
            assert!(!text.contains("private-body"));
            assert!(!text.contains("do-not-log"));
            assert!(!text.contains("StatusCode"));
            assert!(!text.contains("RequestClient"));
            assert!(text.contains("TIDAL"));
        }
    }

    #[test]
    fn a_missing_radio_items_page_remains_a_retryable_error() {
        let error = TidalAppClient::track_radio_error(status_error(reqwest::StatusCode::NOT_FOUND, "private-body"));
        assert_eq!(error.to_string(), "Request failed: This radio mix is no longer available on TIDAL. Please try again");
    }

    #[test]
    fn malformed_and_timeout_radio_responses_do_not_claim_absence() {
        for error in [
            tidlers::error::TidalError::Other("private-body".into()),
            tidlers::error::TidalError::RequestClient(tidlers::requests::RequestClientError::Timeout),
            tidlers::error::TidalError::RequestClient(tidlers::requests::RequestClientError::Unauthorized),
        ] {
            let text = TidalAppClient::track_radio_lookup::<()>(Err(error)).unwrap_err().to_string();
            assert!(!text.contains("private-body"));
            assert!(!text.contains("RequestClient"));
        }
    }

    #[test]
    fn anything_else_keeps_its_message() {
        let err = tidlers::error::TidalError::Other("kaboom".to_string());
        match classify_playback_error(&err) {
            PlaybackFailure::Failed(msg) => assert!(msg.contains("kaboom"), "{msg}"),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn an_unavailable_track_reads_as_itself() {
        assert_eq!(PlaybackFailure::Unavailable.to_string(), "This item is not available for playback on TIDAL");
    }

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
            Some("https://resources.tidal.com/images/7bd9a4c2/424a/49cf/afd9/31f6e526a71e/320x320.jpg")
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
