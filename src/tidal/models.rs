// SPDX-License-Identifier: MIT

//! Data models for TIDAL API responses.
//!
//! These models provide a simplified view of TIDAL's data structures
//! suitable for display in the COSMIC applet UI.

use serde::{Deserialize, Serialize};

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
        Self {
            id: t.id.to_string(),
            title: t.title,
            duration: t.duration as u32,
            track_number: t.track_number,
            artist_name: t.artist.name,
            artist_id: Some(t.artist.id.to_string()),
            album_name: Some(t.album.title.clone()),
            album_id: Some(t.album.id.to_string()),
            cover_url: Some(format!(
                "https://resources.tidal.com/images/{}/320x320.jpg",
                t.album.cover.replace('-', "/")
            )),
            explicit: t.explicit,
            audio_quality: Some(t.audio_quality),
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
            album_name: Some(t.album.title),
            album_id: Some(t.album.id.to_string()),
            cover_url: Some(format!(
                "https://resources.tidal.com/images/{}/320x320.jpg",
                t.album.cover.replace('-', "/")
            )),
            explicit: t.explicit,
            audio_quality: t.audio_quality,
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

/// Convert from tidlers AlbumInfoResponse type (full album info)
impl From<tidlers::client::models::album::AlbumInfoResponse> for Album {
    fn from(a: tidlers::client::models::album::AlbumInfoResponse) -> Self {
        Self {
            id: a.id.to_string(),
            title: a.title,
            artist_name: a.artist.name,
            artist_id: Some(a.artist.id.to_string()),
            num_tracks: a.number_of_tracks,
            duration: a.duration as u32,
            release_date: Some(a.release_date),
            cover_url: Some(format!(
                "https://resources.tidal.com/images/{}/320x320.jpg",
                a.cover.replace('-', "/")
            )),
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
            cover_url: a.cover.map(|c| {
                format!(
                    "https://resources.tidal.com/images/{}/320x320.jpg",
                    c.replace('-', "/")
                )
            }),
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
            picture_url: a.picture.map(|p| {
                format!(
                    "https://resources.tidal.com/images/{}/320x320.jpg",
                    p.replace('-', "/")
                )
            }),
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
            picture_url: a.picture.map(|p| {
                format!(
                    "https://resources.tidal.com/images/{}/750x750.jpg",
                    p.replace('-', "/")
                )
            }),
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
            picture_url: a.picture.map(|p| {
                format!(
                    "https://resources.tidal.com/images/{}/320x320.jpg",
                    p.replace('-', "/")
                )
            }),
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
            cover_url: Some(format!(
                "https://resources.tidal.com/images/{}/320x320.jpg",
                a.cover.replace('-', "/")
            )),
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

/// Convert from tidlers PlaylistInfo type (user playlists)
impl From<tidlers::client::models::playlist::PlaylistInfo> for Playlist {
    fn from(p: tidlers::client::models::playlist::PlaylistInfo) -> Self {
        Self {
            uuid: p.uuid,
            title: p.title,
            description: Some(p.description),
            creator_name: None,
            num_tracks: p.number_of_tracks as u32,
            duration: p.duration as u32,
            last_updated: Some(p.last_updated),
            image_url: Some(format!(
                "https://resources.tidal.com/images/{}/320x320.jpg",
                p.image.replace('-', "/")
            )),
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
            image_url: image_id.map(|img| {
                format!(
                    "https://resources.tidal.com/images/{}/320x320.jpg",
                    img.replace('-', "/")
                )
            }),
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
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    /// Matching tracks
    pub tracks: Vec<Track>,
    /// Matching albums
    pub albums: Vec<Album>,
    /// Matching artists
    pub artists: Vec<Artist>,
    /// Matching playlists
    pub playlists: Vec<Playlist>,
}

impl SearchResults {
    /// Check if the search returned any results
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }

    /// Total number of results across all categories
    pub fn total_count(&self) -> usize {
        self.tracks.len() + self.albums.len() + self.artists.len() + self.playlists.len()
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
