// SPDX-License-Identifier: MIT

//! Integration tests for the TIDAL data models module.
//!
//! Tests Track, Album, Artist, Playlist, Mix, and SearchResults types
//! including display formatting, defaults, edge cases, and serde roundtrips.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::tidal::models::{Album, Artist, Mix, Playlist, SearchResults, Track};

// ===========================================================================
// Track
// ===========================================================================

mod track {
    use super::*;

    #[test]
    fn default_has_empty_fields() {
        let t = Track::default();
        assert!(t.id.is_empty());
        assert!(t.title.is_empty());
        assert_eq!(t.duration, 0);
        assert_eq!(t.track_number, 0);
        assert!(t.artist_name.is_empty());
        assert!(t.artist_id.is_none());
        assert!(t.album_name.is_none());
        assert!(t.album_id.is_none());
        assert!(t.cover_url.is_none());
        assert!(!t.explicit);
        assert!(t.audio_quality.is_none());
    }

    #[test]
    fn duration_display_zero() {
        let t = Track {
            duration: 0,
            ..Default::default()
        };
        assert_eq!(t.duration_display(), "0:00");
    }

    #[test]
    fn duration_display_seconds_only() {
        let t = Track {
            duration: 5,
            ..Default::default()
        };
        assert_eq!(t.duration_display(), "0:05");
    }

    #[test]
    fn duration_display_one_minute() {
        let t = Track {
            duration: 60,
            ..Default::default()
        };
        assert_eq!(t.duration_display(), "1:00");
    }

    #[test]
    fn duration_display_typical() {
        let t = Track {
            duration: 185,
            ..Default::default()
        };
        assert_eq!(t.duration_display(), "3:05");
    }

    #[test]
    fn duration_display_long_track() {
        let t = Track {
            duration: 600,
            ..Default::default()
        };
        assert_eq!(t.duration_display(), "10:00");
    }

    #[test]
    fn duration_display_very_long() {
        // 1 hour 23 minutes 45 seconds = 5025 seconds
        // Note: Track::duration_display only does M:SS, not H:MM:SS
        let t = Track {
            duration: 5025,
            ..Default::default()
        };
        assert_eq!(t.duration_display(), "83:45");
    }

    #[test]
    fn duration_display_single_digit_seconds_padded() {
        let t = Track {
            duration: 61,
            ..Default::default()
        };
        assert_eq!(t.duration_display(), "1:01");
    }

    #[test]
    fn duration_display_59_seconds() {
        let t = Track {
            duration: 59,
            ..Default::default()
        };
        assert_eq!(t.duration_display(), "0:59");
    }

    #[test]
    fn clone_is_independent() {
        let t1 = Track {
            id: "123".to_string(),
            title: "Song".to_string(),
            duration: 200,
            ..Default::default()
        };
        let t2 = t1.clone();
        assert_eq!(t1.id, t2.id);
        assert_eq!(t1.title, t2.title);
        assert_eq!(t1.duration, t2.duration);
    }

    #[test]
    fn debug_format() {
        let t = Track {
            id: "42".to_string(),
            title: "Test".to_string(),
            ..Default::default()
        };
        let debug = format!("{:?}", t);
        assert!(debug.contains("42"));
        assert!(debug.contains("Test"));
    }

    #[test]
    fn serde_roundtrip() {
        let t = Track {
            id: "99".to_string(),
            title: "Roundtrip".to_string(),
            duration: 300,
            track_number: 5,
            artist_name: "Artist".to_string(),
            artist_id: Some("10".to_string()),
            album_name: Some("Album".to_string()),
            album_id: Some("20".to_string()),
            cover_url: Some("https://example.com/cover.jpg".to_string()),
            explicit: true,
            audio_quality: Some("HI_RES".to_string()),
        };
        let json = serde_json::to_string(&t).unwrap();
        let t2: Track = serde_json::from_str(&json).unwrap();
        assert_eq!(t2.id, "99");
        assert_eq!(t2.title, "Roundtrip");
        assert_eq!(t2.duration, 300);
        assert_eq!(t2.track_number, 5);
        assert_eq!(t2.artist_name, "Artist");
        assert_eq!(t2.artist_id, Some("10".to_string()));
        assert_eq!(t2.album_name, Some("Album".to_string()));
        assert_eq!(t2.album_id, Some("20".to_string()));
        assert_eq!(
            t2.cover_url,
            Some("https://example.com/cover.jpg".to_string())
        );
        assert!(t2.explicit);
        assert_eq!(t2.audio_quality, Some("HI_RES".to_string()));
    }

