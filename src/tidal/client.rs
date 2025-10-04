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
use super::models::{Album, Artist, Mix, Playlist, SearchResults, Track};
use base64::{Engine, engine::general_purpose};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use tidlers::{TidalClient, auth::init::TidalAuth, client::models::playback::AudioQuality};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::disk_cache::DiskCache;

/// Safety margin before token expiry to trigger refresh (5 minutes)
const TOKEN_REFRESH_MARGIN_SECS: u64 = 300;

/// Result of getting a playback URL - can be direct URL, DASH manifest, or cached file
#[derive(Debug, Clone)]
pub enum PlaybackUrl {
    /// Direct streaming URL (for Low/High/Lossless quality)
    Direct(String, Option<f32>),
    /// Path to a temporary DASH manifest file (for HiRes quality)
    DashManifest(PathBuf, Option<f32>),
    /// Path to a cached audio file on disk (already downloaded previously)
    CachedFile(PathBuf, Option<f32>),
}

impl PlaybackUrl {
    /// Get the URL or path as a string for playback
    pub fn as_url(&self) -> String {
        match self {
            PlaybackUrl::Direct(url, _) => url.clone(),
            PlaybackUrl::DashManifest(path, _) | PlaybackUrl::CachedFile(path, _) => {
                path.to_string_lossy().to_string()
            }
        }
    }

    /// Check if this is a DASH manifest (requires special handling)
    pub fn is_dash(&self) -> bool {
        matches!(self, PlaybackUrl::DashManifest(..))
    }

    /// Check if this is a cached file (play from disk, no download needed)
    pub fn is_cached(&self) -> bool {
        matches!(self, PlaybackUrl::CachedFile(..))
    }

