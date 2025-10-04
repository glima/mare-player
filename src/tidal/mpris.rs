// SPDX-License-Identifier: MIT

//! MPRIS D-Bus interface for media player control.
//!
//! This module implements the MPRIS (Media Player Remote Interfacing Specification)
//! D-Bus interface, allowing external applications to:
//! - Control playback (play, pause, next, previous, stop)
//! - Query current track metadata (title, artist, album, artwork)
//! - Monitor playback state changes
//!
//! The interface is exposed at `org.mpris.MediaPlayer2.Mare` on the session bus.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, watch};
use tracing::{debug, info, warn};
use zbus::{
    Connection,
    connection::Builder,
    interface,
    names::InterfaceName,
    object_server::SignalEmitter,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

/// MPRIS player name (appears after org.mpris.MediaPlayer2.)
const MPRIS_NAME: &str = "Mare";

/// D-Bus object path where all MPRIS interfaces are served.
const MPRIS_OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";

/// Sentinel path meaning "no track is currently selected".
const MPRIS_NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

/// Prefix for individual track object paths (`{PREFIX}/{id}_{index}`).
const MPRIS_TRACK_PREFIX: &str = "/org/mpris/MediaPlayer2/track";

/// Commands that can be sent from D-Bus to the player
#[derive(Debug, Clone)]
pub enum MprisCommand {
    /// Play or resume playback
    Play,
    /// Pause playback
    Pause,
    /// Toggle play/pause
    PlayPause,
    /// Stop playback
    Stop,
    /// Skip to next track
    Next,
    /// Skip to previous track
    Previous,
    /// Seek to absolute position (in microseconds)
    Seek(i64),
    /// Set position (track_id, position in microseconds)
    SetPosition(String, i64),
    /// Open a URI for playback
    OpenUri(String),
    /// Jump to a specific track in the tracklist by track ID
    GoTo(String),
    /// Set shuffle mode
    SetShuffle(bool),
    /// Set loop/repeat mode
    SetLoopStatus(LoopStatus),
    /// Set volume (0.0 to 1.0)
    SetVolume(f64),
    /// Raise the application window
    Raise,
    /// Quit the application
    Quit,
}

/// Playback status for MPRIS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MprisPlaybackStatus {
    #[default]
    Stopped,
    Playing,
    Paused,
}

impl MprisPlaybackStatus {
    fn as_str(&self) -> &'static str {
        match self {
            MprisPlaybackStatus::Stopped => "Stopped",
            MprisPlaybackStatus::Playing => "Playing",
            MprisPlaybackStatus::Paused => "Paused",
        }
    }
}

/// MPRIS loop/repeat status.
///
/// Per the MPRIS spec, `LoopStatus` controls how the player handles reaching
/// the end of a track or playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopStatus {
    /// Playback stops at the end of the queue (default).
    #[default]
    None,
    /// The current track repeats forever.
    Track,
    /// The queue loops back to the beginning.
    Playlist,
}

impl LoopStatus {
    /// Convert to the MPRIS D-Bus string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            LoopStatus::None => "None",
            LoopStatus::Track => "Track",
            LoopStatus::Playlist => "Playlist",
        }
    }

    /// Parse from an MPRIS D-Bus string. Returns `None` for unrecognised values.
    pub fn parse_mpris(s: &str) -> Option<Self> {
        match s {
            "None" => Some(LoopStatus::None),
            "Track" => Some(LoopStatus::Track),
            "Playlist" => Some(LoopStatus::Playlist),
            _ => None,
        }
    }
}

/// Track metadata for MPRIS
#[derive(Debug, Clone, Default)]
pub struct MprisMetadata {
    /// Unique track ID (D-Bus object path format)
    pub track_id: String,
    /// Track title
    pub title: String,
    /// Artist name(s)
    pub artists: Vec<String>,
    /// Album name
    pub album: Option<String>,
    /// Album artist(s)
    pub album_artists: Vec<String>,
    /// Track duration in microseconds
    pub length_us: i64,
    /// Cover art URL
    pub art_url: Option<String>,
    /// Track number on album
    pub track_number: Option<i32>,
    /// Disc number
    pub disc_number: Option<i32>,
}