    #[test]
    fn serde_deserialize_minimal_json() {
        let json = r#"{
            "id": "1",
            "title": "Minimal",
            "duration": 0,
            "track_number": 0,
            "artist_name": "",
            "artist_id": null,
            "album_name": null,
            "album_id": null,
            "cover_url": null,
            "explicit": false,
            "audio_quality": null
        }"#;
        let t: Track = serde_json::from_str(json).unwrap();
        assert_eq!(t.id, "1");
        assert_eq!(t.title, "Minimal");
        assert!(!t.explicit);
    }

    #[test]
    fn many_tracks_in_vec() {
        let tracks: Vec<Track> = (0..100)
            .map(|i| Track {
                id: i.to_string(),
                title: format!("Track {}", i),
                duration: i * 30,
                ..Default::default()
            })
            .collect();
        assert_eq!(tracks.len(), 100);
        assert_eq!(tracks[0].duration_display(), "0:00");
        assert_eq!(tracks[99].duration_display(), "49:30");
    }
}

// ===========================================================================
// Album
// ===========================================================================

mod album {
    use super::*;

    #[test]
    fn default_has_empty_fields() {
        let a = Album::default();
        assert!(a.id.is_empty());
        assert!(a.title.is_empty());
        assert!(a.artist_name.is_empty());
        assert!(a.artist_id.is_none());
        assert_eq!(a.num_tracks, 0);
        assert_eq!(a.duration, 0);
        assert!(a.release_date.is_none());
        assert!(a.cover_url.is_none());
        assert!(!a.explicit);
        assert!(a.audio_quality.is_none());
        assert!(a.review.is_none());
    }

    #[test]
    fn clone_preserves_all_fields() {
        let a = Album {
            id: "album-1".to_string(),
            title: "Great Album".to_string(),
            artist_name: "Great Artist".to_string(),
            artist_id: Some("artist-1".to_string()),
            num_tracks: 12,
            duration: 3600,
            release_date: Some("2024-01-15".to_string()),
            cover_url: Some("https://example.com/album.jpg".to_string()),
            explicit: true,
            audio_quality: Some("LOSSLESS".to_string()),
            review: Some("A masterpiece".to_string()),
        };
        let b = a.clone();
        assert_eq!(a.id, b.id);
        assert_eq!(a.title, b.title);
        assert_eq!(a.artist_name, b.artist_name);
        assert_eq!(a.num_tracks, b.num_tracks);
        assert_eq!(a.duration, b.duration);
        assert_eq!(a.release_date, b.release_date);
        assert_eq!(a.cover_url, b.cover_url);
        assert_eq!(a.explicit, b.explicit);
        assert_eq!(a.audio_quality, b.audio_quality);
        assert_eq!(a.review, b.review);
    }

    #[test]
    fn serde_roundtrip() {
        let a = Album {
            id: "55".to_string(),
            title: "Serde Album".to_string(),
            artist_name: "Serde Artist".to_string(),
            artist_id: Some("77".to_string()),
            num_tracks: 8,
            duration: 2400,
            release_date: Some("2023-06-01".to_string()),
            cover_url: Some("https://tidal.com/cover.jpg".to_string()),
            explicit: false,
            audio_quality: Some("HI_RES".to_string()),
            review: None,
        };
        let json = serde_json::to_string(&a).unwrap();
        let a2: Album = serde_json::from_str(&json).unwrap();
        assert_eq!(a2.id, "55");
        assert_eq!(a2.title, "Serde Album");
        assert_eq!(a2.num_tracks, 8);
        assert!(!a2.explicit);
    }

    #[test]
    fn debug_format_contains_fields() {
        let a = Album {
            id: "100".to_string(),
            title: "Debug Album".to_string(),
            ..Default::default()
        };
        let debug = format!("{:?}", a);
        assert!(debug.contains("100"));
        assert!(debug.contains("Debug Album"));
    }
}

