// SPDX-License-Identifier: MIT

//! Application messages for Maré Player.
//!
//! This module defines all the messages that can be sent to update the application state.

use std::sync::Arc;

use cosmic::iced::core::window::Screenshot;
use cosmic::iced::window::Id;
use cosmic::surface;
use tokio::sync::Mutex;

use crate::config::{AudioQuality, Config};
use crate::tidal::auth::DeviceCodeInfo;
use crate::tidal::client::PlaybackUrl;
use crate::tidal::models::{Album, Artist, FeedActivity, Mix, Playlist, SearchResults, Track};
use crate::tidal::mpris::{MprisCommand, MprisHandle};

/// Result type for MPRIS service initialization.
///
/// Carries the handle for updating MPRIS metadata/state and a receiver
/// for playback commands sent by external media controllers.
pub type MprisStartResult = Result<
    (
        MprisHandle,
        Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<MprisCommand>>>,
    ),
    String,
>;

/// Application messages for state updates
#[derive(Debug, Clone)]
pub enum Message {
    // Popup management (panel-applet mode) / window lifecycle (standalone mode)
    /// Toggle the popup window visibility (applet) or raise the window (standalone).
    TogglePopup,
    /// Popup window was closed (applet only; no-op in standalone mode).
    PopupClosed(Id),

    // Subscription/background events
    /// Subscription channel event (used for startup)
    SubscriptionChannel,
    /// Configuration was updated
    UpdateConfig(Config),

    // Authentication
    /// Start the login flow
    StartLogin,
    /// OAuth device code info received
    LoginOAuthReceived(Result<DeviceCodeInfo, String>),
    /// Open the OAuth URL in browser
    OpenOAuthUrl,
    /// OAuth flow completed
    OAuthComplete(Result<(), String>),
    /// Session restore attempted
    SessionRestored(Result<bool, String>),
    /// Log out the user
    Logout,

    // Navigation
    /// Show the main collection view
    ShowMain,
    /// Show the search view
    ShowSearch,
    /// Show the settings view
    ShowSettings,
    /// Show the play history view
    ShowHistory,
    /// Toggle the history search/filter bar visibility
    ToggleHistoryFilter,
    /// History filter query text changed (local client-side filter)
    HistoryFilterChanged(String),
    /// Toggle the favorite tracks search/filter bar visibility
    ToggleFavoriteTracksFilter,
    /// Favorite tracks filter query text changed (local client-side filter)
    FavoriteTracksFilterChanged(String),
    /// Show the mixes & radio view
    ShowMixes,
    /// Show the feed view (new releases from followed artists)
    ShowFeed,
    /// Show the playlists list
    ShowPlaylists,
    /// Show the albums list
    ShowAlbums,
    /// Show favorite tracks
    ShowFavoriteTracks,
    /// Show the followed artists (profiles) view
    ShowProfiles,
    /// Show mix detail (mix_id, mix_name)
    ShowMixDetail(String, String),
    /// Show playlist detail (playlist_uuid, playlist_name)
    ShowPlaylistDetail(String, String),
    /// Show album detail (from favorites list where we already have the Album)
    ShowAlbumDetail(Album),
    /// Show album detail by ID (from now-playing bar or artist view)
    ShowAlbumDetailById(String),
    /// Show artist detail by ID (from now-playing bar or track list)
    ShowArtistDetail(String),
    /// Navigate back by popping the navigation stack
    NavigateBack,

    // Feed (new releases from followed artists)
    /// Load feed activities
    LoadFeed,
    /// Feed activities loaded
    FeedLoaded(Result<Vec<FeedActivity>, String>),

    // Mixes & Radio
    /// Load user mixes from home feed
    LoadMixes,
    /// Mixes loaded from home feed
    MixesLoaded(Result<Vec<Mix>, String>),
    /// Mix tracks loaded
    MixTracksLoaded(Result<Vec<Track>, String>),

    // Track Radio
    /// Show track radio view for a specific track
    ShowTrackRadio(Track),
    /// Track radio tracks loaded
    TrackRadioLoaded(Result<Vec<Track>, String>),

    // Track Detail (recommendations from a track)
    /// Show track detail view (more albums by artist, related albums, related artists)
    ShowTrackDetail(Track),
    /// More albums by the track's artist loaded
    TrackDetailArtistAlbumsLoaded(Result<Vec<Album>, String>),
    /// Similar/related artists loaded
    TrackDetailRelatedArtistsLoaded(Result<Vec<Artist>, String>),
    /// Related albums (one per similar artist) loaded
    TrackDetailRelatedAlbumsLoaded(Result<Vec<Album>, String>),

    // Profiles (followed artists)
    /// Load followed artists
    LoadProfiles,
    /// Followed artists loaded
    ProfilesLoaded(Result<Vec<Artist>, String>),

    // Search
    /// Search query text changed
    SearchQueryChanged(String),
    /// Perform search immediately
    PerformSearch,
    /// Perform search after debounce (version number for debouncing)
    PerformSearchDebounced(u64),
    /// Search completed with results
    SearchComplete(Result<SearchResults, String>),

    // Playlists
    /// Load user playlists
    LoadPlaylists,
    /// Playlists loaded
    PlaylistsLoaded(Result<Vec<Playlist>, String>),
    /// Playlist tracks loaded
    PlaylistTracksLoaded(Result<Vec<Track>, String>),
    /// Kick off background generation of 2×2 album-art grid thumbnails for all playlists
    GeneratePlaylistThumbnails,
    /// A playlist's composite grid thumbnail has been generated (uuid, width, height, rgba_pixels)
    PlaylistThumbnailGenerated(String, u32, u32, Vec<u8>),

