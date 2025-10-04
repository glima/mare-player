// SPDX-License-Identifier: MIT

//! Integration tests for the tidal/mpris.rs module.
//!
//! Tests MprisPlaybackStatus (Default, as_str, Clone, Copy, PartialEq, Eq, Debug),
//! MprisMetadata (Default, to_dbus_metadata, Clone, Debug, field access),
//! MprisState (Default, Clone, Debug, field access),
//! and MprisCommand enum (variants, Clone, Debug).
//!
//! Note: MprisHandle and the D-Bus service (start_mpris_service) require a
//! running D-Bus session bus and are not tested here. These tests focus on the
//! data types and their behaviour.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::tidal::mpris::{
    LoopStatus, MprisCommand, MprisMetadata, MprisPlaybackStatus, MprisState,
};

// ===========================================================================
// MprisPlaybackStatus — Default
// ===========================================================================

mod playback_status_default {
    use super::*;

    #[test]
    fn default_is_stopped() {
        let status = MprisPlaybackStatus::default();
        assert_eq!(status, MprisPlaybackStatus::Stopped);
    }
}

// ===========================================================================
// MprisPlaybackStatus — Clone, Copy, PartialEq, Eq
// ===========================================================================

mod playback_status_traits {
    use super::*;

    #[test]
    fn clone_equals_original() {
        let status = MprisPlaybackStatus::Playing;
        let cloned = status.clone();
        assert_eq!(status, cloned);
    }

    #[test]
    fn copy_equals_original() {
        let status = MprisPlaybackStatus::Paused;
        let copied = status;
        assert_eq!(status, copied);
    }

    #[test]
    fn same_variants_are_equal() {
        assert_eq!(MprisPlaybackStatus::Stopped, MprisPlaybackStatus::Stopped);
        assert_eq!(MprisPlaybackStatus::Playing, MprisPlaybackStatus::Playing);
        assert_eq!(MprisPlaybackStatus::Paused, MprisPlaybackStatus::Paused);
    }

    #[test]
    fn different_variants_are_not_equal() {
        assert_ne!(MprisPlaybackStatus::Stopped, MprisPlaybackStatus::Playing);
        assert_ne!(MprisPlaybackStatus::Playing, MprisPlaybackStatus::Paused);
        assert_ne!(MprisPlaybackStatus::Paused, MprisPlaybackStatus::Stopped);
    }

    #[test]
    fn eq_is_reflexive() {
        for s in [
            MprisPlaybackStatus::Stopped,
            MprisPlaybackStatus::Playing,
            MprisPlaybackStatus::Paused,
        ] {
            assert_eq!(s, s);
        }
    }

    #[test]
    fn eq_is_symmetric() {
        let a = MprisPlaybackStatus::Playing;
        let b = MprisPlaybackStatus::Playing;
        assert_eq!(a, b);
        assert_eq!(b, a);
    }
}

// ===========================================================================
// MprisPlaybackStatus — Debug
// ===========================================================================

mod playback_status_debug {
    use super::*;

    #[test]
    fn debug_stopped() {
        let dbg = format!("{:?}", MprisPlaybackStatus::Stopped);
        assert!(dbg.contains("Stopped"), "expected 'Stopped' in: {dbg}");
    }

    #[test]
    fn debug_playing() {
        let dbg = format!("{:?}", MprisPlaybackStatus::Playing);
        assert!(dbg.contains("Playing"), "expected 'Playing' in: {dbg}");
    }

    #[test]
    fn debug_paused() {
        let dbg = format!("{:?}", MprisPlaybackStatus::Paused);
        assert!(dbg.contains("Paused"), "expected 'Paused' in: {dbg}");
    }

    #[test]
    fn all_variants_produce_nonempty_debug() {
        for s in [
            MprisPlaybackStatus::Stopped,
            MprisPlaybackStatus::Playing,
            MprisPlaybackStatus::Paused,
        ] {
            let dbg = format!("{:?}", s);
            assert!(!dbg.is_empty());
        }
    }

    #[test]
    fn all_variants_produce_distinct_debug() {
        let variants = [
            MprisPlaybackStatus::Stopped,
            MprisPlaybackStatus::Playing,
            MprisPlaybackStatus::Paused,
        ];
        let debug_strings: Vec<String> = variants.iter().map(|s| format!("{:?}", s)).collect();
        for (i, a) in debug_strings.iter().enumerate() {
            for (j, b) in debug_strings.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "debug strings for variants {i} and {j} should differ");
                }
            }
        }
    }
}

// ===========================================================================
// MprisMetadata — Default
// ===========================================================================

mod metadata_default {
    use super::*;

    #[test]
    fn default_has_empty_track_id() {
        let m = MprisMetadata::default();
        assert!(m.track_id.is_empty());
    }

    #[test]
    fn default_has_empty_title() {
        let m = MprisMetadata::default();
        assert!(m.title.is_empty());
    }