// ===========================================================================
// Artist
// ===========================================================================

mod artist {
    use super::*;

    #[test]
    fn default_has_empty_fields() {
        let a = Artist::default();
        assert!(a.id.is_empty());
        assert!(a.name.is_empty());
        assert!(a.picture_url.is_none());
        assert!(a.bio.is_none());
        assert!(a.popularity.is_none());
        assert!(a.roles.is_empty());
        assert!(a.url.is_none());
    }

    #[test]
    fn clone_preserves_all_fields() {
        let a = Artist {
            id: "art-1".to_string(),
            name: "Famous Artist".to_string(),
            picture_url: Some("https://example.com/artist.jpg".to_string()),
            bio: Some("A really great musician.".to_string()),
            popularity: Some(95),
            roles: vec!["Artist".to_string(), "Producer".to_string()],
            url: Some("https://tidal.com/artist/123".to_string()),
        };
        let b = a.clone();
        assert_eq!(a.id, b.id);
        assert_eq!(a.name, b.name);
        assert_eq!(a.picture_url, b.picture_url);
        assert_eq!(a.bio, b.bio);
        assert_eq!(a.popularity, b.popularity);
        assert_eq!(a.roles, b.roles);
        assert_eq!(a.url, b.url);
    }

    #[test]
    fn serde_roundtrip() {
        let a = Artist {
            id: "42".to_string(),
            name: "Test Artist".to_string(),
            picture_url: Some("https://pic.example.com/a.jpg".to_string()),
            bio: Some("Bio text".to_string()),
            popularity: Some(75),
            roles: vec!["DJ".to_string(), "Songwriter".to_string()],
            url: Some("https://tidal.com/artist/42".to_string()),
        };
        let json = serde_json::to_string(&a).unwrap();
        let a2: Artist = serde_json::from_str(&json).unwrap();
        assert_eq!(a2.id, "42");
        assert_eq!(a2.name, "Test Artist");
        assert_eq!(a2.roles.len(), 2);
        assert_eq!(a2.roles[0], "DJ");
    }

    #[test]
    fn roles_can_be_empty() {
        let a = Artist {
            roles: vec![],
            ..Default::default()
        };
        let json = serde_json::to_string(&a).unwrap();
        let a2: Artist = serde_json::from_str(&json).unwrap();
        assert!(a2.roles.is_empty());
    }

    #[test]
    fn debug_format() {
        let a = Artist {
            id: "1".to_string(),
            name: "Debug Artist".to_string(),
            ..Default::default()
        };
        let debug = format!("{:?}", a);
        assert!(debug.contains("Debug Artist"));
    }
}

// ===========================================================================
// Playlist
// ===========================================================================

mod playlist {
    use super::*;

    #[test]
    fn default_has_empty_fields() {
        let p = Playlist::default();
        assert!(p.uuid.is_empty());
        assert!(p.title.is_empty());
        assert!(p.description.is_none());
        assert!(p.creator_name.is_none());
        assert_eq!(p.num_tracks, 0);
        assert_eq!(p.duration, 0);
        assert!(p.last_updated.is_none());
        assert!(p.image_url.is_none());
        assert!(!p.is_user_playlist);
    }

    #[test]
    fn duration_display_short() {
        let p = Playlist {
            duration: 125,
            ..Default::default()
        };
        assert_eq!(p.duration_display(), "2:05");
    }

    #[test]
    fn duration_display_with_hours() {
        let p = Playlist {
            duration: 3665,
            ..Default::default()
        };
        assert_eq!(p.duration_display(), "1:01:05");
    }

    #[test]
    fn duration_display_zero() {
        let p = Playlist {
            duration: 0,
            ..Default::default()
        };
        assert_eq!(p.duration_display(), "0:00");
    }

    #[test]
    fn duration_display_exactly_one_hour() {
        let p = Playlist {
            duration: 3600,
            ..Default::default()
        };
        assert_eq!(p.duration_display(), "1:00:00");
    }

