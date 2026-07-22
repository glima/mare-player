// SPDX-License-Identifier: MIT

//! Data models for TIDAL API responses.
//!
//! These models provide a simplified view of TIDAL's data structures
//! suitable for display in the COSMIC applet UI.

use serde::{Deserialize, Serialize};

// ── CDN URL helpers ────────────────────────────────────────────────────

/// Default cover size requested from TIDAL's resource CDN.
///
/// 320 px square is enough for list rows and search hits and is what almost
/// every call site wants.  Use [`tidal_cover_url_sized`] when you need a
/// different size (e.g. 750 for the artist detail hero).
pub const DEFAULT_COVER_SIZE_PX: u32 = 320;

/// Build a TIDAL CDN URL for an image UUID at the default size.
///
/// TIDAL's CDN expects the UUID's hyphen-separated segments to be joined by
/// `/` rather than `-`, followed by `<size>x<size>.jpg`.  Example:
///
/// ```text
/// uuid:  "7e58f111-5b1a-492a-aaf1-88fb55ce8a44"
/// url:   "https://resources.tidal.com/images/7e58f111/5b1a/492a/aaf1/88fb55ce8a44/320x320.jpg"
/// ```
pub fn tidal_cover_url(uuid: &str) -> String {
    tidal_cover_url_sized(uuid, DEFAULT_COVER_SIZE_PX)
}

/// Build a TIDAL CDN URL for an image UUID at a specific square size.
pub fn tidal_cover_url_sized(uuid: &str, size_px: u32) -> String {
    format!(
        "https://resources.tidal.com/images/{}/{size_px}x{size_px}.jpg",
        uuid.replace('-', "/")
    )
}

/// Build a TIDAL CDN URL for a featured-promo image UUID.
///
/// Promo artwork is a 550×400 banner — it has no square variant, so the
/// usual `320x320` request returns 403 Forbidden.  Featured cards must
/// request this specific non-square size.
pub fn tidal_promo_image_url(uuid: &str) -> String {
    format!(
        "https://resources.tidal.com/images/{}/550x400.jpg",
        uuid.replace('-', "/")
    )
}

/// A music track
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Track {
    /// Unique track ID
    pub id: String,
    /// Track title
    pub title: String,
    /// Track duration in seconds
    pub duration: u32,
    /// Track number on the album
    pub track_number: u32,
    /// Artist name
    pub artist_name: String,
    /// Artist ID
    pub artist_id: Option<String>,
    /// Album name
    pub album_name: Option<String>,
    /// Album ID
    pub album_id: Option<String>,
    /// Cover art URL (if available)
    pub cover_url: Option<String>,
    /// Whether the track is explicit
    pub explicit: bool,
    /// Audio quality available
    pub audio_quality: Option<String>,
    /// `true` if this entry is a music **video** (HLS), not an audio track.
    /// Such items play through the GStreamer video pipeline, not the audio
    /// engine. Defaults to `false` and is only set for playlist video items.
    #[serde(default)]
    pub is_video: bool,
}

impl std::fmt::Display for Track {
    /// Human-readable one-liner for logs: `Artist - Title [id]`, with a
    /// `(video)` suffix for music videos.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} - {} [{}]", self.artist_name, self.title, self.id)?;
        if self.is_video {
            write!(f, " (video)")?;
        }
        Ok(())
    }
}

impl Track {
    /// Format duration as MM:SS
    pub fn duration_display(&self) -> String {
        let minutes = self.duration / 60;
        let seconds = self.duration % 60;
        format!("{}:{:02}", minutes, seconds)
    }
}

/// Convert from tidlers Track type (full track response)
impl From<tidlers::client::models::track::Track> for Track {
    fn from(t: tidlers::client::models::track::Track) -> Self {
        let album = t.album;
        Self {
            id: t.id.to_string(),
            title: t.title,
            duration: t.duration as u32,
            track_number: t.track_number,
            artist_name: t.artist.name,
            artist_id: Some(t.artist.id.to_string()),
            album_name: album.as_ref().map(|a| a.title.clone()),
            album_id: album.as_ref().map(|a| a.id.to_string()),
            cover_url: album
                .as_ref()
                .and_then(|a| a.cover.as_deref())
                .map(tidal_cover_url),
            explicit: t.explicit,
            audio_quality: Some(t.audio_quality),
            is_video: false,
        }
    }
}

/// Convert from tidlers SearchTrackHit type (search results)
impl From<tidlers::client::models::search::SearchTrackHit> for Track {
    fn from(t: tidlers::client::models::search::SearchTrackHit) -> Self {
        let artist_name = t
            .artists
            .first()
            .and_then(|a| a.name.clone())
            .unwrap_or_else(|| "Unknown Artist".to_string());
        let artist_id = t
            .artists
            .first()
            .and_then(|a| a.id.map(|id| id.to_string()));

        Self {
            id: t.id.to_string(),
            title: t.title,
            duration: t.duration as u32,
            track_number: t.track_number.unwrap_or(0),
            artist_name,
            artist_id,
            album_name: t.album.as_ref().map(|a| a.title.clone()),
            album_id: t.album.as_ref().map(|a| a.id.to_string()),
            cover_url: t.album.as_ref().map(|a| tidal_cover_url(&a.cover)),
            explicit: t.explicit,
            audio_quality: t.audio_quality,
            is_video: false,
        }
    }
}