impl MprisMetadata {
    /// Build an `MprisMetadata` from a domain `Track`.
    pub fn from_track(track: &crate::tidal::models::Track) -> Self {
        Self {
            track_id: track.id.clone(),
            title: track.title.clone(),
            artists: vec![track.artist_name.clone()],
            album: track.album_name.clone(),
            album_artists: vec![track.artist_name.clone()],
            length_us: (track.duration as i64) * 1_000_000,
            art_url: track.cover_url.clone(),
            track_number: if track.track_number > 0 {
                Some(track.track_number as i32)
            } else {
                None
            },
            disc_number: None,
        }
    }

    /// Convert to D-Bus metadata dictionary
    fn to_dbus_metadata(&self) -> HashMap<String, OwnedValue> {
        let mut map = HashMap::new();

        // Track ID (required)
        let track_path = if self.track_id.is_empty() {
            MPRIS_NO_TRACK.to_string()
        } else {
            format!("{}/{}", MPRIS_TRACK_PREFIX, self.track_id)
        };
        if let Ok(path) = ObjectPath::try_from(track_path.as_str())
            && let Ok(owned) = Value::ObjectPath(path).try_into()
        {
            map.insert("mpris:trackid".to_string(), owned);
        }

        // Length (duration in microseconds)
        if self.length_us > 0
            && let Ok(owned) = Value::I64(self.length_us).try_into()
        {
            map.insert("mpris:length".to_string(), owned);
        }

        // Art URL
        if let Some(ref url) = self.art_url
            && let Ok(owned) = Value::Str(url.as_str().into()).try_into()
        {
            map.insert("mpris:artUrl".to_string(), owned);
        }

        // Title
        if !self.title.is_empty()
            && let Ok(owned) = Value::Str(self.title.as_str().into()).try_into()
        {
            map.insert("xesam:title".to_string(), owned);
        }

        // Artists
        if !self.artists.is_empty() {
            let artists: Vec<&str> = self.artists.iter().map(|s| s.as_str()).collect();
            if let Ok(owned) = Value::Array(artists.into()).try_into() {
                map.insert("xesam:artist".to_string(), owned);
            }
        }

        // Album
        if let Some(ref album) = self.album
            && let Ok(owned) = Value::Str(album.as_str().into()).try_into()
        {
            map.insert("xesam:album".to_string(), owned);
        }

        // Album artists
        if !self.album_artists.is_empty() {
            let artists: Vec<&str> = self.album_artists.iter().map(|s| s.as_str()).collect();
            if let Ok(owned) = Value::Array(artists.into()).try_into() {
                map.insert("xesam:albumArtist".to_string(), owned);
            }
        }

        // Track number
        if let Some(num) = self.track_number
            && let Ok(owned) = Value::I32(num).try_into()
        {
            map.insert("xesam:trackNumber".to_string(), owned);
        }

        // Disc number
        if let Some(num) = self.disc_number
            && let Ok(owned) = Value::I32(num).try_into()
        {
            map.insert("xesam:discNumber".to_string(), owned);
        }

        map
    }
}

/// Build the D-Bus object path for a track ID.
///
/// The spec requires unique object paths within the tracklist scope.
/// If the same TIDAL track appears twice in the queue we disambiguate
/// by appending the queue position.
pub fn track_object_path(track_id: &str, queue_index: usize) -> String {
    format!("{}/{}_{}", MPRIS_TRACK_PREFIX, track_id, queue_index)
}

/// A single entry in the MPRIS tracklist.
#[derive(Debug, Clone)]
pub struct MprisTrackEntry {
    /// D-Bus object path (unique within the tracklist)
    pub object_path: String,
    /// Full metadata for this track
    pub metadata: MprisMetadata,
}

/// Current player state sent to MPRIS service
#[derive(Debug, Clone, Default)]
pub struct MprisState {
    /// Current playback status
    pub playback_status: MprisPlaybackStatus,
    /// Current track metadata
    pub metadata: MprisMetadata,
    /// Current position in microseconds
    pub position_us: i64,
    /// Current volume (0.0 to 1.0)
    pub volume: f64,
    /// Whether shuffle is enabled
    pub shuffle: bool,
    /// Current loop/repeat status
    pub loop_status: LoopStatus,
    /// Whether the player can go to next track
    pub can_go_next: bool,
    /// Whether the player can go to previous track
    pub can_go_previous: bool,
    /// Whether the player can play
    pub can_play: bool,
    /// Whether the player can pause
    pub can_pause: bool,
    /// Whether the player can seek
    pub can_seek: bool,
    /// The current tracklist (queue) exposed over MPRIS.
    ///
    /// Built from `playback_queue` — each entry has a unique object path
    /// and full metadata so the TrackList interface can serve them.
    pub tracklist: Vec<MprisTrackEntry>,
    /// Object path of the currently playing track in the tracklist.
    pub current_track_path: String,
}