    /// Get the replay gain value in dB, if available from the TIDAL API.
    pub fn replay_gain_db(&self) -> Option<f32> {
        match self {
            PlaybackUrl::Direct(_, rg)
            | PlaybackUrl::DashManifest(_, rg)
            | PlaybackUrl::CachedFile(_, rg) => *rg,
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
struct ApiItemWrapper<T> {
    item: Option<T>,
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
    artist: ApiTrackArtist,
    album: ApiTrackAlbum,
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
        Track {
            id: t.id.to_string(),
            title: t.title,
            duration: t.duration as u32,
            track_number: t.track_number,
            artist_name: t
                .artist
                .name
                .unwrap_or_else(|| "Unknown Artist".to_string()),
            artist_id: Some(t.artist.id.to_string()),
            album_name: Some(t.album.title),
            album_id: Some(t.album.id.to_string()),
            cover_url: t.album.cover.map(|c| TidalAppClient::uuid_to_cdn_url(&c)),
            explicit: t.explicit,
            audio_quality: t.audio_quality,
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
            cover_url: Some(TidalAppClient::uuid_to_cdn_url(&a.cover)),
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
    /// Disk cache for DASH manifest files (size-limited, LRU-evicted)
    dash_cache: DiskCache,
    /// Disk cache for downloaded audio/song files (size-limited, LRU-evicted)
    audio_cache: DiskCache,
    /// Disk cache for API response JSON (playlists, albums, tracks, etc.)
    api_cache: DiskCache,
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

    // ── Favorites add/remove helpers ────────────────────────────────────

    /// POST to `/v1/users/{userId}/favorites/{resource}` with a form body.
    ///
    /// `resource` is `"tracks"`, `"albums"`, or `"artists"`.
    /// `id_param` is `"trackIds"`, `"albumIds"`, or `"artistIds"`.
    async fn add_to_favorites(
        &self,
        resource: &str,
        id_param: &str,
        resource_id: &str,
    ) -> TidalResult<()> {
        self.ensure_valid_token().await?;

        let ctx = self.auth_context_with_user().await?;

        let url = format!(
            "https://api.tidal.com/v1/users/{}/favorites/{}?countryCode={}",
            ctx.user_id, resource, ctx.country_code
        );

        let http_client = reqwest::Client::new();
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", ctx.access_token))
                .map_err(|e| TidalError::RequestFailed(format!("Invalid auth header: {}", e)))?,
        );
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );

        let body = format!("{}={}", id_param, resource_id);

        match http_client
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await
        {
            Ok(response) => {
                if response.status().is_success() || response.status().as_u16() == 201 {
                    info!("{} {} added to favorites", resource, resource_id);
                    Ok(())
                } else {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    error!("Failed to add {} favorite: {} - {}", resource, status, body);
                    Err(TidalError::RequestFailed(format!(
                        "HTTP {}: {}",
                        status, body
                    )))
                }
            }
            Err(e) => {
                error!("Failed to add favorite {}: {:?}", resource, e);
                Err(TidalError::NetworkError(format!("{:?}", e)))
            }
        }
    }

    /// DELETE `/v1/users/{userId}/favorites/{resource}/{resourceId}`.
    async fn remove_from_favorites(&self, resource: &str, resource_id: &str) -> TidalResult<()> {
        self.ensure_valid_token().await?;

        let ctx = self.auth_context_with_user().await?;

        let url = format!(
            "https://api.tidal.com/v1/users/{}/favorites/{}/{}?countryCode={}",
            ctx.user_id, resource, resource_id, ctx.country_code
        );

        let http_client = reqwest::Client::new();
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", ctx.access_token))
                .map_err(|e| TidalError::RequestFailed(format!("Invalid auth header: {}", e)))?,
        );

        match http_client.delete(&url).headers(headers).send().await {
            Ok(response) => {
                if response.status().is_success() || response.status().as_u16() == 204 {
                    info!("{} {} removed from favorites", resource, resource_id);
                    Ok(())
                } else {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    error!(
                        "Failed to remove {} favorite: {} - {}",
                        resource, status, body
                    );
                    Err(TidalError::RequestFailed(format!(
                        "HTTP {}: {}",
                        status, body
                    )))
                }
            }
            Err(e) => {
                error!("Failed to remove favorite {}: {:?}", resource, e);
                Err(TidalError::NetworkError(format!("{:?}", e)))
            }
        }
    }

    /// Create a new TidalAppClient
    pub fn new() -> Self {
        Self::new_with_audio_cache_mb(2000)
    }

    /// Create a new TidalAppClient with a specific audio cache size
    pub fn new_with_audio_cache_mb(audio_cache_max_mb: u32) -> Self {
        Self {
            client: Arc::new(Mutex::new(None)),
            auth_manager: AuthManager::new(),
            audio_quality: AudioQuality::High,
            // DASH manifests are small XML files (a few KB each); 10 MB is plenty
            dash_cache: DiskCache::xdg("dash", 10),
            // Audio files: songs cached on disk for instant replay
            audio_cache: DiskCache::xdg("audio", audio_cache_max_mb),
            // API responses: playlists, albums, tracks JSON (small, 50 MB)
            api_cache: DiskCache::xdg("api", 50),
        }
    }

    /// Create a client with all caches rooted under `base_dir`.
    ///
    /// Intended for tests so each test gets an isolated directory and
    /// parallel runs never interfere with each other.
    pub fn new_with_cache_dir(base_dir: &std::path::Path, audio_cache_max_mb: u32) -> Self {
        Self {
            client: Arc::new(Mutex::new(None)),
            auth_manager: AuthManager::new(),
            audio_quality: AudioQuality::High,
            dash_cache: DiskCache::new(base_dir.join("dash"), 10),
            audio_cache: DiskCache::new(base_dir.join("audio"), audio_cache_max_mb),
            api_cache: DiskCache::new(base_dir.join("api"), 50),
        }
    }

    /// Get a reference to the audio cache (for saving downloaded audio from the engine)
    pub fn audio_cache(&self) -> &DiskCache {
        &self.audio_cache
    }

    /// Get a reference to the API cache
    pub fn api_cache(&self) -> &DiskCache {
        &self.api_cache
    }

    /// Build the audio cache key for a track ID and the current quality setting
    pub fn audio_cache_key(&self, track_id: &str) -> String {
        format!("{}_{:?}", track_id, self.audio_quality)
    }

    /// Minimum size (in bytes) for a cached audio file to be considered valid.
    ///
    /// When the user skips tracks quickly, in-flight downloads are aborted and
    /// the partial data may have been saved to disk before the abort-guard was
    /// added.  A real FLAC/AAC track is always well above 64 KB, so anything
    /// smaller is almost certainly a truncated fragment left over from an
    /// interrupted download.
    const MIN_CACHED_AUDIO_BYTES: u64 = 64 * 1024;

    /// Check if a track is already cached on disk. Returns the path if so.
    ///
    /// Files smaller than [`Self::MIN_CACHED_AUDIO_BYTES`] are treated as
    /// corrupt/truncated leftovers from an aborted download: they are deleted
    /// on the spot and `None` is returned so the caller fetches a fresh copy.
    pub fn get_cached_audio_path(&self, track_id: &str) -> Option<PathBuf> {
        let key = self.audio_cache_key(track_id);
        let path = self.audio_cache.hashed_path(&key, "dat");
        if path.exists() {
            // Reject suspiciously small files — they are almost certainly
            // truncated fragments from an aborted download.
            if let Ok(meta) = std::fs::metadata(&path) {
                if meta.len() < Self::MIN_CACHED_AUDIO_BYTES {
                    warn!(
                        "Cached audio for track {} is only {} bytes — removing truncated file {:?}",
                        track_id,
                        meta.len(),
                        path,
                    );
                    let _ = std::fs::remove_file(&path);
                    // Also remove the replay-gain sidecar if present
                    let rg_path = self.audio_cache.hashed_path(&key, "rg");
                    let _ = std::fs::remove_file(&rg_path);
                    return None;
                }
            }
            // Touch the file so LRU eviction keeps it alive
            DiskCache::touch_path(&path);
            info!("Audio cache hit for track {}", track_id);
            Some(path)
        } else {
            None
        }
    }

    /// Save replay-gain metadata as a tiny sidecar file next to the cached audio.
    ///
    /// The sidecar uses the same hash key as the audio file but with a `.rg`
    /// extension, containing the dB value as plain ASCII (e.g. `"-7.4"`).
    pub fn save_replay_gain(&self, track_id: &str, replay_gain_db: f32) {
        let key = self.audio_cache_key(track_id);
        let path = self.audio_cache.hashed_path(&key, "rg");
        let data = format!("{}", replay_gain_db);
        if let Err(e) = std::fs::write(&path, data.as_bytes()) {
            warn!(
                "Failed to save replay gain sidecar for track {}: {}",
                track_id, e
            );
        } else {
            debug!(
                "Saved replay gain {:.1} dB for track {} at {:?}",
                replay_gain_db, track_id, path
            );
        }
    }

    /// Load replay-gain metadata from the sidecar file for a cached track.
    pub fn load_replay_gain(&self, track_id: &str) -> Option<f32> {
        let key = self.audio_cache_key(track_id);
        let path = self.audio_cache.hashed_path(&key, "rg");
        match std::fs::read_to_string(&path) {
            Ok(contents) => match contents.trim().parse::<f32>() {
                Ok(db) => {
                    debug!(
                        "Loaded replay gain {:.1} dB for track {} from {:?}",
                        db, track_id, path
                    );
                    Some(db)
                }
                Err(e) => {
                    warn!("Invalid replay gain sidecar for track {}: {}", track_id, e);
                    None
                }
            },
            Err(_) => None,
        }
    }

    /// Get the expected cache path for a track (for saving after download).
    ///
    /// This also pre-emptively evicts old cache entries to make room for the
    /// incoming file based on a conservative size estimate for the current
    /// quality setting.  The estimate doesn't need to be exact — on the next
    /// app startup [`DiskCache::scan_size`] will re-scan the directory and
    /// correct the counter.
    ///
    /// **Note:** the audio decoder writes files directly with `std::fs::write`
    /// (bypassing [`DiskCache::put`]), so the in-memory byte counter drifts
    /// after each download.  The playback handlers call
    /// [`DiskCache::rescan`] on every track transition to reconcile the
    /// counter with reality before the next `reserve_room` runs.
    pub fn audio_cache_path_for(&self, track_id: &str) -> PathBuf {
        let key = self.audio_cache_key(track_id);

        // Size estimates by quality tier (typical 4-minute track).
        // These only need to be in the right ballpark — the rescan on
        // track transition keeps the counter honest, so a moderate
        // over-estimate just means slightly earlier eviction of the
        // oldest file rather than runaway cache growth.
        //
        //   Low      ~3 MB  (96 kbps AAC)
        //   High    ~10 MB  (320 kbps AAC)
        //   Lossless ~25 MB (FLAC 16-bit/44.1 kHz)
        //   HiRes   ~40 MB  (FLAC 24-bit/96 kHz, most common tier)
        let estimated_bytes: u64 = match self.audio_quality {
            AudioQuality::Low => 5 * 1024 * 1024,
            AudioQuality::High => 15 * 1024 * 1024,
            AudioQuality::Lossless => 30 * 1024 * 1024,
            AudioQuality::HiRes => 50 * 1024 * 1024,
        };

        self.audio_cache.reserve_room(estimated_bytes);
        self.audio_cache.hashed_path(&key, "dat")
    }

    /// Get the total audio cache disk usage in bytes.
    ///
    /// Re-scans the directory first so the value reflects files written
    /// directly to the cache path (bypassing [`DiskCache::put`]).
    pub fn audio_cache_size(&self) -> u64 {
        self.audio_cache.rescan();
        self.audio_cache.current_bytes()
    }

    /// Get the audio cache max size in bytes
    pub fn audio_cache_max(&self) -> u64 {
        self.audio_cache.max_bytes()
    }

    /// Update the audio cache size limit at runtime (e.g. from settings).
    pub fn set_audio_cache_max_mb(&mut self, max_mb: u32) {
        self.audio_cache.set_max_mb(max_mb);
    }

    /// Clear the audio cache
    pub fn clear_audio_cache(&self) {
        self.audio_cache.clear();
    }

    /// Get the current authentication state
    pub fn auth_state(&self) -> &AuthState {
        self.auth_manager.state()
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
                                "Token refreshed - expires_in: {}s, remaining: {}s (~{} minutes)",
                                expiry,
                                remaining,
                                remaining / 60
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

            debug!(
                "Token still valid - expires_in: {}s, elapsed: {}s, remaining: {}s (~{} minutes)",
                expiry,
                now.saturating_sub(last_refresh),
                remaining,
                remaining / 60
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
                        "Stored token state - expires_in: {}s (~{} min), elapsed since refresh: {}s (~{} min), remaining: {}s (~{} min)",
                        expiry,
                        expiry / 60,
                        elapsed,
                        elapsed / 60,
                        remaining,
                        remaining / 60
                    );
                }

                // Try to refresh the access token
                match client.refresh_access_token(false).await {
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
                                "Token valid for {}s (~{} minutes) from last refresh",
                                expiry,
                                expiry / 60
                            );
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            let remaining = (last_refresh + expiry).saturating_sub(now);
                            info!(
                                "Token will expire in {}s (~{} minutes)",
                                remaining,
                                remaining / 60
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
                        warn!("Failed to refresh access token: {:?}", e);
                        // Clear invalid credentials
                        let _ = AuthManager::delete_credentials();
                        self.auth_manager.set_state(AuthState::NotAuthenticated);
                        Err(TidalError::SessionExpired)
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
                        "New OAuth token received - expires_in: {}s (~{} minutes)",
                        expiry,
                        expiry / 60
                    );
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let remaining = (last_refresh + expiry).saturating_sub(now);
                    info!(
                        "Token will expire in {}s (~{} minutes)",
                        remaining,
                        remaining / 60
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

                Ok(search_results)
            }
            Err(e) => {
                error!("Search failed: {:?}", e);
                Err(TidalError::RequestFailed(format!("{:?}", e)))
            }
        }
    }

    // ── API response caching helpers ────────────────────────────────────

    /// Save an API response to disk cache as JSON
    fn cache_api_response<T: serde::Serialize>(&self, cache_key: &str, data: &T) {
        match serde_json::to_vec(data) {
            Ok(json) => {
                if let Err(e) = self.api_cache.put_hashed(cache_key, "json", &json) {
                    warn!("Failed to cache API response '{}': {}", cache_key, e);
                } else {
                    debug!("Cached API response '{}' ({} bytes)", cache_key, json.len());
                }
            }
            Err(e) => {
                warn!("Failed to serialize API response '{}': {}", cache_key, e);
            }
        }
    }

    /// Load a cached API response from disk
    fn load_cached_api_response<T: serde::de::DeserializeOwned>(
        &self,
        cache_key: &str,
    ) -> Option<T> {
        self.api_cache
            .get_hashed(cache_key, "json")
            .and_then(|data| {
                serde_json::from_slice(&data)
                    .map_err(|e| {
                        warn!(
                            "Failed to deserialize cached API response '{}': {}",
                            cache_key, e
                        );
                        e
                    })
                    .ok()
            })
    }

    /// Get cached user playlists (returns None if not cached)
    pub fn get_cached_playlists(&self) -> Option<Vec<Playlist>> {
        self.load_cached_api_response("user_playlists")
    }

    /// Get cached user favorite albums (returns None if not cached)
    pub fn get_cached_albums(&self) -> Option<Vec<Album>> {
        self.load_cached_api_response("user_albums")
    }

    /// Get cached user favorite tracks (returns None if not cached)
    pub fn get_cached_favorite_tracks(&self) -> Option<Vec<Track>> {
        self.load_cached_api_response("user_favorite_tracks")
    }

    /// Get cached user mixes (returns None if not cached)
    pub fn get_cached_mixes(&self) -> Option<Vec<Mix>> {
        self.load_cached_api_response("user_mixes")
    }

    /// Get cached followed artists (returns None if not cached)
    pub fn get_cached_followed_artists(&self) -> Option<Vec<Artist>> {
        self.load_cached_api_response("user_followed_artists")
    }

    /// Get cached playlist tracks (returns None if not cached)
    pub fn get_cached_playlist_tracks(&self, playlist_uuid: &str) -> Option<Vec<Track>> {
        let key = format!("playlist_tracks_{}", playlist_uuid);
        self.load_cached_api_response(&key)
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
                // Cache the response for offline/instant startup
                self.cache_api_response("user_playlists", &playlists);
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
                return Err(TidalError::RequestFailed(format!(
                    "HTTP {}: {}",
                    status, body
                )));
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

        // Cache the response for offline/instant startup
        self.cache_api_response("user_favorite_tracks", &all_tracks);
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
                return Err(TidalError::RequestFailed(format!(
                    "HTTP {}: {}",
                    status, body
                )));
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

        // Cache the response for offline/instant startup
        self.cache_api_response("user_albums", &all_albums);
        Ok(all_albums)
    }

    /// Get playlist items (tracks)
    /// Uses custom lenient parsing to handle playlists where artist name may be null
    pub async fn get_playlist_tracks(
        &self,
        playlist_uuid: &str,
        limit: Option<u32>,
        _offset: Option<u32>,
    ) -> TidalResult<Vec<Track>> {
        // Note: playlist_uuid is used as part of the cache key below
        self.ensure_valid_token().await?;

        let ctx = self.auth_context().await?;

        debug!("Getting playlist tracks for: {}", playlist_uuid);

        let http_client = reqwest::Client::new();
        let page_size: u32 = limit.unwrap_or(100);
        let mut offset: u32 = 0;
        let mut all_tracks: Vec<Track> = Vec::new();

        loop {
            let url = format!(
                "https://api.tidal.com/v1/playlists/{}/items?countryCode={}&limit={}&offset={}",
                playlist_uuid, ctx.country_code, page_size, offset
            );

            let response = http_client
                .get(&url)
                .header(AUTHORIZATION, format!("Bearer {}", ctx.access_token))
                .send()
                .await
                .map_err(|e| TidalError::RequestFailed(format!("HTTP request failed: {}", e)))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                error!("Playlist tracks request failed: {} - {}", status, body);
                return Err(TidalError::RequestFailed(format!(
                    "HTTP {}: {}",
                    status, body
                )));
            }

            let body = response.text().await.map_err(|e| {
                TidalError::RequestFailed(format!("Failed to read response: {}", e))
            })?;

            let parsed: ApiPaginatedResponse<ApiItemWrapper<ApiTrackData>> =
                serde_json::from_str(&body)
                    .map_err(|e| TidalError::ParseError(format!("JSON parse error: {}", e)))?;

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
                "Fetched playlist tracks page: {} / {} total",
                all_tracks.len(),
                total
            );

            if page_items == 0 || offset >= total {
                break;
            }
        }

        // Cache the playlist tracks for offline/instant access
        let cache_key = format!("playlist_tracks_{}", playlist_uuid);
        self.cache_api_response(&cache_key, &all_tracks);

        Ok(all_tracks)
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
                // Cache the album tracks for offline/instant access
                let cache_key = format!("album_tracks_{}", album_id);
                self.cache_api_response(&cache_key, &tracks);
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
    ///
    /// If the track has been previously downloaded and cached on disk, returns
    /// a `CachedFile` variant immediately without making any API call.
    pub async fn get_track_playback_url(&self, track_id: &str) -> TidalResult<PlaybackUrl> {
        // Check audio cache first — if we already downloaded this track at this
        // quality, skip the API call entirely and play from disk.
        if let Some(cached_path) = self.get_cached_audio_path(track_id) {
            let replay_gain_db = self.load_replay_gain(track_id);
            info!(
                "Audio cache hit for track {} — playing from {:?} (replay gain: {:?} dB)",
                track_id, cached_path, replay_gain_db
            );
            return Ok(PlaybackUrl::CachedFile(cached_path, replay_gain_db));
        }

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
            return Err(TidalError::RequestFailed(format!(
                "HTTP {}: {}",
                status, body
            )));
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

        // Persist replay gain so cached playback can use it later
        if let Some(rg) = replay_gain_db {
            self.save_replay_gain(track_id, rg);
        }

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
            info!("DASH manifest detected for HiRes quality - writing to cache");
            let preview_len = manifest_str.len().min(500);
            let preview: String = manifest_str.chars().take(preview_len).collect();
            debug!("DASH manifest content:\n{}", preview);

            // Write the DASH manifest to the XDG cache directory
            // (~/.cache/cosmic-applet-mare/dash/)
            let filename = format!("tidal_dash_{}.mpd", track_id);
            let manifest_path = self
                .dash_cache
                .put(&filename, manifest_str.as_bytes())
                .map_err(|e| {
                    TidalError::RequestFailed(format!("Failed to write manifest to cache: {}", e))
                })?;

            info!("DASH manifest written to: {:?}", manifest_path);
            return Ok(PlaybackUrl::DashManifest(manifest_path, replay_gain_db));
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
        self.add_to_favorites("tracks", "trackIds", track_id).await
    }

    /// Remove a track from user's favorites
    pub async fn remove_favorite_track(&self, track_id: &str) -> TidalResult<()> {
        debug!("Removing track {} from favorites", track_id);
        self.remove_from_favorites("tracks", track_id).await
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
    /// Calls `GET /v1/albums/{id}/review?countryCode=…` which returns a JSON
    /// object with a `text` field containing the editorial review.  Many albums
    /// do not have a review, so callers should treat errors as "no review".
    pub async fn get_album_review(&self, album_id: &str) -> TidalResult<String> {
        let ctx = self.auth_context().await?;

        let url = format!(
            "https://api.tidal.com/v1/albums/{}/review?countryCode={}",
            album_id, ctx.country_code
        );

        debug!("Fetching album review for: {}", album_id);

        let http_client = reqwest::Client::new();
        let response = http_client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", ctx.access_token))
            .send()
            .await
            .map_err(|e| TidalError::NetworkError(format!("{:?}", e)))?;

        if !response.status().is_success() {
            debug!(
                "No review for album {} (HTTP {})",
                album_id,
                response.status()
            );
            return Err(TidalError::RequestFailed(format!(
                "HTTP {}",
                response.status()
            )));
        }

        #[derive(Deserialize)]
        struct AlbumReviewResponse {
            text: String,
        }

        let review: AlbumReviewResponse = response
            .json()
            .await
            .map_err(|e| TidalError::ParseError(format!("{:?}", e)))?;

        if review.text.is_empty() {
            return Err(TidalError::RequestFailed(
                "Review text is empty".to_string(),
            ));
        }

        Ok(review.text)
    }

    // =========================================================================
    // Album Favorites
    // =========================================================================

    /// Add an album to user's favorites
    pub async fn add_favorite_album(&self, album_id: &str) -> TidalResult<()> {
        debug!("Adding album {} to favorites", album_id);
        self.add_to_favorites("albums", "albumIds", album_id).await
    }

    /// Remove an album from user's favorites
    pub async fn remove_favorite_album(&self, album_id: &str) -> TidalResult<()> {
        debug!("Removing album {} from favorites", album_id);
        self.remove_from_favorites("albums", album_id).await
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

        let (user_id, access_token) = {
            let client_guard = self.client.lock().await;
            let client = client_guard.as_ref().ok_or(TidalError::NotAuthenticated)?;
            let uid = match client.session.auth.user_id {
                Some(id) => id,
                None => return Ok((None, None, None, None)),
            };
            let token = match client.session.auth.access_token.as_ref() {
                Some(t) => t.clone(),
                None => return Ok((None, None, None, None)),
            };
            (uid, token)
        };

        let http_client = reqwest::Client::new();
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", access_token))
                .map_err(|e| TidalError::RequestFailed(format!("Invalid auth header: {}", e)))?,
        );

        let mut picture_url: Option<String> = None;
        let mut display_name: Option<String> = None;
        let mut first_name: Option<String> = None;
        let mut last_name: Option<String> = None;

        // --- Attempt 1: GET /v1/users/{id} --------------------------------
        // Returns firstName, lastName, and sometimes a picture UUID.
        let url_v1 = format!("https://api.tidal.com/v1/users/{}?countryCode=US", user_id);
        if let Ok(response) = http_client
            .get(&url_v1)
            .headers(headers.clone())
            .send()
            .await
            && response.status().is_success()
            && let Ok(body) = response.text().await
        {
            debug!("User v1 response: {}", body);
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                // Extract firstName / lastName (trim whitespace — TIDAL
                // sometimes returns trailing spaces)
                if let Some(f) = v.get("firstName").and_then(|x| x.as_str()) {
                    let f = f.trim().to_string();
                    if !f.is_empty() {
                        info!("v1 firstName: {:?}", f);
                        first_name = Some(f);
                    }
                }
                if let Some(l) = v.get("lastName").and_then(|x| x.as_str()) {
                    let l = l.trim().to_string();
                    if !l.is_empty() {
                        info!("v1 lastName: {:?}", l);
                        last_name = Some(l);
                    }
                }

                // Picture UUID (direct string, e.g. "abcd-1234-...")
                if let Some(pic_id) = v.get("picture").and_then(|p| p.as_str())
                    && !pic_id.is_empty()
                {
                    let url = Self::uuid_to_cdn_url(pic_id);
                    info!("User profile picture from v1: {}", url);
                    picture_url = Some(url);
                }
            }
        }

        // --- Attempt 2: GET /v2/profiles/{id} -----------------------------
        // Returns name, handle, and picture: { url: "uuid" }.
        let url_v2 = format!("https://api.tidal.com/v2/profiles/{}", user_id);
        if let Ok(response) = http_client
            .get(&url_v2)
            .headers(headers.clone())
            .send()
            .await
        {
            let status = response.status();
            if status.is_success() {
                if let Ok(body) = response.text().await {
                    debug!("User v2 profile response: {}", body);
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                        // Display name — v2 "name" is the user-chosen profile
                        // name (e.g. "Gustavo")
                        if display_name.is_none()
                            && let Some(name) = v.get("name").and_then(|n| n.as_str())
                        {
                            let name = name.trim().to_string();
                            if !name.is_empty() {
                                info!("v2 profile name: {:?}", name);
                                display_name = Some(name);
                            }
                        }

                        // Picture — can be various shapes:
                        //   string UUID: "abcd-1234-..."
                        //   object:      { "url": "abcd-1234-..." }
                        //   object:      { "320x320": "https://..." }
                        if picture_url.is_none() {
                            picture_url = Self::extract_picture_url_from_json(&v);
                        }
                    }
                }
            } else {
                debug!("v2 profile endpoint returned HTTP {}", status);
            }
        }

        if let Some(url) = &picture_url {
            info!("Resolved profile picture URL: {}", url);
        } else {
            info!("No profile picture found for user {}", user_id);
        }

        Ok((picture_url, display_name, first_name, last_name))
    }

    /// Convert a TIDAL image UUID (e.g. "7e58f111-5b1a-492a-aaf1-88fb55ce8a44")
    /// to a CDN URL.
    pub fn uuid_to_cdn_url(uuid: &str) -> String {
        format!(
            "https://resources.tidal.com/images/{}/320x320.jpg",
            uuid.replace('-', "/")
        )
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
                    return Some(Self::uuid_to_cdn_url(url_str));
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
                        return Some(Self::uuid_to_cdn_url(url_val));
                    }
                    // Then try size keys
                    for size_key in &["320x320", "640x640", "750x750", "medium", "large", "small"] {
                        if let Some(url_str) = obj.get(*size_key).and_then(|u| u.as_str())
                            && !url_str.is_empty()
                        {
                            if url_str.starts_with("http") {
                                return Some(url_str.to_string());
                            }
                            return Some(Self::uuid_to_cdn_url(url_str));
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
        // Cache the response for offline/instant startup
        self.cache_api_response("user_mixes", &mixes);
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

    /// Fetch the tracks for a specific mix by its ID.
    ///
    /// Uses the TIDAL v1 API endpoint `GET /v1/mixes/{mix_id}/items`.
    pub async fn get_mix_tracks(&self, mix_id: &str) -> TidalResult<Vec<Track>> {
        self.ensure_valid_token().await?;

        let ctx = self.auth_context().await?;

        info!("Fetching tracks for mix: {}", mix_id);

        let url = format!(
            "https://api.tidal.com/v1/mixes/{}/items?countryCode={}&limit=100",
            mix_id, ctx.country_code
        );

        let http_client = reqwest::Client::new();
        let response = http_client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", ctx.access_token))
            .send()
            .await
            .map_err(|e| TidalError::NetworkError(format!("mix tracks request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Mix tracks request failed: HTTP {} — {}", status, body);
            return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
        }

        let body = response
            .text()
            .await
            .map_err(|e| TidalError::NetworkError(format!("reading mix body: {}", e)))?;

        let parsed: ApiPaginatedResponse<ApiItemWrapper<ApiTrackData>> =
            serde_json::from_str(&body)
                .map_err(|e| TidalError::ParseError(format!("parsing mix tracks: {}", e)))?;

        let tracks: Vec<Track> = parsed
            .items
            .into_iter()
            .filter_map(|w| w.item)
            .map(Track::from)
            .collect();

        info!("Loaded {} tracks for mix {}", tracks.len(), mix_id);
        Ok(tracks)
    }

    // =========================================================================
    // Track Radio
    // =========================================================================

    /// Fetch a "radio" playlist generated from a specific track.
    ///
    /// Uses the TIDAL v1 API endpoint `GET /v1/tracks/{track_id}/radio`.
    /// Returns a list of recommended tracks similar to the seed track.
    pub async fn get_track_radio(
        &self,
        track_id: &str,
        limit: Option<u32>,
    ) -> TidalResult<Vec<Track>> {
        self.ensure_valid_token().await?;

        let ctx = self.auth_context().await?;

        let limit_param = limit.unwrap_or(100);
        info!(
            "Fetching track radio for track {} (limit {})",
            track_id, limit_param
        );

        let url = format!(
            "https://api.tidal.com/v1/tracks/{}/radio?countryCode={}&limit={}",
            track_id, ctx.country_code, limit_param
        );

        let http_client = reqwest::Client::new();
        let response = http_client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", ctx.access_token))
            .send()
            .await
            .map_err(|e| TidalError::NetworkError(format!("track radio request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Track radio request failed: HTTP {} — {}", status, body);
            return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
        }

        let body = response
            .text()
            .await
            .map_err(|e| TidalError::NetworkError(format!("reading radio body: {}", e)))?;

        // The response is a paginated wrapper: { limit, offset, totalNumberOfItems, items: [...] }
        // Each item is a track object directly (no extra "item" wrapper like mixes).
        let parsed: ApiPaginatedResponse<ApiTrackData> = serde_json::from_str(&body)
            .map_err(|e| TidalError::ParseError(format!("parsing track radio: {}", e)))?;

        let tracks: Vec<Track> = parsed.items.into_iter().map(Track::from).collect();

        info!(
            "Loaded {} radio tracks for track {}",
            tracks.len(),
            track_id
        );
        Ok(tracks)
    }

    // =========================================================================
    // Similar Artists
    // =========================================================================

    /// Fetch artists similar to the given artist from TIDAL's recommendation
    /// engine (`/v1/artists/{id}/similar`).
    ///
    /// Returns up to `limit` (default 20) [`Artist`] entries.  The endpoint
    /// is not exposed by `tidlers`, so we hit the REST API directly — same
    /// pattern as [`Self::get_track_radio`].
    pub async fn get_similar_artists(
        &self,
        artist_id: &str,
        limit: Option<u32>,
    ) -> TidalResult<Vec<Artist>> {
        self.ensure_valid_token().await?;

        let ctx = self.auth_context().await?;

        let limit_param = limit.unwrap_or(20);
        info!(
            "Fetching similar artists for artist {} (limit {})",
            artist_id, limit_param
        );

        let url = format!(
            "https://api.tidal.com/v1/artists/{}/similar?countryCode={}&limit={}",
            artist_id, ctx.country_code, limit_param
        );

        let http_client = reqwest::Client::new();
        let response = http_client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", ctx.access_token))
            .send()
            .await
            .map_err(|e| {
                TidalError::NetworkError(format!("similar artists request failed: {}", e))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Similar artists request failed: HTTP {} — {}", status, body);
            return Err(TidalError::RequestFailed(format!("HTTP {}", status)));
        }

        let body = response.text().await.map_err(|e| {
            TidalError::NetworkError(format!("reading similar artists body: {}", e))
        })?;

        #[derive(Deserialize)]
        struct SimilarResponse {
            items: Vec<SimilarArtist>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct SimilarArtist {
            id: u64,
            name: String,
            picture: Option<String>,
            popularity: Option<u32>,
            artist_roles: Option<Vec<SimilarArtistRole>>,
        }
        #[derive(Deserialize)]
        struct SimilarArtistRole {
            category: String,
        }

        let parsed: SimilarResponse = serde_json::from_str(&body)
            .map_err(|e| TidalError::ParseError(format!("parsing similar artists: {}", e)))?;

        let artists: Vec<Artist> = parsed
            .items
            .into_iter()
            .map(|a| Artist {
                id: a.id.to_string(),
                name: a.name,
                picture_url: a.picture.map(|p| Self::uuid_to_cdn_url(&p)),
                bio: None,
                popularity: a.popularity,
                roles: a
                    .artist_roles
                    .unwrap_or_default()
                    .into_iter()
                    .map(|r| r.category)
                    .collect(),
                url: None,
            })
            .collect();

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
                            .map(Self::uuid_to_cdn_url);

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
        // Cache the response for offline/instant startup
        self.cache_api_response("user_followed_artists", &artists);
        Ok(artists)
    }

    /// Follow (add to favorites) an artist by ID.
    ///
    /// Uses the TIDAL v1 API endpoint `PUT /v1/users/{userId}/favorites/artists`.
    pub async fn follow_artist(&self, artist_id: &str) -> TidalResult<()> {
        debug!("Following artist {}", artist_id);
        self.add_to_favorites("artists", "artistIds", artist_id)
            .await
    }

    /// Unfollow (remove from favorites) an artist by ID.
    ///
    /// Uses the TIDAL v1 API endpoint `DELETE /v1/users/{userId}/favorites/artists/{artistId}`.
    pub async fn unfollow_artist(&self, artist_id: &str) -> TidalResult<()> {
        debug!("Unfollowing artist {}", artist_id);
        self.remove_from_favorites("artists", artist_id).await
    }
}