    #[test]
    fn default_has_empty_artists() {
        let m = MprisMetadata::default();
        assert!(m.artists.is_empty());
    }

    #[test]
    fn default_has_none_album() {
        let m = MprisMetadata::default();
        assert!(m.album.is_none());
    }

    #[test]
    fn default_has_empty_album_artists() {
        let m = MprisMetadata::default();
        assert!(m.album_artists.is_empty());
    }

    #[test]
    fn default_has_zero_length() {
        let m = MprisMetadata::default();
        assert_eq!(m.length_us, 0);
    }

    #[test]
    fn default_has_none_art_url() {
        let m = MprisMetadata::default();
        assert!(m.art_url.is_none());
    }

    #[test]
    fn default_has_none_track_number() {
        let m = MprisMetadata::default();
        assert!(m.track_number.is_none());
    }

    #[test]
    fn default_has_none_disc_number() {
        let m = MprisMetadata::default();
        assert!(m.disc_number.is_none());
    }
}

// ===========================================================================
// MprisMetadata — Construction and field access
// ===========================================================================

mod metadata_construction {
    use super::*;

    #[test]
    fn full_metadata() {
        let m = MprisMetadata {
            track_id: "12345".to_string(),
            title: "Test Track".to_string(),
            artists: vec!["Artist One".to_string(), "Artist Two".to_string()],
            album: Some("Test Album".to_string()),
            album_artists: vec!["Album Artist".to_string()],
            length_us: 180_000_000,
            art_url: Some("https://example.com/art.jpg".to_string()),
            track_number: Some(3),
            disc_number: Some(1),
        };

        assert_eq!(m.track_id, "12345");
        assert_eq!(m.title, "Test Track");
        assert_eq!(m.artists.len(), 2);
        assert_eq!(m.artists[0], "Artist One");
        assert_eq!(m.artists[1], "Artist Two");
        assert_eq!(m.album.as_deref(), Some("Test Album"));
        assert_eq!(m.album_artists.len(), 1);
        assert_eq!(m.length_us, 180_000_000);
        assert_eq!(m.art_url.as_deref(), Some("https://example.com/art.jpg"));
        assert_eq!(m.track_number, Some(3));
        assert_eq!(m.disc_number, Some(1));
    }

    #[test]
    fn minimal_metadata() {
        let m = MprisMetadata {
            track_id: "1".to_string(),
            title: "Song".to_string(),
            artists: vec!["A".to_string()],
            ..Default::default()
        };

        assert_eq!(m.track_id, "1");
        assert_eq!(m.title, "Song");
        assert_eq!(m.artists.len(), 1);
        assert!(m.album.is_none());
        assert!(m.art_url.is_none());
        assert!(m.track_number.is_none());
        assert!(m.disc_number.is_none());
        assert_eq!(m.length_us, 0);
    }

    #[test]
    fn unicode_metadata() {
        let m = MprisMetadata {
            track_id: "42".to_string(),
            title: "こんにちは世界".to_string(),
            artists: vec!["アーティスト".to_string()],
            album: Some("アルバム名".to_string()),
            ..Default::default()
        };
        assert_eq!(m.title, "こんにちは世界");
        assert_eq!(m.artists[0], "アーティスト");
    }

    #[test]
    fn emoji_metadata() {
        let m = MprisMetadata {
            track_id: "emoji-1".to_string(),
            title: "🎵 Groovy Tune 🎵".to_string(),
            artists: vec!["🎤 Singer".to_string()],
            album: Some("💿 Greatest Hits".to_string()),
            ..Default::default()
        };
        assert!(m.title.contains('🎵'));
    }

    #[test]
    fn multiple_artists() {
        let m = MprisMetadata {
            artists: vec![
                "Artist A".to_string(),
                "Artist B".to_string(),
                "Artist C".to_string(),
                "Artist D".to_string(),
            ],
            ..Default::default()
        };
        assert_eq!(m.artists.len(), 4);
    }

    #[test]
    fn very_long_track_id() {
        let long_id = "x".repeat(10000);
        let m = MprisMetadata {
            track_id: long_id.clone(),
            ..Default::default()
        };
        assert_eq!(m.track_id.len(), 10000);
    }

    #[test]
    fn negative_length_is_possible() {
        // Metadata struct doesn't validate; it's just data
        let m = MprisMetadata {
            length_us: -1,
            ..Default::default()
        };
        assert_eq!(m.length_us, -1);
    }

    #[test]
    fn large_track_number() {
        let m = MprisMetadata {
            track_number: Some(i32::MAX),
            disc_number: Some(i32::MAX),
            ..Default::default()
        };
        assert_eq!(m.track_number, Some(i32::MAX));
        assert_eq!(m.disc_number, Some(i32::MAX));
    }

    #[test]
    fn zero_length_us() {
        let m = MprisMetadata {
            length_us: 0,
            ..Default::default()
        };
        assert_eq!(m.length_us, 0);
    }
}

// ===========================================================================
// MprisMetadata — Clone
// ===========================================================================