/// Handle for communicating with the MPRIS service
#[derive(Clone, Debug)]
pub struct MprisHandle {
    /// Sender for state updates
    state_tx: watch::Sender<MprisState>,
    /// Connection to D-Bus (for emitting signals)
    connection: Arc<Mutex<Option<Connection>>>,
}

impl MprisHandle {
    /// Update the MPRIS state (will emit PropertiesChanged signals)
    pub async fn update_state(&self, state: MprisState) {
        // Snapshot the old tracklist so we can detect changes
        let old_tracklist: Vec<String> = self
            .state_tx
            .borrow()
            .tracklist
            .iter()
            .map(|e| e.object_path.clone())
            .collect();

        // Send state update
        if self.state_tx.send(state.clone()).is_err() {
            warn!("Failed to send MPRIS state update");
        }

        // Emit PropertiesChanged signal
        if let Some(conn) = self.connection.lock().await.as_ref() {
            let object_server = conn.object_server();

            // Get interface reference and emit Player property changes
            if let Ok(iface_ref) = object_server
                .interface::<_, MprisPlayer>(MPRIS_OBJECT_PATH)
                .await
            {
                let emitter = iface_ref.signal_emitter();
                // Emit property changes
                if let Err(e) = MprisPlayer::emit_properties_changed(emitter, &state).await {
                    debug!("Failed to emit Player properties changed: {}", e);
                }
            }

            // Check if the tracklist changed and emit TrackListReplaced
            let new_tracklist: Vec<String> = state
                .tracklist
                .iter()
                .map(|e| e.object_path.clone())
                .collect();

            if new_tracklist != old_tracklist
                && let Ok(iface_ref) = object_server
                    .interface::<_, MprisTrackList>(MPRIS_OBJECT_PATH)
                    .await
            {
                let emitter = iface_ref.signal_emitter();
                // Build the track path list and current track path
                let track_paths: Vec<ObjectPath<'_>> = new_tracklist
                    .iter()
                    .filter_map(|p| ObjectPath::try_from(p.as_str()).ok())
                    .collect();

                let current = if state.current_track_path.is_empty() {
                    MPRIS_NO_TRACK
                } else {
                    &state.current_track_path
                };

                if let Ok(current_path) = ObjectPath::try_from(current)
                    && let Err(e) =
                        MprisTrackList::track_list_replaced(emitter, track_paths, current_path)
                            .await
                {
                    debug!("Failed to emit TrackListReplaced: {}", e);
                }
            }
        }
    }

    /// Update playback position (call frequently during playback)
    pub async fn update_position(&self, position_us: i64) {
        self.state_tx.send_modify(|state| {
            state.position_us = position_us;
        });
    }
}

/// The main MPRIS MediaPlayer2 interface
struct MprisRoot {
    // Used via self.command_tx.send() in interface methods below, but compiler can't see through #[interface] macro
    #[allow(dead_code)]
    command_tx: mpsc::UnboundedSender<MprisCommand>,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl MprisRoot {
    /// Brings the media player's user interface to the front
    fn raise(&self) {
        debug!("MPRIS: Raise called");
        let _ = self.command_tx.send(MprisCommand::Raise);
    }

    /// Causes the media player to stop running
    fn quit(&self) {
        debug!("MPRIS: Quit called");
        let _ = self.command_tx.send(MprisCommand::Quit);
    }

    /// Whether the media player can quit
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }

    /// Whether fullscreen mode can be set
    #[zbus(property)]
    fn fullscreen(&self) -> bool {
        false
    }

    /// Whether fullscreen can be set
    #[zbus(property)]
    fn can_set_fullscreen(&self) -> bool {
        false
    }

    /// Whether the player can be raised
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }

    /// Whether the player has a tracklist
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        true
    }

    /// The player identity
    #[zbus(property)]
    fn identity(&self) -> &str {
        "Maré Player"
    }

    /// The desktop entry name (matches `APP_ID` in `app.rs`)
    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        #[cfg(feature = "panel-applet")]
        {
            "io.github.cosmic-applet-mare"
        }
        #[cfg(not(feature = "panel-applet"))]
        {
            "io.github.mare-player"
        }
    }

    /// Supported URI schemes
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<&str> {
        vec!["tidal", "https"]
    }

    /// Supported MIME types
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<&str> {
        vec!["audio/flac", "audio/aac", "audio/mp4"]
    }
}