/// A music album
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Album {
    /// Unique album ID
    pub id: String,
    /// Album title
    pub title: String,
    /// Artist name
    pub artist_name: String,
    /// Artist ID
    pub artist_id: Option<String>,
    /// Number of tracks
    pub num_tracks: u32,
    /// Total duration in seconds
    pub duration: u32,
    /// Release date
    pub release_date: Option<String>,
    /// Cover art URL
    pub cover_url: Option<String>,
    /// Whether the album has explicit content
    pub explicit: bool,
    /// Audio quality available
    pub audio_quality: Option<String>,
    /// Album review / editorial description text
    pub review: Option<String>,
}

/// Convert from tidlers AlbumResponse type (full album info)
impl From<tidlers::client::models::album::AlbumResponse> for Album {
    fn from(a: tidlers::client::models::album::AlbumResponse) -> Self {
        Self {
            id: a.id.to_string(),
            title: a.title,
            artist_name: a.artist.name,
            artist_id: Some(a.artist.id.to_string()),
            num_tracks: a.number_of_tracks,
            duration: a.duration as u32,
            release_date: Some(a.release_date),
            cover_url: Some(tidal_cover_url(&a.cover)),
            explicit: a.explicit,
            audio_quality: Some(a.audio_quality),
            review: None,
        }
    }
}

/// Convert from tidlers SearchAlbumHit type (search results)
impl From<tidlers::client::models::search::SearchAlbumHit> for Album {
    fn from(a: tidlers::client::models::search::SearchAlbumHit) -> Self {
        let artist_name = a
            .artists
            .first()
            .and_then(|ar| ar.name.clone())
            .unwrap_or_else(|| "Unknown Artist".to_string());
        let artist_id = a
            .artists
            .first()
            .and_then(|ar| ar.id.map(|id| id.to_string()));

        Self {
            id: a.id.to_string(),
            title: a.title,
            artist_name,
            artist_id,
            num_tracks: a.number_of_tracks.unwrap_or(0),
            duration: a.duration.unwrap_or(0) as u32,
            release_date: a.release_date,
            cover_url: a.cover.as_deref().map(tidal_cover_url),
            explicit: a.explicit.unwrap_or(false),
            audio_quality: a.audio_quality,
            review: None,
        }
    }
}

/// A music artist
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Artist {
    /// Unique artist ID
    pub id: String,
    /// Artist name
    pub name: String,
    /// Artist picture URL
    pub picture_url: Option<String>,
    /// Artist bio/description
    pub bio: Option<String>,
    /// Popularity score (0-100)
    pub popularity: Option<u32>,
    /// Artist roles (e.g. "Artist", "Producer", "DJ")
    pub roles: Vec<String>,
    /// TIDAL URL for the artist page
    pub url: Option<String>,
}

/// Convert from tidlers Artist type (basic, embedded in other responses)
impl From<tidlers::client::models::artist::Artist> for Artist {
    fn from(a: tidlers::client::models::artist::Artist) -> Self {
        Self {
            id: a.id.to_string(),
            name: a.name,
            picture_url: a.picture.as_deref().map(tidal_cover_url),
            bio: None,
            popularity: None,
            roles: Vec::new(),
            url: None,
        }
    }
}

/// Convert from tidlers ArtistResponse type (full artist detail)
impl From<tidlers::client::models::artist::ArtistResponse> for Artist {
    fn from(a: tidlers::client::models::artist::ArtistResponse) -> Self {
        Self {
            id: a.id.to_string(),
            name: a.name,
            picture_url: a.picture.as_deref().map(|p| tidal_cover_url_sized(p, 750)),
            bio: None,
            popularity: Some(a.popularity),
            roles: a.artist_roles.into_iter().map(|r| r.category).collect(),
            url: Some(a.url),
        }
    }
}

/// Convert from tidlers SearchArtistHit type (search results)
impl From<tidlers::client::models::search::SearchArtistHit> for Artist {
    fn from(a: tidlers::client::models::search::SearchArtistHit) -> Self {
        Self {
            id: a.id.to_string(),
            name: a.name,
            picture_url: a.picture.as_deref().map(tidal_cover_url),
            bio: None,
            popularity: None,
            roles: Vec::new(),
            url: None,
        }
    }
}

/// Convert from tidlers ArtistAlbum type (artist discography)
impl From<tidlers::client::models::album::ArtistAlbum> for Album {
    fn from(a: tidlers::client::models::album::ArtistAlbum) -> Self {
        Self {
            id: a.id.to_string(),
            title: a.title,
            artist_name: a.artist.name,
            artist_id: Some(a.artist.id.to_string()),
            num_tracks: a.number_of_tracks,
            duration: a.duration as u32,
            release_date: Some(a.release_date),
            cover_url: Some(tidal_cover_url(&a.cover)),
            explicit: a.explicit,
            audio_quality: Some(a.audio_quality),
            review: None,
        }
    }
}

/// A playlist
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Playlist {
    /// Unique playlist UUID
    pub uuid: String,
    /// Playlist title
    pub title: String,
    /// Playlist description
    pub description: Option<String>,
    /// Creator username
    pub creator_name: Option<String>,
    /// Number of tracks
    pub num_tracks: u32,
    /// Total duration in seconds
    pub duration: u32,
    /// Last updated timestamp
    pub last_updated: Option<String>,
    /// Cover/image URL
    pub image_url: Option<String>,
    /// Whether this is a user-created playlist
    pub is_user_playlist: bool,
}

impl Playlist {
    /// Format duration as H:MM:SS or M:SS depending on length
    pub fn duration_display(&self) -> String {
        let hours = self.duration / 3600;
        let minutes = (self.duration % 3600) / 60;
        let seconds = self.duration % 60;
        if hours > 0 {
            format!("{}:{:02}:{:02}", hours, minutes, seconds)
        } else {
            format!("{}:{:02}", minutes, seconds)
        }
    }
}