    // Albums
    /// Load user albums
    LoadAlbums,
    /// Albums loaded
    AlbumsLoaded(Result<Vec<Album>, String>),
    /// Album tracks loaded
    AlbumTracksLoaded(Result<Vec<Track>, String>),
    /// Album info loaded (when navigating by ID)
    AlbumInfoLoaded(Result<Album, String>),
    /// Album review text loaded (fetched separately; many albums have none)
    AlbumReviewLoaded(Result<String, String>),

    // Artist detail
    /// Artist info loaded (full detail with bio, picture, etc.)
    ArtistInfoLoaded(Result<Artist, String>),
    /// Artist top tracks loaded
    ArtistTopTracksLoaded(Result<Vec<Track>, String>),
    /// Artist albums (discography) loaded
    ArtistAlbumsLoaded(Result<Vec<Album>, String>),

    // Favorite tracks
    /// Load favorite tracks
    LoadFavoriteTracks,
    /// Favorite tracks loaded
    FavoriteTracksLoaded(Result<Vec<Track>, String>),
    /// Toggle favorite status for a track
    ToggleFavorite(Track),
    /// Result of toggling favorite (track, is_now_favorite)
    FavoriteToggled(Result<(Track, bool), String>),
    /// Toggle favorite status for an album
    ToggleFavoriteAlbum(Album),
    /// Result of toggling album favorite (album, is_now_favorite)
    FavoriteAlbumToggled(Result<(Album, bool), String>),
    /// Toggle follow status for an artist
    ToggleFollowArtist(Artist),
    /// Result of toggling artist follow (artist, is_now_followed)
    FollowArtistToggled(Result<(Artist, bool), String>),

    // Track actions
    /// Play a list of tracks starting from a specific index, with optional context (playlist/album name)
    PlayTrackList(Arc<[Track]>, usize, Option<String>),
    /// Shuffle and play a list of tracks, with optional context (playlist/album name)
    ShufflePlay(Arc<[Track]>, Option<String>),
    /// Play next track in queue
    NextTrack,
    /// Play previous track in queue
    PreviousTrack,
    /// Toggle shuffle mode (used by MPRIS SetShuffle; not directly used in UI)
    ToggleShuffle,
    /// Set loop/repeat mode to a specific value (used by MPRIS SetLoopStatus)
    SetLoopStatus(crate::tidal::mpris::LoopStatus),
    /// Cycle through playback modes: Off → Shuffle → Repeat All → Repeat Track → Off.
    /// This is the single UI-facing action that manages both shuffle and loop status.
    CyclePlaybackMode,

    // Playback control
    /// Toggle play/pause
    TogglePlayPause,
    /// Stop playback
    StopPlayback,
    /// Seek to position (0.0 to 100.0 percent) - debounced
    SeekTo(f64),
    /// Execute debounced seek (version)
    SeekDebounced(u64),
    /// Playback URL received for track
    PlaybackUrlReceived(Result<(Track, PlaybackUrl), String>),
    /// Preload the next track for gapless playback
    PreloadNextTrack,
    /// Preload URL received for gapless playback
    PreloadUrlReceived(Result<(Track, PlaybackUrl), String>),
    /// Gapless transition occurred — the preloaded track started playing
    GaplessTransition,
    /// Periodic playback tick — updates position, processes engine events,
    /// and hides the volume bar after a timeout.
    PlaybackTick,

    // Error handling
    /// Clear the current error message
    ClearError,

    // Image loading
    /// Image loaded (url, width, height, rgba_pixels)
    ImageLoaded(String, u32, u32, Vec<u8>),
    /// Request to load an image (url)
    LoadImage(String),

    // Sharing (song.link integration)
    /// Show share prompt for current track
    ShowSharePrompt(Track),
    /// Share a track via song.link (track_id, track_title)
    ShareTrack(String, String),
    /// Share an album via song.link (album_id, album_title)
    ShareAlbum(String, String),
    /// Cancel share dialog
    CancelShare,
    /// Result of generating a song.link URL
    ShareLinkGenerated(Result<String, String>),

    // Settings
    /// Set audio quality preference
    SetAudioQuality(AudioQuality),
    /// Set maximum audio cache size in megabytes
    SetAudioCacheMaxMb(u32),
    /// Clear the audio cache (downloaded songs)
    ClearAudioCache,
    /// Clear the local play history
    ClearHistory,

    // MPRIS D-Bus integration
    /// MPRIS service started
    MprisServiceStarted(MprisStartResult),
    /// MPRIS command received
    MprisCommand(MprisCommand),

    // Volume control
    /// Adjust volume by delta (positive = up, negative = down)
    AdjustVolume(f32),
    /// Set volume to an absolute level (0.0 to 1.0)
    SetVolume(f32),
    /// Toggle the volume popup (standalone mode only)
    ToggleVolumePopup,
    /// Close the volume popup (standalone mode only, e.g. click-away)
    CloseVolumePopup,

    // Screenshot
    /// Capture a screenshot of the applet window (Ctrl+Shift+S).
    TakeScreenshot,
    /// A screenshot has been captured; encode it to PNG and save to disk.
    ScreenshotCaptured(Screenshot),

    // Debug / API discovery
    /// Probe the TIDAL Feed page endpoint and dump the raw JSON structure.
    ProbeFeedPage,
    /// Result of the feed page probe.
    FeedProbeResult(Result<String, String>),

    // Wayland surface actions (used by responsive_menu_bar for popup menus)
    /// Forward a surface action to the COSMIC runtime (menu popups on Wayland).
    Surface(surface::Action),
}