/// The MPRIS Player interface
struct MprisPlayer {
    command_tx: mpsc::UnboundedSender<MprisCommand>,
    state_rx: watch::Receiver<MprisState>,
}

impl MprisPlayer {
    /// Emit PropertiesChanged signal for changed properties
    async fn emit_properties_changed(
        emitter: &SignalEmitter<'_>,
        state: &MprisState,
    ) -> zbus::Result<()> {
        use zbus::fdo::Properties;

        let mut changed: HashMap<&str, Value<'_>> = HashMap::new();

        // Always include these in the changed properties
        changed.insert(
            "PlaybackStatus",
            Value::Str(state.playback_status.as_str().into()),
        );

        // Convert metadata to owned value
        let metadata_map = state.metadata.to_dbus_metadata();
        // Create a Dict from the metadata - we need to convert to borrowed values
        let metadata_borrowed: HashMap<&str, Value<'_>> = HashMap::new();
        // For simplicity, skip metadata in the changed signal for now
        // The property getter will return the correct value
        let _ = metadata_map;
        let _ = metadata_borrowed;

        changed.insert("Volume", Value::F64(state.volume));
        changed.insert("Shuffle", Value::Bool(state.shuffle));
        changed.insert("LoopStatus", Value::Str(state.loop_status.as_str().into()));
        changed.insert("CanGoNext", Value::Bool(state.can_go_next));
        changed.insert("CanGoPrevious", Value::Bool(state.can_go_previous));
        changed.insert("CanPlay", Value::Bool(state.can_play));
        changed.insert("CanPause", Value::Bool(state.can_pause));
        changed.insert("CanSeek", Value::Bool(state.can_seek));

        // Emit the signal
        let interface_name = InterfaceName::try_from("org.mpris.MediaPlayer2.Player")
            .map_err(|e| zbus::Error::Failure(format!("Invalid interface name: {}", e)))?;
        let invalidated: Cow<'_, [&str]> = Cow::Borrowed(&[]);

        Properties::properties_changed(emitter, interface_name, changed, invalidated).await
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl MprisPlayer {
    /// Skip to the next track
    fn next(&self) {
        debug!("MPRIS: Next called");
        let _ = self.command_tx.send(MprisCommand::Next);
    }

    /// Skip to the previous track
    fn previous(&self) {
        debug!("MPRIS: Previous called");
        let _ = self.command_tx.send(MprisCommand::Previous);
    }

    /// Pause playback
    fn pause(&self) {
        debug!("MPRIS: Pause called");
        let _ = self.command_tx.send(MprisCommand::Pause);
    }

    /// Toggle play/pause
    fn play_pause(&self) {
        debug!("MPRIS: PlayPause called");
        let _ = self.command_tx.send(MprisCommand::PlayPause);
    }

    /// Stop playback
    fn stop(&self) {
        debug!("MPRIS: Stop called");
        let _ = self.command_tx.send(MprisCommand::Stop);
    }

    /// Start or resume playback
    fn play(&self) {
        debug!("MPRIS: Play called");
        let _ = self.command_tx.send(MprisCommand::Play);
    }

    /// Seek relative to current position
    fn seek(&self, offset: i64) {
        debug!("MPRIS: Seek called with offset {}", offset);
        let current_pos = self.state_rx.borrow().position_us;
        let new_pos = (current_pos + offset).max(0);
        let _ = self.command_tx.send(MprisCommand::Seek(new_pos));
    }

    /// Set absolute position
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        debug!(
            "MPRIS: SetPosition called: track={}, pos={}",
            track_id, position
        );
        let track_id_str = track_id
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
        let _ = self
            .command_tx
            .send(MprisCommand::SetPosition(track_id_str, position));
    }

    /// Open a URI
    fn open_uri(&self, uri: &str) {
        debug!("MPRIS: OpenUri called: {}", uri);
        let _ = self.command_tx.send(MprisCommand::OpenUri(uri.to_string()));
    }

    /// Signal emitted when track changes
    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    /// Current playback status
    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.state_rx.borrow().playback_status.as_str().to_string()
    }

    /// Loop/repeat status
    #[zbus(property)]
    fn loop_status(&self) -> String {
        self.state_rx.borrow().loop_status.as_str().to_string()
    }