/// Convert from tidlers PlaylistResponse type (user playlists)
impl From<tidlers::client::models::playlist::PlaylistResponse> for Playlist {
    fn from(p: tidlers::client::models::playlist::PlaylistResponse) -> Self {
        Self {
            uuid: p.uuid,
            title: p.title,
            description: Some(p.description),
            creator_name: None,
            num_tracks: p.number_of_tracks as u32,
            duration: p.duration as u32,
            last_updated: Some(p.last_updated),
            image_url: Some(tidal_cover_url(&p.image)),
            is_user_playlist: true,
        }
    }
}

/// Convert from tidlers SearchPlaylistHit type (search results)
impl From<tidlers::client::models::search::SearchPlaylistHit> for Playlist {
    fn from(p: tidlers::client::models::search::SearchPlaylistHit) -> Self {
        // Prefer square_image over image - the image field URLs often return 403 Forbidden
        let image_id = p.square_image.or(p.image);
        Self {
            uuid: p.uuid,
            title: p.title,
            description: p.description,
            creator_name: None,
            num_tracks: p.number_of_tracks.unwrap_or(0),
            duration: p.duration.unwrap_or(0) as u32,
            last_updated: p.last_updated,
            image_url: image_id.as_deref().map(tidal_cover_url),
            is_user_playlist: false,
        }
    }
}

/// A personalized mix (e.g. "My Daily Discovery", artist mixes, track mixes)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mix {
    /// Unique mix ID (used to fetch tracks)
    pub id: String,
    /// Mix title (e.g. "My Daily Discovery")
    pub title: String,
    /// Mix subtitle / short description
    pub subtitle: String,
    /// Mix type (e.g. "DAILY_MIX", "ARTIST_MIX", "TRACK_MIX")
    pub mix_type: String,
    /// Cover image URL (best available from mix_images)
    pub image_url: Option<String>,
}

/// Search results container
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResults {
    /// Matching tracks
    pub tracks: Vec<Track>,
    /// Matching albums
    pub albums: Vec<Album>,
    /// Matching artists
    pub artists: Vec<Artist>,
    /// Matching playlists
    pub playlists: Vec<Playlist>,
    /// Matching music videos (each is a playable [`Track`] with `is_video`).
    /// `#[serde(default)]` keeps older cached search payloads deserializable.
    #[serde(default)]
    pub videos: Vec<Track>,
}

// ── Playback source ──────────────────────────────────────────────────

/// The TIDAL container that started a playback session.
///
/// Maps directly onto TIDAL's `sourceType` enum in playback_session
/// events.  Threaded from the view that initiated playback (album detail,
/// playlist detail, mix detail, etc.) into `play_reporter` so plays
/// surface in Recently Played and credit the right container in the
/// TIDAL recommendations engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackSourceKind {
    Album,
    Playlist,
    Mix,
    Artist,
    /// Track-seeded radio (TIDAL's "Radio" feature: pick a track, get a
    /// generated station of similar tracks).  Emits `TRACK_RADIO` as the
    /// `sourceType`; consistent with TIDAL's `ARTIST_RADIO` pattern.
    /// **Probe**: this string is inferred, not confirmed from a captured
    /// official-client play.  If TIDAL ignores it, plays fall back to
    /// the same not-in-Recently-Played behaviour as `Track`.
    TrackRadio,
    /// Catch-all for ad-hoc / local-only contexts (favorites list,
    /// history view, single-track plays).  Reports as `TRACK` to TIDAL,
    /// which counts toward 'Most Listened' aggregates but doesn't
    /// surface in Recently Played.
    Track,
}

impl PlaybackSourceKind {
    /// String the TIDAL Event Producer expects for `sourceType`.
    pub fn as_tidal_str(&self) -> &'static str {
        match self {
            Self::Album => "ALBUM",
            Self::Playlist => "PLAYLIST",
            Self::Mix => "MIX",
            Self::Artist => "ARTIST",
            Self::TrackRadio => "TRACK_RADIO",
            Self::Track => "TRACK",
        }
    }
}

/// Resolved source for a playback session: the kind of container, its
/// TIDAL id, and a human-readable name for UI display.
#[derive(Debug, Clone)]
pub struct PlaybackSource {
    pub kind: PlaybackSourceKind,
    /// TIDAL id appropriate to `kind`.  For `Track` contexts this is
    /// typically the track id itself — the play still attributes
    /// somewhere, just not as a container play.
    pub id: String,
    /// Human-readable name shown in the now-playing bar (album title,
    /// playlist name, mix title, etc.).
    pub display_name: String,
}

impl PlaybackSource {
    /// Convenience constructor for an album-rooted session.
    pub fn album(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: PlaybackSourceKind::Album,
            id: id.into(),
            display_name: name.into(),
        }
    }
    pub fn playlist(uuid: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: PlaybackSourceKind::Playlist,
            id: uuid.into(),
            display_name: name.into(),
        }
    }
    pub fn mix(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: PlaybackSourceKind::Mix,
            id: id.into(),
            display_name: name.into(),
        }
    }
    pub fn artist(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: PlaybackSourceKind::Artist,
            id: id.into(),
            display_name: name.into(),
        }
    }
    /// Track-seeded radio station: pick a track, TIDAL generates a
    /// station of similar tracks.  Tries `sourceType=TRACK_RADIO` — see
    /// [`PlaybackSourceKind::TrackRadio`] for the probe caveat.
    pub fn track_radio(seed_track_id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            kind: PlaybackSourceKind::TrackRadio,
            id: seed_track_id.into(),
            display_name: display_name.into(),
        }
    }

    /// Catch-all fallback: a play without a real container context
    /// (e.g. from the favorites list, the history view, or a single
    /// MPRIS OpenUri).  TIDAL won't surface these in Recently Played.
    pub fn track(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            kind: PlaybackSourceKind::Track,
            id: id.into(),
            display_name: display_name.into(),
        }
    }

    /// Ad-hoc playback context with only a display label — used for
    /// the favorites list, the history view, etc., where there is no
    /// real TIDAL container.  The source id is left empty; `open_play_session`
    /// substitutes the per-listen track id when sending the play_log
    /// event, so attribution still credits the right track.
    pub fn ad_hoc(display_name: impl Into<String>) -> Self {
        Self {
            kind: PlaybackSourceKind::Track,
            id: String::new(),
            display_name: display_name.into(),
        }
    }
}