    #[test]
    fn duration_display_many_hours() {
        // 10 hours, 30 minutes, 15 seconds = 37815 seconds
        let p = Playlist {
            duration: 37815,
            ..Default::default()
        };
        assert_eq!(p.duration_display(), "10:30:15");
    }

    #[test]
    fn duration_display_59_minutes_59_seconds() {
        // Just under one hour
        let p = Playlist {
            duration: 3599,
            ..Default::default()
        };
        assert_eq!(p.duration_display(), "59:59");
    }

    #[test]
    fn duration_display_one_second() {
        let p = Playlist {
            duration: 1,
            ..Default::default()
        };
        assert_eq!(p.duration_display(), "0:01");
    }

    #[test]
    fn serde_roundtrip() {
        let p = Playlist {
            uuid: "abc-def-123".to_string(),
            title: "My Playlist".to_string(),
            description: Some("A cool playlist".to_string()),
            creator_name: Some("user42".to_string()),
            num_tracks: 50,
            duration: 7200,
            last_updated: Some("2024-01-01T00:00:00Z".to_string()),
            image_url: Some("https://example.com/playlist.jpg".to_string()),
            is_user_playlist: true,
        };
        let json = serde_json::to_string(&p).unwrap();
        let p2: Playlist = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.uuid, "abc-def-123");
        assert_eq!(p2.title, "My Playlist");
        assert_eq!(p2.num_tracks, 50);
        assert!(p2.is_user_playlist);
        assert_eq!(p2.description, Some("A cool playlist".to_string()));
    }

    #[test]
    fn clone_preserves_all_fields() {
        let p = Playlist {
            uuid: "uuid-1".to_string(),
            title: "Clone Test".to_string(),
            description: Some("desc".to_string()),
            creator_name: Some("creator".to_string()),
            num_tracks: 10,
            duration: 600,
            last_updated: Some("2024-06-15".to_string()),
            image_url: Some("https://img.example.com/p.jpg".to_string()),
            is_user_playlist: false,
        };
        let q = p.clone();
        assert_eq!(p.uuid, q.uuid);
        assert_eq!(p.title, q.title);
        assert_eq!(p.description, q.description);
        assert_eq!(p.creator_name, q.creator_name);
        assert_eq!(p.num_tracks, q.num_tracks);
        assert_eq!(p.duration, q.duration);
        assert_eq!(p.last_updated, q.last_updated);
        assert_eq!(p.image_url, q.image_url);
        assert_eq!(p.is_user_playlist, q.is_user_playlist);
    }
}

// ===========================================================================
// Mix
// ===========================================================================

mod mix {
    use super::*;

    #[test]
    fn default_has_empty_fields() {
        let m = Mix::default();
        assert!(m.id.is_empty());
        assert!(m.title.is_empty());
        assert!(m.subtitle.is_empty());
        assert!(m.mix_type.is_empty());
        assert!(m.image_url.is_none());
    }

    #[test]
    fn clone_preserves_all_fields() {
        let m = Mix {
            id: "mix-abc".to_string(),
            title: "My Daily Discovery".to_string(),
            subtitle: "Fresh picks for you".to_string(),
            mix_type: "DAILY_MIX".to_string(),
            image_url: Some("https://example.com/mix.jpg".to_string()),
        };
        let m2 = m.clone();
        assert_eq!(m.id, m2.id);
        assert_eq!(m.title, m2.title);
        assert_eq!(m.subtitle, m2.subtitle);
        assert_eq!(m.mix_type, m2.mix_type);
        assert_eq!(m.image_url, m2.image_url);
    }

    #[test]
    fn serde_roundtrip() {
        let m = Mix {
            id: "mix-123".to_string(),
            title: "Artist Mix".to_string(),
            subtitle: "Based on your listening".to_string(),
            mix_type: "ARTIST_MIX".to_string(),
            image_url: Some("https://example.com/art.jpg".to_string()),
        };
        let json = serde_json::to_string(&m).unwrap();
        let m2: Mix = serde_json::from_str(&json).unwrap();
        assert_eq!(m2.id, "mix-123");
        assert_eq!(m2.mix_type, "ARTIST_MIX");
    }