    /// Set loop status (not supported)
    #[zbus(property)]
    fn set_loop_status(&self, status: &str) -> zbus::Result<()> {
        if let Some(ls) = LoopStatus::parse_mpris(status) {
            debug!("MPRIS: SetLoopStatus called: {:?}", ls);
            let _ = self.command_tx.send(MprisCommand::SetLoopStatus(ls));
            Ok(())
        } else {
            Err(zbus::Error::Failure(format!(
                "Invalid loop status: {status}"
            )))
        }
    }

    /// Playback rate
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    /// Set playback rate (not supported)
    #[zbus(property)]
    fn set_rate(&self, _rate: f64) -> zbus::Result<()> {
        Ok(()) // Silently ignore
    }

    /// Shuffle mode
    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.state_rx.borrow().shuffle
    }

    /// Set shuffle mode
    #[zbus(property)]
    fn set_shuffle(&self, shuffle: bool) -> zbus::Result<()> {
        debug!("MPRIS: SetShuffle called: {}", shuffle);
        let _ = self.command_tx.send(MprisCommand::SetShuffle(shuffle));
        Ok(())
    }

    /// Current track metadata
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.state_rx.borrow().metadata.to_dbus_metadata()
    }

    /// Current volume (0.0 to 1.0)
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.state_rx.borrow().volume
    }

    /// Set volume
    #[zbus(property)]
    fn set_volume(&self, volume: f64) -> zbus::Result<()> {
        debug!("MPRIS: SetVolume called: {:.2}", volume);
        let _ = self.command_tx.send(MprisCommand::SetVolume(volume));
        Ok(())
    }

    /// Current position in microseconds
    #[zbus(property)]
    fn position(&self) -> i64 {
        self.state_rx.borrow().position_us
    }

    /// Minimum playback rate
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    /// Maximum playback rate
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    /// Whether the player can go to the next track
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.state_rx.borrow().can_go_next
    }

    /// Whether the player can go to the previous track
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.state_rx.borrow().can_go_previous
    }

    /// Whether the player can play
    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.state_rx.borrow().can_play
    }

    /// Whether the player can pause
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.state_rx.borrow().can_pause
    }

    /// Whether the player can seek
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.state_rx.borrow().can_seek
    }

    /// Whether the player can control playback
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

// ── TrackList interface ─────────────────────────────────────────────────

/// The MPRIS TrackList interface.
///
/// Exposes the current playback queue so external clients (playerctl,
/// KDE Connect, GNOME Shell, etc.) can see what's playing and jump to
/// any track via `GoTo`.
///
/// `AddTrack` / `RemoveTrack` are no-ops because `CanEditTracks` is
/// `false` — the queue is managed by the applet's own UI.
struct MprisTrackList {
    command_tx: mpsc::UnboundedSender<MprisCommand>,
    state_rx: watch::Receiver<MprisState>,
}

#[interface(name = "org.mpris.MediaPlayer2.TrackList")]
impl MprisTrackList {
    /// Get metadata for a set of tracks.
    fn get_tracks_metadata(
        &self,
        track_ids: Vec<ObjectPath<'_>>,
    ) -> Vec<HashMap<String, OwnedValue>> {
        let state = self.state_rx.borrow();
        track_ids
            .iter()
            .filter_map(|requested| {
                state
                    .tracklist
                    .iter()
                    .find(|e| e.object_path == requested.as_str())
                    .map(|e| e.metadata.to_dbus_metadata())
            })
            .collect()
    }