impl SearchResults {
    /// Check if the search returned any results
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
            && self.videos.is_empty()
    }

    /// Total number of results across all categories
    pub fn total_count(&self) -> usize {
        self.tracks.len()
            + self.albums.len()
            + self.artists.len()
            + self.playlists.len()
            + self.videos.len()
    }
}

/// A single activity from the TIDAL Feed (new releases from followed artists).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedActivity {
    /// The feed item (album release or history mix).
    pub item: FeedItem,
    /// ISO 8601 timestamp when the activity occurred.
    pub occurred_at: String,
    /// Whether the user has already seen this activity.
    pub seen: bool,
}

/// The content of a feed activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FeedItem {
    /// A new album or single released by a followed artist.
    AlbumRelease(Album),
    /// A monthly listening history mix.
    HistoryMix {
        id: String,
        title: String,
        subtitle: String,
        image_url: Option<String>,
    },
}

// ── Explore (TIDAL browse pages) ────────────────────────────────────
//
// Mirrors TIDAL's `GET /v1/pages/{path}` response: a page is a list of
// modules (sections), each holding either promo cards, links to other
// browse pages (genres/moods/decades/…), or content lists.

/// A fully parsed TIDAL browse page (e.g. "explore", a genre, a mood).
#[derive(Debug, Clone, Default)]
pub struct ExplorePage {
    pub title: String,
    pub sections: Vec<ExploreSection>,
}

/// One section of a browse page.
#[derive(Debug, Clone)]
pub enum ExploreSection {
    /// Top "Featured" carousel of promo cards.
    Featured {
        title: String,
        items: Vec<ExploreCard>,
    },
    /// A cloud/grid of links to other browse pages
    /// (Genres, Moods & Activities, Decades, More).
    Links { title: String, links: Vec<PageLink> },
    /// Horizontal list of albums.
    Albums { title: String, albums: Vec<Album> },
    /// Horizontal list of playlists.
    Playlists {
        title: String,
        playlists: Vec<Playlist>,
    },
    /// Horizontal list of artists.
    Artists { title: String, artists: Vec<Artist> },
}

/// A featured promo card: an image plus a navigation target.
#[derive(Debug, Clone)]
pub struct ExploreCard {
    pub title: String,
    pub subtitle: Option<String>,
    pub image_url: Option<String>,
    pub target: ExploreTarget,
}

/// Where an explore card or link points when activated.
#[derive(Debug, Clone)]
pub enum ExploreTarget {
    Album(String),
    Playlist(String),
    Artist(String),
    Mix(String),
    /// Another browse page to load recursively (`/v1/pages/{path}`).
    Page(String),
    /// Nothing actionable (unknown/unsupported target type).
    None,
}

/// A text link to another browse page (genre, mood, decade, …).
#[derive(Debug, Clone)]
pub struct PageLink {
    pub text: String,
    /// TIDAL page path to load, e.g. `"genre_hip_hop"` or a full
    /// `apiPath` the client normalises before requesting.
    pub path: String,
}

/// A single flattened row of the Explore page, suitable for rendering
/// through the virtual `List` widget (only visible rows materialise,
/// keeping scrolling smooth on long browse pages).
#[derive(Debug, Clone)]
pub enum ExploreRow {
    /// A section heading ("Featured", "Genres", …).
    SectionHeader(String),
    /// A featured promo card.
    Featured(ExploreCard),
    /// A page link (genre/mood/decade/more).
    Link(PageLink),
    /// An album entry.
    Album(Album),
    /// A playlist entry.
    Playlist(Playlist),
    /// An artist entry.
    Artist(Artist),
}

/// A single flattened row of the **artist-detail** view, rendered through the
/// virtual `List` widget so only the rows visible in the viewport materialise
/// (their covers then load lazily via `HandleCache::get_or_request`).
#[derive(Debug, Clone)]
pub enum ArtistRow {
    /// Hero block: picture, roles, popularity, and bio.
    Info(Box<Artist>),
    /// A section heading ("Top Tracks", "Videos", "Discography").
    SectionHeader(String),
    /// A top-track row, addressed by index into `selected_artist_top_tracks`.
    TopTrack(usize),
    /// A music-video row, addressed by index into `selected_artist_videos`.
    Video(usize),
    /// A discography album card.
    Album(Box<Album>),
}

/// A single flattened row of the **feed** view, rendered through the virtual
/// `List` widget so only visible rows materialise and covers load lazily.
#[derive(Debug, Clone)]
pub enum FeedRow {
    /// A time-period heading ("New", "Last week", …).
    SectionHeader(String),
    /// A feed activity (new album release or history mix).
    Activity(Box<FeedActivity>),
}

