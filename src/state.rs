// SPDX-License-Identifier: MIT

//! Application state for Maré Player.
//!
//! This module defines the main application model and view state types.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use cosmic::iced::widget::list;
use cosmic::iced::window::Id;
#[cfg(not(feature = "panel-applet"))]
use cosmic::widget::menu::key_bind::KeyBind;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::image_cache::ImageCache;
#[cfg(not(feature = "panel-applet"))]
use crate::menu::TidalMenuAction;
use crate::tidal::auth::DeviceCodeInfo;
use crate::tidal::client::TidalAppClient;
use crate::tidal::models::{Album, Artist, FeedActivity, Mix, Playlist, SearchResults, Track};
use crate::tidal::mpris::{MprisCommand, MprisHandle};
use crate::tidal::play_history::PlayHistory;
use crate::tidal::player::{NowPlaying, PlaybackState, Player};
use crate::views::visualizer::VisualizerState;
use cosmic::widget::image::Handle;

/// Fixed-capacity FIFO cache for decoded RGBA image handles.
///
/// Wraps a [`HashMap`] and a [`VecDeque`] that tracks insertion order.
/// When the cache is full the oldest entry is evicted.  Evicted images
/// are cheap to re-decode from the on-disk [`ImageCache`], so the user
/// never notices.
///
/// [`Deref`] delegates to the inner `HashMap` so read-only access
/// (`.get()`, `.contains_key()`, iteration, borrowing as
/// `&HashMap<…>`) works transparently in view code.
pub(crate) struct HandleCache {
    map: HashMap<String, cosmic::widget::image::Handle>,
    order: VecDeque<String>,
    capacity: usize,
}

impl HandleCache {
    /// Create a new cache that holds at most `capacity` entries.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Insert a handle, evicting the oldest entry if at capacity.
    pub(crate) fn insert(&mut self, key: String, value: cosmic::widget::image::Handle) {
        // If already present just update the value in place.
        if let std::collections::hash_map::Entry::Occupied(mut e) = self.map.entry(key.clone()) {
            e.insert(value);
            return;
        }
        // Evict oldest entries until there is room.
        while self.order.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, value);
    }
}