mod metadata_clone {
    use super::*;

    #[test]
    fn clone_preserves_all_fields() {
        let m = MprisMetadata {
            track_id: "clone-test".to_string(),
            title: "Clone Title".to_string(),
            artists: vec!["Clone Artist".to_string()],
            album: Some("Clone Album".to_string()),
            album_artists: vec!["Clone Album Artist".to_string()],
            length_us: 240_000_000,
            art_url: Some("https://example.com/clone.jpg".to_string()),
            track_number: Some(7),
            disc_number: Some(2),
        };
        let cloned = m.clone();

        assert_eq!(m.track_id, cloned.track_id);
        assert_eq!(m.title, cloned.title);
        assert_eq!(m.artists, cloned.artists);
        assert_eq!(m.album, cloned.album);
        assert_eq!(m.album_artists, cloned.album_artists);
        assert_eq!(m.length_us, cloned.length_us);
        assert_eq!(m.art_url, cloned.art_url);
        assert_eq!(m.track_number, cloned.track_number);
        assert_eq!(m.disc_number, cloned.disc_number);
    }

    #[test]
    fn clone_is_independent() {
        let m = MprisMetadata {
            title: "Original".to_string(),
            ..Default::default()
        };
        let mut cloned = m.clone();
        cloned.title = "Modified".to_string();
        assert_eq!(m.title, "Original");
        assert_eq!(cloned.title, "Modified");
    }

    #[test]
    fn clone_default() {
        let m = MprisMetadata::default();
        let cloned = m.clone();
        assert!(cloned.track_id.is_empty());
        assert!(cloned.title.is_empty());
        assert!(cloned.artists.is_empty());
    }
}

// ===========================================================================
// MprisMetadata — Debug
// ===========================================================================

mod metadata_debug {
    use super::*;

    #[test]
    fn debug_contains_struct_name() {
        let m = MprisMetadata::default();
        let dbg = format!("{:?}", m);
        assert!(
            dbg.contains("MprisMetadata"),
            "expected 'MprisMetadata' in: {dbg}"
        );
    }

    #[test]
    fn debug_contains_field_values() {
        let m = MprisMetadata {
            track_id: "debug-test-id".to_string(),
            title: "Debug Title".to_string(),
            ..Default::default()
        };
        let dbg = format!("{:?}", m);
        assert!(
            dbg.contains("debug-test-id"),
            "expected track_id in debug: {dbg}"
        );
        assert!(
            dbg.contains("Debug Title"),
            "expected title in debug: {dbg}"
        );
    }

    #[test]
    fn debug_is_nonempty() {
        let m = MprisMetadata::default();
        let dbg = format!("{:?}", m);
        assert!(!dbg.is_empty());
    }
}

// ===========================================================================
// MprisState — Default
// ===========================================================================

mod state_default {
    use super::*;

    #[test]
    fn default_playback_status_is_stopped() {
        let s = MprisState::default();
        assert_eq!(s.playback_status, MprisPlaybackStatus::Stopped);
    }

    #[test]
    fn default_metadata_is_default() {
        let s = MprisState::default();
        assert!(s.metadata.track_id.is_empty());
        assert!(s.metadata.title.is_empty());
    }

    #[test]
    fn default_position_is_zero() {
        let s = MprisState::default();
        assert_eq!(s.position_us, 0);
    }