    /// Add a URI to the tracklist (not supported — CanEditTracks is false).
    fn add_track(
        &self,
        _uri: &str,
        _after_track: ObjectPath<'_>,
        _set_as_current: bool,
    ) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::NotSupported(
            "This player does not support editing the tracklist".into(),
        ))
    }

    /// Remove a track from the tracklist (not supported — CanEditTracks is false).
    fn remove_track(&self, _track_id: ObjectPath<'_>) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::NotSupported(
            "This player does not support editing the tracklist".into(),
        ))
    }

    /// Skip to the specified track.
    fn go_to(&self, track_id: ObjectPath<'_>) {
        let path = track_id.as_str().to_string();
        debug!("MPRIS: GoTo called for {}", path);
        let _ = self.command_tx.send(MprisCommand::GoTo(path));
    }

    /// Emitted when the entire tracklist is replaced.
    #[zbus(signal)]
    async fn track_list_replaced(
        emitter: &SignalEmitter<'_>,
        tracks: Vec<ObjectPath<'_>>,
        current_track: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    /// Emitted when a track is added (unused — we always replace).
    #[zbus(signal)]
    async fn track_added(
        emitter: &SignalEmitter<'_>,
        metadata: HashMap<String, OwnedValue>,
        after_track: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    /// Emitted when a track is removed (unused — we always replace).
    #[zbus(signal)]
    async fn track_removed(
        emitter: &SignalEmitter<'_>,
        track_id: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    /// Emitted when a track's metadata changes (unused currently).
    #[zbus(signal)]
    async fn track_metadata_changed(
        emitter: &SignalEmitter<'_>,
        track_id: ObjectPath<'_>,
        metadata: HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;

    /// The ordered list of track object paths in the tracklist.
    #[zbus(property)]
    fn tracks(&self) -> Vec<OwnedObjectPath> {
        self.state_rx
            .borrow()
            .tracklist
            .iter()
            .filter_map(|e| ObjectPath::try_from(e.object_path.as_str()).ok())
            .map(|p| p.into())
            .collect()
    }

    /// Whether the tracklist can be edited by external clients.
    #[zbus(property)]
    fn can_edit_tracks(&self) -> bool {
        false
    }
}

/// Start the MPRIS service and return a handle for communication
///
/// # Returns
/// A tuple of (MprisHandle, command receiver)
/// - The handle is used to update MPRIS state from the app
/// - The receiver is used to receive commands from D-Bus
pub async fn start_mpris_service()
-> Result<(MprisHandle, mpsc::UnboundedReceiver<MprisCommand>), String> {
    info!("Starting MPRIS D-Bus service");

    // Create communication channels
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (state_tx, state_rx) = watch::channel(MprisState::default());

    // Build the connection
    let bus_name = format!("org.mpris.MediaPlayer2.{}", MPRIS_NAME);

    let connection = Builder::session()
        .map_err(|e| format!("Failed to create session bus builder: {}", e))?
        .name(bus_name.as_str())
        .map_err(|e| format!("Failed to request bus name: {}", e))?
        .serve_at(
            MPRIS_OBJECT_PATH,
            MprisRoot {
                command_tx: command_tx.clone(),
            },
        )
        .map_err(|e| format!("Failed to serve MprisRoot: {}", e))?
        .serve_at(
            MPRIS_OBJECT_PATH,
            MprisPlayer {
                command_tx: command_tx.clone(),
                state_rx: state_rx.clone(),
            },
        )
        .map_err(|e| format!("Failed to serve MprisPlayer: {}", e))?
        .serve_at(
            MPRIS_OBJECT_PATH,
            MprisTrackList {
                command_tx: command_tx.clone(),
                state_rx,
            },
        )
        .map_err(|e| format!("Failed to serve MprisTrackList: {}", e))?
        .build()
        .await
        .map_err(|e| format!("Failed to build D-Bus connection: {}", e))?;

    info!("MPRIS service registered at {} on session bus", bus_name);

    let handle = MprisHandle {
        state_tx,
        connection: Arc::new(Mutex::new(Some(connection))),
    };

    Ok((handle, command_rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playback_status_as_str() {
        assert_eq!(MprisPlaybackStatus::Stopped.as_str(), "Stopped");
        assert_eq!(MprisPlaybackStatus::Playing.as_str(), "Playing");
        assert_eq!(MprisPlaybackStatus::Paused.as_str(), "Paused");
    }

    #[test]
    fn test_metadata_to_dbus() {
        let metadata = MprisMetadata {
            track_id: "12345".to_string(),
            title: "Test Track".to_string(),
            artists: vec!["Test Artist".to_string()],
            album: Some("Test Album".to_string()),
            length_us: 180_000_000, // 3 minutes
            art_url: Some("https://example.com/art.jpg".to_string()),
            ..Default::default()
        };

        let dbus_metadata = metadata.to_dbus_metadata();

        assert!(dbus_metadata.contains_key("mpris:trackid"));
        assert!(dbus_metadata.contains_key("mpris:length"));
        assert!(dbus_metadata.contains_key("xesam:title"));
        assert!(dbus_metadata.contains_key("xesam:artist"));
        assert!(dbus_metadata.contains_key("xesam:album"));
        assert!(dbus_metadata.contains_key("mpris:artUrl"));
    }

    #[test]
    fn test_empty_metadata() {
        let metadata = MprisMetadata::default();
        let dbus_metadata = metadata.to_dbus_metadata();

        // Should still have trackid pointing to NoTrack
        assert!(dbus_metadata.contains_key("mpris:trackid"));
    }
}