impl std::ops::Deref for HandleCache {
    type Target = HashMap<String, cosmic::widget::image::Handle>;
    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

/// Main application model holding all state
pub struct AppModel {
    /// Application state which is managed by the COSMIC runtime.
    pub(crate) core: cosmic::Core,
    /// The popup id (only used in `panel-applet` mode; always `None` in standalone).
    #[cfg_attr(not(feature = "panel-applet"), allow(dead_code))]
    pub(crate) popup: Option<Id>,
    /// Configuration data that persists between application runs.
    pub(crate) config: Config,
    /// The TIDAL client
    pub(crate) tidal_client: Arc<Mutex<TidalAppClient>>,
    /// Current view state
    pub(crate) view_state: ViewState,
    /// OAuth device code info (during login flow)
    pub(crate) device_code_info: Option<DeviceCodeInfo>,
    /// Current search query
    pub(crate) search_query: String,
    /// Search results
    pub(crate) search_results: Option<SearchResults>,
    /// User playlists
    pub(crate) user_playlists: Vec<Playlist>,
    /// Cached 2×2 album-art grid thumbnails for playlists (UUID -> image handle)
    pub(crate) playlist_thumbnails: HashMap<String, Handle>,
    /// User favorite albums
    pub(crate) user_albums: Vec<Album>,
    /// User favorite tracks
    pub(crate) user_favorite_tracks: Vec<Track>,
    /// User's personalized mixes (from home feed)
    pub(crate) user_mixes: Vec<Mix>,
    /// User's followed artists (profiles)
    pub(crate) user_followed_artists: Vec<Artist>,
    /// Feed activities (new releases from followed artists)
    pub(crate) feed_activities: Vec<FeedActivity>,
    /// Tracks for the currently selected mix
    pub(crate) selected_mix_tracks: Vec<Track>,
    /// Name of the currently selected mix
    pub(crate) selected_mix_name: Option<String>,
    /// Tracks for the currently selected track radio
    pub(crate) selected_radio_tracks: Vec<Track>,
    /// The seed track that the radio is based on
    pub(crate) selected_radio_source_track: Option<Track>,
    /// The track whose detail/recommendations view is open
    pub(crate) selected_detail_track: Option<Track>,
    /// "More Albums by {Artist}" for the track detail view
    pub(crate) track_detail_artist_albums: Vec<Album>,
    /// Related/similar artists for the track detail view
    pub(crate) track_detail_related_artists: Vec<Artist>,
    /// Related albums (one per similar artist) for the track detail view
    pub(crate) track_detail_related_albums: Vec<Album>,
    /// Currently selected playlist tracks
    pub(crate) selected_playlist_tracks: Vec<Track>,
    /// Currently selected album tracks
    pub(crate) selected_album_tracks: Vec<Track>,
    /// Selected playlist name
    pub(crate) selected_playlist_name: Option<String>,
    /// Selected album info
    pub(crate) selected_album: Option<Album>,
    /// Selected artist info (for artist detail view)
    pub(crate) selected_artist: Option<Artist>,
    /// Selected artist's top tracks
    pub(crate) selected_artist_top_tracks: Vec<Track>,
    /// Selected artist's albums (discography)
    pub(crate) selected_artist_albums: Vec<Album>,
    /// Set of album IDs that are in user's favorites
    pub(crate) favorite_album_ids: HashSet<String>,
    /// Set of artist IDs that the user follows
    pub(crate) followed_artist_ids: HashSet<String>,
    /// Navigation stack for back navigation (push current state before entering detail pages)
    pub(crate) nav_stack: Vec<ViewState>,
    /// Loading state
    pub(crate) is_loading: bool,
    /// Error message to display
    pub(crate) error_message: Option<String>,
    /// Whether we've attempted to restore the session
    pub(crate) session_restore_attempted: bool,
    /// Audio player
    pub(crate) player: Option<Player>,
    /// Current playback state
    pub(crate) playback_state: PlaybackState,
    /// Currently playing track info
    pub(crate) now_playing: Option<NowPlaying>,
    /// Current playback position in seconds
    pub(crate) playback_position: f64,
    /// Playback queue (list of tracks to play)
    pub(crate) playback_queue: Vec<Track>,
    /// Current index in the playback queue
    pub(crate) playback_queue_index: usize,
    /// Shuffle mode enabled
    pub(crate) shuffle_enabled: bool,
    /// Loop/repeat mode (None, Track, Playlist)
    pub(crate) loop_status: crate::tidal::mpris::LoopStatus,
    /// Current playback context (playlist name, album name, or "Favorites", etc.)
    pub(crate) playback_context: Option<String>,
    /// Image cache for album art
    pub(crate) image_cache: ImageCache,
    /// Decoded RGBA image handles, FIFO-evicted at 512 entries.
    pub(crate) loaded_images: HandleCache,
    /// URLs currently being loaded (to avoid duplicate requests)
    pub(crate) pending_image_loads: HashSet<String>,
    /// Set of track IDs that are in user's favorites
    pub(crate) favorite_track_ids: HashSet<String>,
    /// MPRIS D-Bus handle for external media control
    pub(crate) mpris_handle: Option<MprisHandle>,
    /// Receiver for MPRIS commands (wrapped in `Arc<Mutex>` for sharing)
    pub(crate) mpris_command_rx:
        Option<Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<MprisCommand>>>>,
    /// Search debounce version counter (incremented on each keystroke)
    pub(crate) search_debounce_version: u64,
    /// Audio visualizer state
    pub(crate) visualizer_state: VisualizerState,
    /// Download/buffering progress (0.0 to 1.0) for the current loading track
    pub(crate) loading_progress: f32,
    /// Pending seek position (for debouncing slider drags)
    pub(crate) pending_seek: Option<f64>,
    /// Seek debounce version counter
    pub(crate) seek_debounce_version: u64,
    /// Current volume level (0.0 to 1.0)
    pub(crate) volume_level: f32,
    /// Whether to show the volume bar overlay (panel-applet scroll-wheel indicator)
    pub(crate) show_volume_bar: bool,
    /// When the volume bar was last shown (for auto-hide)
    pub(crate) volume_bar_shown_at: Option<Instant>,
    /// Whether the volume popup (vertical slider) is open (standalone mode only)
    #[cfg(not(feature = "panel-applet"))]
    pub(crate) show_volume_popup: bool,
    /// Local play history (most-recently-played tracks, persisted to disk)
    pub(crate) play_history: PlayHistory,
    /// Virtual-list content for the active track-list view (only visible items are rendered).
    pub(crate) track_list_content: list::Content<Track>,
    /// Shared reference to the same tracks, for `PlayTrackList`/`ShufflePlay` messages.
    pub(crate) track_list_arc: Arc<[Track]>,
    /// Whether the history search/filter bar is visible
    pub(crate) history_filter_visible: bool,
    /// Current filter query for the history view (local, client-side only)
    pub(crate) history_filter_query: String,
    /// Whether the favorite tracks search/filter bar is visible
    pub(crate) favorite_tracks_filter_visible: bool,
    /// Current filter query for the favorite tracks view (local, client-side only)
    pub(crate) favorite_tracks_filter_query: String,
    /// Current window width in logical pixels (updated on resize).
    /// Used to scale text truncation limits proportionally.
    pub(crate) window_width: f32,
    /// Keyboard shortcut bindings for the header menu bar (standalone mode only).
    #[cfg(not(feature = "panel-applet"))]
    pub(crate) menu_key_binds: HashMap<KeyBind, TidalMenuAction>,
}

impl AppModel {
    /// Populate the virtual track list used by the currently visible view.
    ///
    /// This sets up both the `Content<Track>` (for the iced virtual `List`
    /// widget) and the `Arc<[Track]>` (for `PlayTrackList` / `ShufflePlay`
    /// messages). Call when entering a track-list view or when its data changes.
    pub(crate) fn set_track_list(&mut self, tracks: Vec<Track>) {
        self.track_list_arc = tracks.clone().into();
        self.track_list_content = tracks.into_iter().collect();
    }
}

/// Current view state for the popup
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewState {
    /// Initial loading state
    Loading,
    /// Login required - show auth prompt
    Login,
    /// Waiting for OAuth completion
    AwaitingOAuth,
    /// Main collection view with categories
    Main,
    /// Search view
    Search,
    /// Mixes & Radio list view
    Mixes,
    /// Mix detail view (showing tracks in a mix)
    MixDetail,
    /// Playlists list view
    Playlists,
    /// Playlist detail view
    PlaylistDetail,
    /// Albums list view
    Albums,
    /// Album detail view
    AlbumDetail,
    /// Artist detail view
    ArtistDetail,
    /// Track radio view (similar tracks based on a seed track)
    TrackRadio,
    /// Track detail view (recommendations: more albums by artist, related albums, related artists)
    TrackDetail,
    /// Favorite tracks view
    FavoriteTracks,
    /// Feed view (new releases from followed artists)
    Feed,
    /// Play history view (locally tracked recently played tracks)
    History,
    /// Followed artists (Profiles) view
    Profiles,
    /// Settings view
    Settings,
    /// Share prompt dialog (track_id, track_title, album_id, album_title)
    SharePrompt(String, String, Option<String>, Option<String>),
}