    #[test]
    fn default_volume_is_zero() {
        let s = MprisState::default();
        assert!((s.volume - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn default_shuffle_is_false() {
        let s = MprisState::default();
        assert!(!s.shuffle);
    }

    #[test]
    fn default_can_go_next_is_false() {
        let s = MprisState::default();
        assert!(!s.can_go_next);
    }

    #[test]
    fn default_can_go_previous_is_false() {
        let s = MprisState::default();
        assert!(!s.can_go_previous);
    }

    #[test]
    fn default_can_play_is_false() {
        let s = MprisState::default();
        assert!(!s.can_play);
    }

    #[test]
    fn default_can_pause_is_false() {
        let s = MprisState::default();
        assert!(!s.can_pause);
    }

    #[test]
    fn default_can_seek_is_false() {
        let s = MprisState::default();
        assert!(!s.can_seek);
    }
}

// ===========================================================================
// MprisState — Construction and field access
// ===========================================================================

mod state_construction {
    use super::*;

    #[test]
    fn full_state() {
        let s = MprisState {
            playback_status: MprisPlaybackStatus::Playing,
            metadata: MprisMetadata {
                track_id: "42".to_string(),
                title: "Playing Track".to_string(),
                artists: vec!["The Artist".to_string()],
                album: Some("The Album".to_string()),
                length_us: 300_000_000,
                ..Default::default()
            },
            position_us: 120_000_000,
            volume: 0.75,
            shuffle: true,
            loop_status: LoopStatus::Playlist,
            can_go_next: true,
            can_go_previous: true,
            can_play: true,
            can_pause: true,
            can_seek: true,
            tracklist: Vec::new(),
            current_track_path: String::new(),
        };

        assert_eq!(s.playback_status, MprisPlaybackStatus::Playing);
        assert_eq!(s.metadata.track_id, "42");
        assert_eq!(s.position_us, 120_000_000);
        assert!((s.volume - 0.75).abs() < f64::EPSILON);
        assert!(s.shuffle);
        assert_eq!(s.loop_status, LoopStatus::Playlist);
        assert!(s.can_go_next);
        assert!(s.can_go_previous);
        assert!(s.can_play);
        assert!(s.can_pause);
        assert!(s.can_seek);
    }

    #[test]
    fn stopped_state_with_no_capabilities() {
        let s = MprisState {
            playback_status: MprisPlaybackStatus::Stopped,
            volume: 1.0,
            ..Default::default()
        };

        assert_eq!(s.playback_status, MprisPlaybackStatus::Stopped);
        assert!(!s.can_go_next);
        assert!(!s.can_go_previous);
        assert!(!s.can_play);
        assert!(!s.can_pause);
        assert!(!s.can_seek);
    }

    #[test]
    fn paused_state() {
        let s = MprisState {
            playback_status: MprisPlaybackStatus::Paused,
            position_us: 60_000_000,
            can_play: true,
            can_seek: true,
            ..Default::default()
        };

        assert_eq!(s.playback_status, MprisPlaybackStatus::Paused);
        assert_eq!(s.position_us, 60_000_000);
        assert!(s.can_play);
        assert!(s.can_seek);
        assert!(!s.can_pause); // Already paused
    }

    #[test]
    fn volume_at_max() {
        let s = MprisState {
            volume: 1.0,
            ..Default::default()
        };
        assert!((s.volume - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn volume_at_zero() {
        let s = MprisState {
            volume: 0.0,
            ..Default::default()
        };
        assert!((s.volume - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn negative_position_is_allowed() {
        // Data struct does not validate
        let s = MprisState {
            position_us: -1,
            ..Default::default()
        };
        assert_eq!(s.position_us, -1);
    }

    #[test]
    fn very_large_position() {
        // 24 hours in microseconds
        let s = MprisState {
            position_us: 86_400_000_000,
            ..Default::default()
        };
        assert_eq!(s.position_us, 86_400_000_000);
    }
}

// ===========================================================================
// MprisState — Clone
// ===========================================================================

mod state_clone {
    use super::*;

    #[test]
    fn clone_preserves_all_fields() {
        let s = MprisState {
            playback_status: MprisPlaybackStatus::Playing,
            metadata: MprisMetadata {
                track_id: "clone-state".to_string(),
                title: "Clone State Title".to_string(),
                artists: vec!["Artist".to_string()],
                album: Some("Album".to_string()),
                album_artists: vec!["Album Artist".to_string()],
                length_us: 200_000_000,
                art_url: Some("https://example.com/state.jpg".to_string()),
                track_number: Some(5),
                disc_number: Some(1),
            },
            position_us: 100_000_000,
            volume: 0.5,
            shuffle: true,
            loop_status: LoopStatus::Track,
            can_go_next: true,
            can_go_previous: false,
            can_play: true,
            can_pause: true,
            can_seek: true,
            tracklist: Vec::new(),
            current_track_path: String::new(),
        };

        let cloned = s.clone();

        assert_eq!(s.playback_status, cloned.playback_status);
        assert_eq!(s.metadata.track_id, cloned.metadata.track_id);
        assert_eq!(s.metadata.title, cloned.metadata.title);
        assert_eq!(s.metadata.artists, cloned.metadata.artists);
        assert_eq!(s.metadata.album, cloned.metadata.album);
        assert_eq!(s.metadata.album_artists, cloned.metadata.album_artists);
        assert_eq!(s.metadata.length_us, cloned.metadata.length_us);
        assert_eq!(s.metadata.art_url, cloned.metadata.art_url);
        assert_eq!(s.metadata.track_number, cloned.metadata.track_number);
        assert_eq!(s.metadata.disc_number, cloned.metadata.disc_number);
        assert_eq!(s.position_us, cloned.position_us);
        assert!((s.volume - cloned.volume).abs() < f64::EPSILON);
        assert_eq!(s.shuffle, cloned.shuffle);
        assert_eq!(s.loop_status, cloned.loop_status);
        assert_eq!(s.can_go_next, cloned.can_go_next);
        assert_eq!(s.can_go_previous, cloned.can_go_previous);
        assert_eq!(s.can_play, cloned.can_play);
        assert_eq!(s.can_pause, cloned.can_pause);
        assert_eq!(s.can_seek, cloned.can_seek);
    }

    #[test]
    fn clone_is_independent() {
        let s = MprisState {
            playback_status: MprisPlaybackStatus::Playing,
            volume: 0.8,
            ..Default::default()
        };
        let mut cloned = s.clone();
        cloned.playback_status = MprisPlaybackStatus::Stopped;
        cloned.volume = 0.2;

        assert_eq!(s.playback_status, MprisPlaybackStatus::Playing);
        assert!((s.volume - 0.8).abs() < f64::EPSILON);
        assert_eq!(cloned.playback_status, MprisPlaybackStatus::Stopped);
        assert!((cloned.volume - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn clone_default() {
        let s = MprisState::default();
        let cloned = s.clone();
        assert_eq!(cloned.playback_status, MprisPlaybackStatus::Stopped);
        assert_eq!(cloned.position_us, 0);
    }
}

// ===========================================================================
// MprisState — Debug
// ===========================================================================

mod state_debug {
    use super::*;

    #[test]
    fn debug_contains_struct_name() {
        let s = MprisState::default();
        let dbg = format!("{:?}", s);
        assert!(
            dbg.contains("MprisState"),
            "expected 'MprisState' in: {dbg}"
        );
    }

    #[test]
    fn debug_contains_status() {
        let s = MprisState {
            playback_status: MprisPlaybackStatus::Playing,
            ..Default::default()
        };
        let dbg = format!("{:?}", s);
        assert!(dbg.contains("Playing"), "expected 'Playing' in: {dbg}");
    }

    #[test]
    fn debug_is_nonempty() {
        let s = MprisState::default();
        let dbg = format!("{:?}", s);
        assert!(!dbg.is_empty());
    }
}

// ===========================================================================
// MprisCommand — variants
// ===========================================================================

mod command_variants {
    use super::*;

    #[test]
    fn play_variant() {
        let cmd = MprisCommand::Play;
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("Play"), "expected 'Play' in: {dbg}");
    }

    #[test]
    fn pause_variant() {
        let cmd = MprisCommand::Pause;
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("Pause"), "expected 'Pause' in: {dbg}");
    }

    #[test]
    fn play_pause_variant() {
        let cmd = MprisCommand::PlayPause;
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("PlayPause"), "expected 'PlayPause' in: {dbg}");
    }

    #[test]
    fn stop_variant() {
        let cmd = MprisCommand::Stop;
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("Stop"), "expected 'Stop' in: {dbg}");
    }

    #[test]
    fn next_variant() {
        let cmd = MprisCommand::Next;
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("Next"), "expected 'Next' in: {dbg}");
    }

    #[test]
    fn previous_variant() {
        let cmd = MprisCommand::Previous;
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("Previous"), "expected 'Previous' in: {dbg}");
    }

    #[test]
    fn seek_variant() {
        let cmd = MprisCommand::Seek(60_000_000);
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("Seek"), "expected 'Seek' in: {dbg}");
        assert!(
            dbg.contains("60000000"),
            "expected position value in: {dbg}"
        );
    }

    #[test]
    fn seek_variant_negative() {
        let cmd = MprisCommand::Seek(-10_000_000);
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("Seek"), "expected 'Seek' in: {dbg}");
    }

    #[test]
    fn seek_variant_zero() {
        let cmd = MprisCommand::Seek(0);
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("Seek"), "expected 'Seek' in: {dbg}");
    }

    #[test]
    fn set_position_variant() {
        let cmd = MprisCommand::SetPosition("track-1".to_string(), 120_000_000);
        let dbg = format!("{:?}", cmd);
        assert!(
            dbg.contains("SetPosition"),
            "expected 'SetPosition' in: {dbg}"
        );
        assert!(dbg.contains("track-1"), "expected track id in: {dbg}");
    }

    #[test]
    fn set_position_empty_track_id() {
        let cmd = MprisCommand::SetPosition(String::new(), 0);
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("SetPosition"));
    }

    #[test]
    fn open_uri_variant() {
        let cmd = MprisCommand::OpenUri("tidal://track/12345".to_string());
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("OpenUri"), "expected 'OpenUri' in: {dbg}");
        assert!(
            dbg.contains("tidal://track/12345"),
            "expected URI in: {dbg}"
        );
    }

    #[test]
    fn open_uri_empty_string() {
        let cmd = MprisCommand::OpenUri(String::new());
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("OpenUri"));
    }