/// A single flattened row of the **track-detail** view, rendered through the
/// virtual `List` widget so only visible rows materialise and covers load
/// lazily.
#[derive(Debug, Clone)]
pub enum TrackDetailRow {
    /// The track info header: cover, title, clickable artist/album, metadata.
    Header(Box<Track>),
    /// A recommendation-section heading.
    SectionHeader(String),
    /// A "loading recommendations" placeholder shown under a header while a
    /// section's data is still in flight.
    Loading,
    /// A "More Albums by {Artist}" card (artist name omitted — it's redundant).
    ArtistAlbum(Box<Album>),
    /// A "Related Albums" card (includes artist name — different artists).
    RelatedAlbum(Box<Album>),
    /// A "Related Artists" card (picture + name).
    RelatedArtist(Box<Artist>),
}

impl ExplorePage {
    /// Flatten the page's sections into a single ordered list of rows
    /// (section header followed by its items), for the virtual `List`.
    pub fn into_rows(&self) -> Vec<ExploreRow> {
        let mut rows = Vec::new();
        for section in &self.sections {
            match section {
                ExploreSection::Featured { title, items } => {
                    if !title.is_empty() {
                        rows.push(ExploreRow::SectionHeader(title.clone()));
                    }
                    rows.extend(items.iter().cloned().map(ExploreRow::Featured));
                }
                ExploreSection::Links { title, links } => {
                    if !title.is_empty() {
                        rows.push(ExploreRow::SectionHeader(title.clone()));
                    }
                    rows.extend(links.iter().cloned().map(ExploreRow::Link));
                }
                ExploreSection::Albums { title, albums } => {
                    if !title.is_empty() {
                        rows.push(ExploreRow::SectionHeader(title.clone()));
                    }
                    rows.extend(albums.iter().cloned().map(ExploreRow::Album));
                }
                ExploreSection::Playlists { title, playlists } => {
                    if !title.is_empty() {
                        rows.push(ExploreRow::SectionHeader(title.clone()));
                    }
                    rows.extend(playlists.iter().cloned().map(ExploreRow::Playlist));
                }
                ExploreSection::Artists { title, artists } => {
                    if !title.is_empty() {
                        rows.push(ExploreRow::SectionHeader(title.clone()));
                    }
                    rows.extend(artists.iter().cloned().map(ExploreRow::Artist));
                }
            }
        }
        rows
    }
}

// ── Lyrics ────────────────────────────────────────────────────────────────────────

/// A single time-stamped lyric line parsed from an LRC subtitle string.
///
/// Time offsets are stored in milliseconds; the timestamp is the moment
/// the line should *start* being highlighted during playback.  The end
/// time is implicit (the start of the next line, or end of track).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LrcLine {
    /// Start time in milliseconds from the beginning of the track.
    pub time_ms: u64,
    /// Lyric text for this line (already trimmed of the timestamp prefix).
    pub text: String,
}

/// Lyrics for a single track, as returned by TIDAL.
///
/// TIDAL serves two parallel representations: a flat `plain_text` for
/// non-synced display and an `lrc_lines` vector parsed from the
/// timestamped `subtitles` field.  Either may be empty depending on the
/// provider's data — instrumental tracks tend to have neither; older
/// catalog entries often have plain text but no LRC sync.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackLyrics {
    /// Provider attribution string (e.g. "MusixMatch", "TIDAL").
    pub provider: Option<String>,
    /// Plain-text lyrics with original line breaks preserved.
    pub plain_text: Option<String>,
    /// Time-synced lines parsed from the LRC subtitles, sorted by
    /// `time_ms` ascending.  Empty when TIDAL has no synced data.
    pub lrc_lines: Vec<LrcLine>,
    /// True for languages that render right-to-left (Arabic, Hebrew,
    /// Farsi).  UI should mirror text alignment accordingly.
    pub is_right_to_left: bool,
}

impl TrackLyrics {
    /// True when TIDAL returned neither plain nor synced lyrics.
    pub fn is_empty(&self) -> bool {
        self.plain_text.as_deref().is_none_or(str::is_empty) && self.lrc_lines.is_empty()
    }

    /// True when time-synced (LRC) lyrics are available for karaoke-style
    /// playback highlighting.
    pub fn is_synced(&self) -> bool {
        !self.lrc_lines.is_empty()
    }

    /// Find the index of the line that should be highlighted at the given
    /// playback position (in seconds).
    ///
    /// Returns the last line whose `time_ms` is `<= position_ms`.  Returns
    /// `None` before the first line starts (i.e. during the intro before
    /// the first lyric), and the index of the last line for any position
    /// past the final timestamp.  O(log n) — cheap to call every tick.
    pub fn line_index_at(&self, position_seconds: f64) -> Option<usize> {
        if self.lrc_lines.is_empty() || position_seconds < 0.0 {
            return None;
        }
        let position_ms = (position_seconds * 1000.0) as u64;
        // partition_point returns the count of elements that are <= predicate;
        // since lines are sorted by time_ms ascending and we want the *last*
        // line that has started, that count minus one is our index.
        let count = self
            .lrc_lines
            .partition_point(|line| line.time_ms <= position_ms);
        if count == 0 { None } else { Some(count - 1) }
    }
}