    #[test]
    fn debug_format() {
        let m = Mix {
            id: "dbg".to_string(),
            title: "Debug Mix".to_string(),
            ..Default::default()
        };
        let debug = format!("{:?}", m);
        assert!(debug.contains("Debug Mix"));
        assert!(debug.contains("dbg"));
    }

    #[test]
    fn various_mix_types() {
        for mix_type in ["DAILY_MIX", "ARTIST_MIX", "TRACK_MIX", "DISCOVERY_MIX"] {
            let m = Mix {
                mix_type: mix_type.to_string(),
                ..Default::default()
            };
            assert_eq!(m.mix_type, mix_type);
        }
    }
}

// ===========================================================================
// SearchResults
// ===========================================================================

mod search_results {
    use super::*;

    #[test]
    fn default_is_empty() {
        let sr = SearchResults::default();
        assert!(sr.is_empty());
        assert_eq!(sr.total_count(), 0);
    }

    #[test]
    fn is_empty_when_all_categories_empty() {
        let sr = SearchResults {
            tracks: vec![],
            albums: vec![],
            artists: vec![],
            playlists: vec![],
        };
        assert!(sr.is_empty());
    }

    #[test]
    fn not_empty_with_tracks() {
        let sr = SearchResults {
            tracks: vec![Track::default()],
            albums: vec![],
            artists: vec![],
            playlists: vec![],
        };
        assert!(!sr.is_empty());
        assert_eq!(sr.total_count(), 1);
    }

    #[test]
    fn not_empty_with_albums() {
        let sr = SearchResults {
            tracks: vec![],
            albums: vec![Album::default()],
            artists: vec![],
            playlists: vec![],
        };
        assert!(!sr.is_empty());
        assert_eq!(sr.total_count(), 1);
    }

    #[test]
    fn not_empty_with_artists() {
        let sr = SearchResults {
            tracks: vec![],
            albums: vec![],
            artists: vec![Artist::default()],
            playlists: vec![],
        };
        assert!(!sr.is_empty());
        assert_eq!(sr.total_count(), 1);
    }

    #[test]
    fn not_empty_with_playlists() {
        let sr = SearchResults {
            tracks: vec![],
            albums: vec![],
            artists: vec![],
            playlists: vec![Playlist::default()],
        };
        assert!(!sr.is_empty());
        assert_eq!(sr.total_count(), 1);
    }

    #[test]
    fn total_count_sums_all_categories() {
        let sr = SearchResults {
            tracks: vec![Track::default(), Track::default(), Track::default()],
            albums: vec![Album::default(), Album::default()],
            artists: vec![Artist::default()],
            playlists: vec![Playlist::default(), Playlist::default()],
        };
        assert_eq!(sr.total_count(), 8);
        assert!(!sr.is_empty());
    }

    #[test]
    fn debug_format() {
        let sr = SearchResults::default();
        let debug = format!("{:?}", sr);
        assert!(debug.contains("SearchResults"));
    }

    #[test]
    fn clone_is_independent() {
        let sr = SearchResults {
            tracks: vec![Track {
                id: "1".to_string(),
                ..Default::default()
            }],
            albums: vec![],
            artists: vec![],
            playlists: vec![],
        };
        let sr2 = sr.clone();
        assert_eq!(sr2.total_count(), 1);
        assert_eq!(sr2.tracks[0].id, "1");
    }

    #[test]
    fn large_result_set() {
        let tracks: Vec<Track> = (0..200)
            .map(|i| Track {
                id: i.to_string(),
                ..Default::default()
            })
            .collect();
        let albums: Vec<Album> = (0..50)
            .map(|i| Album {
                id: i.to_string(),
                ..Default::default()
            })
            .collect();
        let sr = SearchResults {
            tracks,
            albums,
            artists: vec![],
            playlists: vec![],
        };
        assert_eq!(sr.total_count(), 250);
        assert!(!sr.is_empty());
    }
}

// ===========================================================================
// Cross-type interactions / realistic scenarios
// ===========================================================================

mod scenarios {
    use super::*;