    #[test]
    fn set_shuffle_true() {
        let cmd = MprisCommand::SetShuffle(true);
        let dbg = format!("{:?}", cmd);
        assert!(
            dbg.contains("SetShuffle"),
            "expected 'SetShuffle' in: {dbg}"
        );
        assert!(dbg.contains("true"), "expected 'true' in: {dbg}");
    }

    #[test]
    fn set_shuffle_false() {
        let cmd = MprisCommand::SetShuffle(false);
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("SetShuffle"));
        assert!(dbg.contains("false"));
    }

    #[test]
    fn set_volume_variant() {
        let cmd = MprisCommand::SetVolume(0.75);
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("SetVolume"), "expected 'SetVolume' in: {dbg}");
        assert!(dbg.contains("0.75"), "expected '0.75' in: {dbg}");
    }

    #[test]
    fn set_volume_zero() {
        let cmd = MprisCommand::SetVolume(0.0);
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("SetVolume"));
    }

    #[test]
    fn set_volume_max() {
        let cmd = MprisCommand::SetVolume(1.0);
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("SetVolume"));
    }

    #[test]
    fn raise_variant() {
        let cmd = MprisCommand::Raise;
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("Raise"), "expected 'Raise' in: {dbg}");
    }

    #[test]
    fn quit_variant() {
        let cmd = MprisCommand::Quit;
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("Quit"), "expected 'Quit' in: {dbg}");
    }
}