/// Parse a TIDAL `subtitles` LRC-format string into time-synced lines.
///
/// Standard LRC syntax: `[mm:ss.xx]lyric text` per line, where the
/// fractional second separator may be `.` or `:` and may have 1–3
/// digits.  A single line may carry multiple timestamps for repeated
/// choruses (`[01:02.34][02:18.56]Same hook`); we expand those into
/// independent `LrcLine` entries.
///
/// Metadata tags (`[ti:Title]`, `[ar:Artist]`, `[al:Album]`, `[by:...]`,
/// `[length:...]`, `[offset:...]`) are skipped — the offset tag would
/// be useful but TIDAL doesn't appear to use it.  Empty lines and lines
/// with no timestamp are skipped.
///
/// Result is sorted by `time_ms` ascending, so consumers can rely on
/// monotonic ordering for binary-search lookups.
pub fn parse_lrc(subtitles: &str) -> Vec<LrcLine> {
    let mut out: Vec<LrcLine> = Vec::new();

    for raw_line in subtitles.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        // Collect every leading [..] tag, then whatever text follows.
        let mut rest = line;
        let mut timestamps: Vec<u64> = Vec::new();
        loop {
            let trimmed = rest.trim_start();
            if !trimmed.starts_with('[') {
                rest = trimmed;
                break;
            }
            let Some(close) = trimmed.find(']') else {
                rest = trimmed;
                break;
            };
            let tag = &trimmed[1..close];
            if let Some(ms) = parse_lrc_timestamp(tag) {
                timestamps.push(ms);
            }
            // Tags that aren't timestamps (metadata like `ti:Title`) are
            // silently discarded; we still consume them to keep the
            // remaining text clean.
            rest = &trimmed[close + 1..];
        }

        if timestamps.is_empty() {
            continue;
        }
        let text = rest.trim().to_string();
        for ms in timestamps {
            out.push(LrcLine {
                time_ms: ms,
                text: text.clone(),
            });
        }
    }

    out.sort_by_key(|line| line.time_ms);
    out
}