    /// Simulate building a playlist view from model data.
    #[test]
    fn build_playlist_with_tracks() {
        let playlist = Playlist {
            uuid: "pl-uuid".to_string(),
            title: "Road Trip".to_string(),
            num_tracks: 3,
            duration: 600,
            is_user_playlist: true,
            ..Default::default()
        };

        let tracks = vec![
            Track {
                id: "t1".to_string(),
                title: "Highway Star".to_string(),
                duration: 200,
                track_number: 1,
                artist_name: "Deep Purple".to_string(),
                album_name: Some("Machine Head".to_string()),
                ..Default::default()
            },
            Track {
                id: "t2".to_string(),
                title: "Radar Love".to_string(),
                duration: 190,
                track_number: 2,
                artist_name: "Golden Earring".to_string(),
                album_name: Some("Moontan".to_string()),
                ..Default::default()
            },
            Track {
                id: "t3".to_string(),
                title: "Born to Run".to_string(),
                duration: 210,
                track_number: 3,
                artist_name: "Bruce Springsteen".to_string(),
                album_name: Some("Born to Run".to_string()),
                ..Default::default()
            },
        ];

        assert_eq!(playlist.num_tracks, tracks.len() as u32);
        assert_eq!(tracks[0].duration_display(), "3:20");
        assert_eq!(tracks[1].duration_display(), "3:10");
        assert_eq!(tracks[2].duration_display(), "3:30");
        assert_eq!(playlist.duration_display(), "10:00");
    }

    /// Simulate building search results from multiple sources.
    #[test]
    fn aggregate_search_results() {
        let mut results = SearchResults::default();
        assert!(results.is_empty());

        // Add tracks
        results.tracks.push(Track {
            id: "100".to_string(),
            title: "Found Track".to_string(),
            artist_name: "Some Artist".to_string(),
            ..Default::default()
        });

        // Add an album
        results.albums.push(Album {
            id: "200".to_string(),
            title: "Found Album".to_string(),
            artist_name: "Some Artist".to_string(),
            ..Default::default()
        });

        assert!(!results.is_empty());
        assert_eq!(results.total_count(), 2);
    }

    /// Verify that track IDs can be used as lookup keys.
    #[test]
    fn track_id_as_hashmap_key() {
        use std::collections::HashMap;

        let tracks = vec![
            Track {
                id: "a".to_string(),
                title: "Track A".to_string(),
                ..Default::default()
            },
            Track {
                id: "b".to_string(),
                title: "Track B".to_string(),
                ..Default::default()
            },
        ];

        let map: HashMap<String, &Track> = tracks.iter().map(|t| (t.id.clone(), t)).collect();

        assert_eq!(map.get("a").unwrap().title, "Track A");
        assert_eq!(map.get("b").unwrap().title, "Track B");
        assert!(map.get("c").is_none());
    }

    /// Verify that artist info can reference albums and tracks.
    #[test]
    fn artist_with_discography() {
        let artist = Artist {
            id: "art-1".to_string(),
            name: "Test Band".to_string(),
            popularity: Some(80),
            roles: vec!["Artist".to_string()],
            ..Default::default()
        };

        let albums = vec![
            Album {
                id: "alb-1".to_string(),
                title: "First Album".to_string(),
                artist_name: artist.name.clone(),
                artist_id: Some(artist.id.clone()),
                num_tracks: 10,
                release_date: Some("2020-01-01".to_string()),
                ..Default::default()
            },
            Album {
                id: "alb-2".to_string(),
                title: "Second Album".to_string(),
                artist_name: artist.name.clone(),
                artist_id: Some(artist.id.clone()),
                num_tracks: 12,
                release_date: Some("2022-06-15".to_string()),
                ..Default::default()
            },
        ];

        assert_eq!(albums.len(), 2);
        for album in &albums {
            assert_eq!(album.artist_id.as_deref(), Some("art-1"));
            assert_eq!(album.artist_name, "Test Band");
        }
    }