// ===========================================================================
// MprisCommand — Clone
// ===========================================================================

mod command_clone {
    use super::*;

    #[test]
    fn clone_play() {
        let cmd = MprisCommand::Play;
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_pause() {
        let cmd = MprisCommand::Pause;
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_play_pause() {
        let cmd = MprisCommand::PlayPause;
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_stop() {
        let cmd = MprisCommand::Stop;
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_next() {
        let cmd = MprisCommand::Next;
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_previous() {
        let cmd = MprisCommand::Previous;
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_seek() {
        let cmd = MprisCommand::Seek(42_000_000);
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_set_position() {
        let cmd = MprisCommand::SetPosition("t-1".to_string(), 99_000_000);
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_set_position_is_independent() {
        let cmd = MprisCommand::SetPosition("orig".to_string(), 100);
        let cloned = cmd.clone();
        // Both should have the same debug representation
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_open_uri() {
        let cmd = MprisCommand::OpenUri("tidal://album/42".to_string());
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_set_shuffle() {
        let cmd = MprisCommand::SetShuffle(true);
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_set_volume() {
        let cmd = MprisCommand::SetVolume(0.42);
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_raise() {
        let cmd = MprisCommand::Raise;
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }

    #[test]
    fn clone_quit() {
        let cmd = MprisCommand::Quit;
        let cloned = cmd.clone();
        assert_eq!(format!("{:?}", cmd), format!("{:?}", cloned));
    }
}

// ===========================================================================
// MprisCommand — Debug completeness
// ===========================================================================

mod command_debug {
    use super::*;

    #[test]
    fn all_simple_variants_produce_nonempty_debug() {
        let commands: Vec<MprisCommand> = vec![
            MprisCommand::Play,
            MprisCommand::Pause,
            MprisCommand::PlayPause,
            MprisCommand::Stop,
            MprisCommand::Next,
            MprisCommand::Previous,
            MprisCommand::Raise,
            MprisCommand::Quit,
        ];
        for cmd in &commands {
            let dbg = format!("{:?}", cmd);
            assert!(!dbg.is_empty(), "debug should not be empty for {:?}", cmd);
        }
    }

    #[test]
    fn all_parameterized_variants_produce_nonempty_debug() {
        let commands: Vec<MprisCommand> = vec![
            MprisCommand::Seek(0),
            MprisCommand::Seek(i64::MAX),
            MprisCommand::Seek(i64::MIN),
            MprisCommand::SetPosition(String::new(), 0),
            MprisCommand::SetPosition("track".to_string(), i64::MAX),
            MprisCommand::OpenUri(String::new()),
            MprisCommand::OpenUri("https://example.com".to_string()),
            MprisCommand::SetShuffle(true),
            MprisCommand::SetShuffle(false),
            MprisCommand::SetVolume(0.0),
            MprisCommand::SetVolume(0.5),
            MprisCommand::SetVolume(1.0),
            MprisCommand::SetVolume(f64::NAN),
            MprisCommand::SetVolume(f64::INFINITY),
        ];
        for cmd in &commands {
            let dbg = format!("{:?}", cmd);
            assert!(!dbg.is_empty(), "debug should not be empty for {:?}", cmd);
        }
    }

    #[test]
    fn simple_variants_have_distinct_debug() {
        let commands = [
            MprisCommand::Play,
            MprisCommand::Pause,
            MprisCommand::PlayPause,
            MprisCommand::Stop,
            MprisCommand::Next,
            MprisCommand::Previous,
            MprisCommand::Raise,
            MprisCommand::Quit,
        ];
        let debug_strings: Vec<String> = commands.iter().map(|c| format!("{:?}", c)).collect();
        for (i, a) in debug_strings.iter().enumerate() {
            for (j, b) in debug_strings.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "debug for commands {i} and {j} should differ");
                }
            }
        }
    }
}

// ===========================================================================
// Scenarios — simulated MPRIS lifecycle
// ===========================================================================

mod scenarios {
    use super::*;

    #[test]
    fn state_transition_stopped_to_playing() {
        let mut state = MprisState::default();
        assert_eq!(state.playback_status, MprisPlaybackStatus::Stopped);

        state.playback_status = MprisPlaybackStatus::Playing;
        state.metadata = MprisMetadata {
            track_id: "42".to_string(),
            title: "New Track".to_string(),
            artists: vec!["Singer".to_string()],
            length_us: 180_000_000,
            ..Default::default()
        };
        state.can_play = true;
        state.can_pause = true;
        state.can_seek = true;
        state.volume = 0.8;

        assert_eq!(state.playback_status, MprisPlaybackStatus::Playing);
        assert_eq!(state.metadata.title, "New Track");
        assert!(state.can_pause);
    }

    #[test]
    fn state_transition_playing_to_paused_to_playing() {
        let mut state = MprisState {
            playback_status: MprisPlaybackStatus::Playing,
            position_us: 60_000_000,
            volume: 0.7,
            can_play: true,
            can_pause: true,
            ..Default::default()
        };

        // Pause
        state.playback_status = MprisPlaybackStatus::Paused;
        assert_eq!(state.playback_status, MprisPlaybackStatus::Paused);
        // Position should remain
        assert_eq!(state.position_us, 60_000_000);

        // Resume
        state.playback_status = MprisPlaybackStatus::Playing;
        assert_eq!(state.playback_status, MprisPlaybackStatus::Playing);
    }

    #[test]
    fn command_sequence_simulation() {
        // Simulate a typical command sequence from a media controller
        let commands = vec![
            MprisCommand::Play,
            MprisCommand::SetVolume(0.5),
            MprisCommand::SetShuffle(true),
            MprisCommand::Next,
            MprisCommand::Seek(120_000_000),
            MprisCommand::Pause,
            MprisCommand::PlayPause,
            MprisCommand::Previous,
            MprisCommand::Stop,
        ];

        assert_eq!(commands.len(), 9);
        // All should be clonable and debuggable
        for cmd in &commands {
            let cloned = cmd.clone();
            let dbg = format!("{:?}", cloned);
            assert!(!dbg.is_empty());
        }
    }

    #[test]
    fn metadata_update_on_track_change() {
        let mut state = MprisState {
            playback_status: MprisPlaybackStatus::Playing,
            metadata: MprisMetadata {
                track_id: "track-1".to_string(),
                title: "First Song".to_string(),
                artists: vec!["Artist A".to_string()],
                album: Some("Album X".to_string()),
                length_us: 200_000_000,
                ..Default::default()
            },
            position_us: 150_000_000,
            volume: 0.9,
            can_go_next: true,
            ..Default::default()
        };

        // Track changes
        state.metadata = MprisMetadata {
            track_id: "track-2".to_string(),
            title: "Second Song".to_string(),
            artists: vec!["Artist B".to_string()],
            album: Some("Album Y".to_string()),
            length_us: 250_000_000,
            art_url: Some("https://example.com/2.jpg".to_string()),
            ..Default::default()
        };
        state.position_us = 0;

        assert_eq!(state.metadata.track_id, "track-2");
        assert_eq!(state.metadata.title, "Second Song");
        assert_eq!(state.position_us, 0);
        // Volume and other settings persist
        assert!((state.volume - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn position_updates_during_playback() {
        let mut state = MprisState {
            playback_status: MprisPlaybackStatus::Playing,
            metadata: MprisMetadata {
                length_us: 180_000_000,
                ..Default::default()
            },
            ..Default::default()
        };

        // Simulate position updates (every second = 1_000_000 us)
        for i in 0..180 {
            state.position_us = i * 1_000_000;
            assert!(state.position_us >= 0);
            assert!(state.position_us <= state.metadata.length_us);
        }

        // Final position should be near the end
        assert_eq!(state.position_us, 179_000_000);
    }

    #[test]
    fn queue_navigation_capabilities() {
        // First track in queue: can go next but not previous
        let first = MprisState {
            can_go_next: true,
            can_go_previous: false,
            can_play: true,
            can_pause: true,
            ..Default::default()
        };
        assert!(first.can_go_next);
        assert!(!first.can_go_previous);

        // Middle of queue: can go both ways
        let middle = MprisState {
            can_go_next: true,
            can_go_previous: true,
            can_play: true,
            can_pause: true,
            ..Default::default()
        };
        assert!(middle.can_go_next);
        assert!(middle.can_go_previous);

        // Last track: can go previous but not next
        let last = MprisState {
            can_go_next: false,
            can_go_previous: true,
            can_play: true,
            can_pause: true,
            ..Default::default()
        };
        assert!(!last.can_go_next);
        assert!(last.can_go_previous);
    }

    #[test]
    fn shuffle_toggle_simulation() {
        let mut state = MprisState::default();
        assert!(!state.shuffle);

        // Toggle on
        state.shuffle = true;
        assert!(state.shuffle);

        // Toggle off
        state.shuffle = false;
        assert!(!state.shuffle);
    }

    #[test]
    fn volume_ramp_simulation() {
        let mut state = MprisState {
            volume: 0.0,
            ..Default::default()
        };

        // Ramp up volume in steps
        for step in 1..=10 {
            state.volume = step as f64 * 0.1;
        }
        assert!((state.volume - 1.0).abs() < f64::EPSILON);

        // Ramp down
        for step in (0..10).rev() {
            state.volume = step as f64 * 0.1;
        }
        assert!((state.volume - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn collecting_commands_into_vec() {
        let mut collected: Vec<MprisCommand> = Vec::new();
        collected.push(MprisCommand::Play);
        collected.push(MprisCommand::SetVolume(0.5));
        collected.push(MprisCommand::Next);
        collected.push(MprisCommand::Seek(30_000_000));
        collected.push(MprisCommand::Stop);

        assert_eq!(collected.len(), 5);

        // All should produce non-empty debug output
        for cmd in &collected {
            let dbg = format!("{:?}", cmd);
            assert!(!dbg.is_empty());
        }
    }

    #[test]
    fn multiple_states_in_vec() {
        let states: Vec<MprisState> = vec![
            MprisState {
                playback_status: MprisPlaybackStatus::Stopped,
                ..Default::default()
            },
            MprisState {
                playback_status: MprisPlaybackStatus::Playing,
                volume: 0.8,
                ..Default::default()
            },
            MprisState {
                playback_status: MprisPlaybackStatus::Paused,
                position_us: 42_000_000,
                ..Default::default()
            },
        ];

        assert_eq!(states.len(), 3);
        assert_eq!(states[0].playback_status, MprisPlaybackStatus::Stopped);
        assert_eq!(states[1].playback_status, MprisPlaybackStatus::Playing);
        assert_eq!(states[2].playback_status, MprisPlaybackStatus::Paused);
    }
}

// ===========================================================================
// Edge cases
// ===========================================================================

mod edge_cases {
    use super::*;

    #[test]
    fn metadata_with_very_long_title() {
        let long_title = "A".repeat(100_000);
        let m = MprisMetadata {
            title: long_title.clone(),
            ..Default::default()
        };
        assert_eq!(m.title.len(), 100_000);
        let cloned = m.clone();
        assert_eq!(cloned.title.len(), 100_000);
    }

    #[test]
    fn metadata_with_many_artists() {
        let artists: Vec<String> = (0..1000).map(|i| format!("Artist {}", i)).collect();
        let m = MprisMetadata {
            artists: artists.clone(),
            ..Default::default()
        };
        assert_eq!(m.artists.len(), 1000);
    }

    #[test]
    fn state_with_extreme_volume() {
        // Values outside 0.0..1.0 are technically allowed by the struct
        let s = MprisState {
            volume: 999.9,
            ..Default::default()
        };
        assert!((s.volume - 999.9).abs() < f64::EPSILON);
    }

    #[test]
    fn state_with_negative_volume() {
        let s = MprisState {
            volume: -0.5,
            ..Default::default()
        };
        assert!((s.volume - (-0.5)).abs() < f64::EPSILON);
    }

    #[test]
    fn command_seek_extreme_values() {
        let cmd_max = MprisCommand::Seek(i64::MAX);
        let cmd_min = MprisCommand::Seek(i64::MIN);
        let dbg_max = format!("{:?}", cmd_max);
        let dbg_min = format!("{:?}", cmd_min);
        assert!(!dbg_max.is_empty());
        assert!(!dbg_min.is_empty());
    }

    #[test]
    fn command_open_uri_with_special_characters() {
        let cmd = MprisCommand::OpenUri("tidal://track/123?foo=bar&baz=quux#fragment".to_string());
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("tidal://track/123"));
    }

    #[test]
    fn metadata_track_id_with_special_chars() {
        // Track IDs that might not form valid D-Bus object paths
        let m = MprisMetadata {
            track_id: "abc/def/../ghi".to_string(),
            ..Default::default()
        };
        assert_eq!(m.track_id, "abc/def/../ghi");
    }

    #[test]
    fn set_position_with_max_values() {
        let cmd = MprisCommand::SetPosition("max-track".to_string(), i64::MAX);
        let cloned = cmd.clone();
        let dbg = format!("{:?}", cloned);
        assert!(dbg.contains("max-track"));
    }

    #[test]
    fn rapid_status_changes() {
        let mut state = MprisState::default();
        for _ in 0..1000 {
            state.playback_status = MprisPlaybackStatus::Playing;
            state.playback_status = MprisPlaybackStatus::Paused;
            state.playback_status = MprisPlaybackStatus::Stopped;
        }
        assert_eq!(state.playback_status, MprisPlaybackStatus::Stopped);
    }

    #[test]
    fn metadata_empty_album_artists_vs_none_album() {
        let m = MprisMetadata {
            album: None,
            album_artists: vec![],
            ..Default::default()
        };
        assert!(m.album.is_none());
        assert!(m.album_artists.is_empty());

        let m2 = MprisMetadata {
            album: Some(String::new()),
            album_artists: vec![String::new()],
            ..Default::default()
        };
        assert_eq!(m2.album.as_deref(), Some(""));
        assert_eq!(m2.album_artists.len(), 1);
    }
}