/// Convert a single LRC timestamp body (the part inside `[...]`) to
/// milliseconds.  Returns `None` for non-timestamp tags like
/// `ti:Title` so callers can treat them as metadata.
///
/// Accepted shapes: `mm:ss`, `mm:ss.xx`, `mm:ss.xxx`, `mm:ss:xx`.
/// Hour-prefixed timestamps (`hh:mm:ss.xx`) are rare in practice and
/// not currently supported — the format ambiguity (vs `mm:ss:xx`)
/// would need a smarter parser if TIDAL ever ships them.
fn parse_lrc_timestamp(tag: &str) -> Option<u64> {
    // Must start with at least one digit then ':'.
    let (mm_str, after_mm) = tag.split_once(':')?;
    let mm: u64 = mm_str.parse().ok()?;

    // Seconds may be followed by `.frac` or `:frac` or nothing.
    let (ss_str, frac_str) = match after_mm.split_once(['.', ':']) {
        Some((s, f)) => (s, Some(f)),
        None => (after_mm, None),
    };
    let ss: u64 = ss_str.parse().ok()?;
    let frac_ms: u64 = match frac_str {
        Some(f) if !f.is_empty() => {
            // Normalise to 3 digits: '5' -> 500ms, '50' -> 500ms,
            // '500' -> 500ms, '5000' -> truncated to '500' → 500ms.
            let digits: String = f.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                0
            } else {
                let padded = format!("{digits:0<3}");
                padded[..3].parse().ok()?
            }
        }
        _ => 0,
    };

    // Checked arithmetic: a pathological minute/second field (e.g. a long
    // all-digit run from garbage or fuzzed lyrics) parses as a valid u64 but
    // overflows when scaled to milliseconds. An overflowing value isn't a real
    // timestamp, so treat it as invalid (`None`) — the caller skips the line,
    // matching the other `?` parse failures above. (Found by `fuzz_lrc_parse`.)
    let ms = mm
        .checked_mul(60_000)?
        .checked_add(ss.checked_mul(1_000)?)?
        .checked_add(frac_ms)?;
    Some(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_results_videos_roundtrip_and_serde_default() {
        // Videos round-trip through the cache serialization.
        let mut r = SearchResults::default();
        r.videos.push(Track {
            id: "42".into(),
            title: "A Music Video".into(),
            is_video: true,
            ..Default::default()
        });
        let json = serde_json::to_string(&r).expect("serialize");
        let back: SearchResults = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.videos.len(), 1);
        assert!(back.videos[0].is_video);
        assert!(!back.is_empty());

        // Older cached payloads (pre-videos) must still deserialize — the
        // `#[serde(default)]` fills an empty Vec rather than failing the read.
        let old = r#"{"tracks":[],"albums":[],"artists":[],"playlists":[]}"#;
        let parsed: SearchResults = serde_json::from_str(old).expect("deserialize legacy");
        assert!(parsed.videos.is_empty());
        assert!(parsed.is_empty());
    }

    #[test]
    fn test_track_duration_display() {
        let track = Track {
            duration: 185,
            ..Default::default()
        };
        assert_eq!(track.duration_display(), "3:05");

        let track2 = Track {
            duration: 60,
            ..Default::default()
        };
        assert_eq!(track2.duration_display(), "1:00");
    }

    #[test]
    fn test_playlist_duration_display() {
        let playlist = Playlist {
            duration: 3665, // 1 hour, 1 minute, 5 seconds
            ..Default::default()
        };
        assert_eq!(playlist.duration_display(), "1:01:05");

        let playlist2 = Playlist {
            duration: 125, // 2 minutes, 5 seconds
            ..Default::default()
        };
        assert_eq!(playlist2.duration_display(), "2:05");
    }

    #[test]
    fn test_search_results_empty() {
        let results = SearchResults::default();
        assert!(results.is_empty());
        assert_eq!(results.total_count(), 0);
    }

    // ── Lyrics tests ───────────────────────────────────────────────────────────────────

    #[test]
    fn parse_lrc_timestamp_does_not_overflow_on_long_digit_runs() {
        // Regression (found by `fuzz_lrc_parse`): a long all-digit field parses
        // as a valid u64 but overflows when scaled to ms. Must return None
        // instead of panicking.
        assert_eq!(parse_lrc_timestamp("11111111111111111111:11"), None);
        assert_eq!(parse_lrc_timestamp("1:11111111111111111111"), None);

        // ...and parse_lrc must not panic on such a line (it's just skipped).
        assert!(parse_lrc("[11111111111111111111:11]lyric").is_empty());
    }

    #[test]
    fn parse_lrc_basic_two_digit_centiseconds() {
        let lines = parse_lrc("[00:01.23]First line\n[00:05.67]Second line");
        assert_eq!(
            lines,
            vec![
                LrcLine {
                    time_ms: 1_230,
                    text: "First line".into()
                },
                LrcLine {
                    time_ms: 5_670,
                    text: "Second line".into()
                },
            ]
        );
    }

    #[test]
    fn parse_lrc_accepts_three_digit_milliseconds() {
        let lines = parse_lrc("[01:30.456]Verse");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].time_ms, 90_456);
    }

    #[test]
    fn parse_lrc_accepts_one_digit_decisecond() {
        // '.5' should normalise to 500ms.
        let lines = parse_lrc("[00:02.5]Half second");
        assert_eq!(lines[0].time_ms, 2_500);
    }

    #[test]
    fn parse_lrc_accepts_colon_centisecond_separator() {
        // Some encoders use `:` instead of `.` for the fractional part.
        let lines = parse_lrc("[00:03:14]Pi-ish");
        assert_eq!(lines[0].time_ms, 3_140);
    }

    #[test]
    fn parse_lrc_accepts_seconds_with_no_fractional() {
        let lines = parse_lrc("[00:07]Whole second");
        assert_eq!(lines[0].time_ms, 7_000);
    }

    #[test]
    fn parse_lrc_expands_multi_timestamp_lines() {
        // Choruses commonly carry multiple timestamps for the same text.
        let lines = parse_lrc("[00:10.00][01:30.00][03:00.00]Chorus hook");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].time_ms, 10_000);
        assert_eq!(lines[1].time_ms, 90_000);
        assert_eq!(lines[2].time_ms, 180_000);
        assert!(lines.iter().all(|l| l.text == "Chorus hook"));
    }

    #[test]
    fn parse_lrc_skips_metadata_tags_keeps_timestamped_lines() {
        let raw = "[ti:Song Title]\n[ar:Artist]\n[al:Album]\n[00:01.00]Real line\n";
        let lines = parse_lrc(raw);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Real line");
    }

    #[test]
    fn parse_lrc_skips_empty_and_untimed_lines() {
        let raw = "\n\nuntimed garbage\n[00:02.00]Kept\n\n";
        let lines = parse_lrc(raw);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Kept");
    }

    #[test]
    fn parse_lrc_sorts_output_by_time() {
        // Out-of-order input (rare but possible when multi-timestamps
        // interleave) should still produce monotonic output.
        let raw = "[01:00.00]Later\n[00:30.00]Earlier\n[00:45.00]Middle";
        let lines = parse_lrc(raw);
        let times: Vec<u64> = lines.iter().map(|l| l.time_ms).collect();
        assert_eq!(times, vec![30_000, 45_000, 60_000]);
    }

    #[test]
    fn parse_lrc_handles_carriage_returns() {
        // TIDAL occasionally serves CRLF; trim_end_matches handles it.
        let lines = parse_lrc("[00:01.00]One\r\n[00:02.00]Two\r\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "One");
        assert_eq!(lines[1].text, "Two");
    }

    #[test]
    fn parse_lrc_empty_input_yields_empty_vec() {
        assert!(parse_lrc("").is_empty());
        assert!(parse_lrc("   \n\n  ").is_empty());
    }

    #[test]
    fn lyrics_line_index_at_returns_none_before_first_line() {
        let lyrics = TrackLyrics {
            lrc_lines: vec![
                LrcLine {
                    time_ms: 5_000,
                    text: "First".into(),
                },
                LrcLine {
                    time_ms: 10_000,
                    text: "Second".into(),
                },
            ],
            ..Default::default()
        };
        assert_eq!(lyrics.line_index_at(0.0), None);
        assert_eq!(lyrics.line_index_at(4.999), None);
    }

    #[test]
    fn lyrics_line_index_at_picks_active_line() {
        let lyrics = TrackLyrics {
            lrc_lines: vec![
                LrcLine {
                    time_ms: 5_000,
                    text: "A".into(),
                },
                LrcLine {
                    time_ms: 10_000,
                    text: "B".into(),
                },
                LrcLine {
                    time_ms: 15_000,
                    text: "C".into(),
                },
            ],
            ..Default::default()
        };
        assert_eq!(lyrics.line_index_at(5.0), Some(0));
        assert_eq!(lyrics.line_index_at(7.5), Some(0));
        assert_eq!(lyrics.line_index_at(10.0), Some(1));
        assert_eq!(lyrics.line_index_at(14.999), Some(1));
        assert_eq!(lyrics.line_index_at(15.0), Some(2));
        // Past the final timestamp: stays on the last line.
        assert_eq!(lyrics.line_index_at(99.0), Some(2));
    }

    #[test]
    fn lyrics_line_index_at_handles_empty_synced_lyrics() {
        let lyrics = TrackLyrics {
            plain_text: Some("Just a plain block".into()),
            ..Default::default()
        };
        assert_eq!(lyrics.line_index_at(10.0), None);
    }

    #[test]
    fn lyrics_is_empty_and_is_synced_flags() {
        let empty = TrackLyrics::default();
        assert!(empty.is_empty());
        assert!(!empty.is_synced());

        let plain_only = TrackLyrics {
            plain_text: Some("text".into()),
            ..Default::default()
        };
        assert!(!plain_only.is_empty());
        assert!(!plain_only.is_synced());

        let synced = TrackLyrics {
            lrc_lines: vec![LrcLine {
                time_ms: 0,
                text: "hi".into(),
            }],
            ..Default::default()
        };
        assert!(!synced.is_empty());
        assert!(synced.is_synced());
    }

    // ── Cover URL helpers ─────────────────────────────────────────────────────

    #[test]
    fn cover_url_joins_uuid_segments_with_slashes() {
        assert_eq!(
            tidal_cover_url("7e58f111-5b1a-492a-aaf1-88fb55ce8a44"),
            "https://resources.tidal.com/images/7e58f111/5b1a/492a/aaf1/88fb55ce8a44/320x320.jpg"
        );
    }

    #[test]
    fn cover_url_uses_default_size() {
        // tidal_cover_url delegates to the sized variant with DEFAULT_COVER_SIZE_PX.
        let uuid = "abcd-ef01";
        assert_eq!(
            tidal_cover_url(uuid),
            tidal_cover_url_sized(uuid, DEFAULT_COVER_SIZE_PX)
        );
    }

    #[test]
    fn cover_url_sized_embeds_requested_dimensions() {
        assert_eq!(
            tidal_cover_url_sized("a-b-c", 750),
            "https://resources.tidal.com/images/a/b/c/750x750.jpg"
        );
    }

    #[test]
    fn cover_url_without_hyphens_is_passed_through() {
        assert_eq!(
            tidal_cover_url_sized("singlesegment", 80),
            "https://resources.tidal.com/images/singlesegment/80x80.jpg"
        );
    }

    // ── PlaybackSourceKind ─────────────────────────────────────────────────

    #[test]
    fn playback_source_kind_tidal_strings() {
        assert_eq!(PlaybackSourceKind::Album.as_tidal_str(), "ALBUM");
        assert_eq!(PlaybackSourceKind::Playlist.as_tidal_str(), "PLAYLIST");
        assert_eq!(PlaybackSourceKind::Mix.as_tidal_str(), "MIX");
        assert_eq!(PlaybackSourceKind::Artist.as_tidal_str(), "ARTIST");
        assert_eq!(PlaybackSourceKind::TrackRadio.as_tidal_str(), "TRACK_RADIO");
        assert_eq!(PlaybackSourceKind::Track.as_tidal_str(), "TRACK");
    }

    // ── PlaybackSource constructors ──────────────────────────────────────

    #[test]
    fn playback_source_album_constructor() {
        let s = PlaybackSource::album("123", "Kind of Blue");
        assert_eq!(s.kind, PlaybackSourceKind::Album);
        assert_eq!(s.id, "123");
        assert_eq!(s.display_name, "Kind of Blue");
    }

    #[test]
    fn playback_source_playlist_constructor() {
        let s = PlaybackSource::playlist("uuid-1", "Roadtrip");
        assert_eq!(s.kind, PlaybackSourceKind::Playlist);
        assert_eq!(s.id, "uuid-1");
        assert_eq!(s.display_name, "Roadtrip");
    }

    #[test]
    fn playback_source_mix_constructor() {
        let s = PlaybackSource::mix("mix-9", "My Mix 1");
        assert_eq!(s.kind, PlaybackSourceKind::Mix);
        assert_eq!(s.id, "mix-9");
        assert_eq!(s.display_name, "My Mix 1");
    }

    #[test]
    fn playback_source_artist_constructor() {
        let s = PlaybackSource::artist("42", "Miles Davis");
        assert_eq!(s.kind, PlaybackSourceKind::Artist);
        assert_eq!(s.id, "42");
        assert_eq!(s.display_name, "Miles Davis");
    }

    #[test]
    fn playback_source_track_radio_constructor() {
        let s = PlaybackSource::track_radio("seed-7", "So What Radio");
        assert_eq!(s.kind, PlaybackSourceKind::TrackRadio);
        assert_eq!(s.id, "seed-7");
        assert_eq!(s.display_name, "So What Radio");
    }

    #[test]
    fn playback_source_track_constructor() {
        let s = PlaybackSource::track("t-1", "Blue in Green");
        assert_eq!(s.kind, PlaybackSourceKind::Track);
        assert_eq!(s.id, "t-1");
        assert_eq!(s.display_name, "Blue in Green");
    }

    #[test]
    fn playback_source_ad_hoc_has_empty_id_and_track_kind() {
        let s = PlaybackSource::ad_hoc("Favorites");
        assert_eq!(s.kind, PlaybackSourceKind::Track);
        assert!(s.id.is_empty());
        assert_eq!(s.display_name, "Favorites");
    }

    // ── SearchResults counting ────────────────────────────────────────

    #[test]
    fn search_results_count_sums_all_categories() {
        let results = SearchResults {
            tracks: vec![Track::default(), Track::default()],
            albums: vec![Album::default()],
            artists: vec![Artist::default(), Artist::default(), Artist::default()],
            playlists: vec![Playlist::default()],
            videos: vec![],
        };
        assert!(!results.is_empty());
        assert_eq!(results.total_count(), 7);
    }

    #[test]
    fn search_results_with_only_one_category_is_not_empty() {
        let results = SearchResults {
            tracks: vec![Track::default()],
            ..Default::default()
        };
        assert!(!results.is_empty());
        assert_eq!(results.total_count(), 1);
    }
}