    /// Simulate a queue of tracks for playback.
    #[test]
    fn playback_queue_operations() {
        let mut queue: Vec<Track> = (0..10)
            .map(|i| Track {
                id: format!("q-{}", i),
                title: format!("Queue Track {}", i),
                duration: 180 + i * 10,
                ..Default::default()
            })
            .collect();

        assert_eq!(queue.len(), 10);
        assert_eq!(queue[0].title, "Queue Track 0");
        assert_eq!(queue[9].title, "Queue Track 9");

        // Simulate "next track"
        let current = queue.remove(0);
        assert_eq!(current.id, "q-0");
        assert_eq!(queue.len(), 9);
        assert_eq!(queue[0].id, "q-1");

        // Simulate "add to queue"
        queue.push(Track {
            id: "bonus".to_string(),
            title: "Bonus Track".to_string(),
            duration: 240,
            ..Default::default()
        });
        assert_eq!(queue.len(), 10);
        assert_eq!(queue.last().unwrap().id, "bonus");
    }

    /// Test filtering explicit tracks from a list.
    #[test]
    fn filter_explicit_tracks() {
        let tracks = vec![
            Track {
                id: "1".to_string(),
                explicit: false,
                ..Default::default()
            },
            Track {
                id: "2".to_string(),
                explicit: true,
                ..Default::default()
            },
            Track {
                id: "3".to_string(),
                explicit: false,
                ..Default::default()
            },
            Track {
                id: "4".to_string(),
                explicit: true,
                ..Default::default()
            },
        ];

        let clean: Vec<&Track> = tracks.iter().filter(|t| !t.explicit).collect();
        assert_eq!(clean.len(), 2);
        assert_eq!(clean[0].id, "1");
        assert_eq!(clean[1].id, "3");
    }

    /// Test JSON serialization of a full search result set.
    #[test]
    fn serde_roundtrip_full_search_results() {
        let sr = SearchResults {
            tracks: vec![
                Track {
                    id: "t1".to_string(),
                    title: "Search Track 1".to_string(),
                    duration: 180,
                    ..Default::default()
                },
                Track {
                    id: "t2".to_string(),
                    title: "Search Track 2".to_string(),
                    duration: 240,
                    ..Default::default()
                },
            ],
            albums: vec![Album {
                id: "a1".to_string(),
                title: "Search Album".to_string(),
                ..Default::default()
            }],
            artists: vec![Artist {
                id: "ar1".to_string(),
                name: "Search Artist".to_string(),
                ..Default::default()
            }],
            playlists: vec![Playlist {
                uuid: "pl1".to_string(),
                title: "Search Playlist".to_string(),
                ..Default::default()
            }],
        };

        // Serialize each component individually (SearchResults doesn't derive Serialize)
        let tracks_json = serde_json::to_string(&sr.tracks).unwrap();
        let albums_json = serde_json::to_string(&sr.albums).unwrap();
        let artists_json = serde_json::to_string(&sr.artists).unwrap();
        let playlists_json = serde_json::to_string(&sr.playlists).unwrap();

        let tracks: Vec<Track> = serde_json::from_str(&tracks_json).unwrap();
        let albums: Vec<Album> = serde_json::from_str(&albums_json).unwrap();
        let artists: Vec<Artist> = serde_json::from_str(&artists_json).unwrap();
        let playlists: Vec<Playlist> = serde_json::from_str(&playlists_json).unwrap();

        assert_eq!(tracks.len(), 2);
        assert_eq!(albums.len(), 1);
        assert_eq!(artists.len(), 1);
        assert_eq!(playlists.len(), 1);
        assert_eq!(tracks[0].id, "t1");
        assert_eq!(albums[0].title, "Search Album");
    }

    /// Verify that Mix image_url is properly optional.
    #[test]
    fn mix_without_image() {
        let m = Mix {
            id: "no-img".to_string(),
            title: "Image-less Mix".to_string(),
            subtitle: "No cover art".to_string(),
            mix_type: "DISCOVERY_MIX".to_string(),
            image_url: None,
        };
        assert!(m.image_url.is_none());
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("null") || !json.contains("image_url"));
    }

    /// Verify track cover_url construction pattern (like TIDAL uses).
    #[test]
    fn cover_url_pattern() {
        let cover_id = "abcd-efgh-ijkl";
        let url = format!(
            "https://resources.tidal.com/images/{}/320x320.jpg",
            cover_id.replace('-', "/")
        );
        assert_eq!(
            url,
            "https://resources.tidal.com/images/abcd/efgh/ijkl/320x320.jpg"
        );
    }
}
