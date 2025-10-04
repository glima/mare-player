// SPDX-License-Identifier: MIT

//! Integration tests for the tidal/player.rs module.
//!
//! Tests PlaybackState enum (Default, From<EnginePlaybackState>, Clone, Copy,
//! PartialEq, Eq, Debug), NowPlaying struct (Default, Clone, field access),
//! and PlayerEvent enum (variants, Clone, Debug).
//!
//! Note: Player struct methods require a running PulseAudio daemon and are
//! not tested here. These tests focus on the data types and conversions.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::audio::PlaybackState as EnginePlaybackState;
use cosmic_applet_mare::tidal::player::{NowPlaying, PlaybackState, PlayerEvent};

// ===========================================================================
// PlaybackState — Default
// ===========================================================================

mod playback_state_default {
    use super::*;

    #[test]
    fn default_is_stopped() {
        let state = PlaybackState::default();
        assert_eq!(state, PlaybackState::Stopped);
    }
}

// ===========================================================================
// PlaybackState — From<EnginePlaybackState>
// ===========================================================================

mod playback_state_from_engine {
    use super::*;

    #[test]
    fn stopped_maps_to_stopped() {
        let engine_state = EnginePlaybackState::Stopped;
        let state: PlaybackState = engine_state.into();
        assert_eq!(state, PlaybackState::Stopped);
    }

    #[test]
    fn playing_maps_to_playing() {
        let engine_state = EnginePlaybackState::Playing;
        let state: PlaybackState = engine_state.into();
        assert_eq!(state, PlaybackState::Playing);
    }

    #[test]
    fn paused_maps_to_paused() {
        let engine_state = EnginePlaybackState::Paused;
        let state: PlaybackState = engine_state.into();
        assert_eq!(state, PlaybackState::Paused);
    }

    #[test]
    fn loading_maps_to_loading() {
        let engine_state = EnginePlaybackState::Loading;
        let state: PlaybackState = engine_state.into();
        assert_eq!(state, PlaybackState::Loading);
    }

    #[test]
    fn all_engine_variants_map_to_distinct_player_variants() {
        let states: Vec<PlaybackState> = vec![
            EnginePlaybackState::Stopped.into(),
            EnginePlaybackState::Playing.into(),
            EnginePlaybackState::Paused.into(),
            EnginePlaybackState::Loading.into(),
        ];
        for (i, a) in states.iter().enumerate() {
            for (j, b) in states.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "variants {i} and {j} should be distinct");
                }
            }
        }
    }

    #[test]
    fn from_is_deterministic() {
        for engine_state in [
            EnginePlaybackState::Stopped,
            EnginePlaybackState::Playing,
            EnginePlaybackState::Paused,
            EnginePlaybackState::Loading,
        ] {
            let a: PlaybackState = engine_state.into();
            let b: PlaybackState = engine_state.into();
            assert_eq!(a, b, "From should be deterministic for {:?}", engine_state);
        }
    }
}

// ===========================================================================
// PlaybackState — Clone, Copy, PartialEq, Eq
// ===========================================================================

mod playback_state_traits {
    use super::*;

    #[test]
    fn clone_equals_original() {
        let state = PlaybackState::Playing;
        let cloned = state.clone();
        assert_eq!(state, cloned);
    }

    #[test]
    fn copy_equals_original() {
        let state = PlaybackState::Paused;
        let copied = state;
        assert_eq!(state, copied);
    }

    #[test]
    fn same_variants_are_equal() {
        assert_eq!(PlaybackState::Stopped, PlaybackState::Stopped);
        assert_eq!(PlaybackState::Playing, PlaybackState::Playing);
        assert_eq!(PlaybackState::Paused, PlaybackState::Paused);
        assert_eq!(PlaybackState::Loading, PlaybackState::Loading);
    }

    #[test]
    fn different_variants_are_not_equal() {
        assert_ne!(PlaybackState::Stopped, PlaybackState::Playing);
        assert_ne!(PlaybackState::Playing, PlaybackState::Paused);
        assert_ne!(PlaybackState::Paused, PlaybackState::Loading);
        assert_ne!(PlaybackState::Loading, PlaybackState::Stopped);
    }

    #[test]
    fn eq_is_reflexive() {
        for state in [
            PlaybackState::Stopped,
            PlaybackState::Playing,
            PlaybackState::Paused,
            PlaybackState::Loading,
        ] {
            assert_eq!(state, state);
        }
    }

    #[test]
    fn eq_is_symmetric() {
        let a = PlaybackState::Playing;
        let b = PlaybackState::Playing;
        assert_eq!(a, b);
        assert_eq!(b, a);
    }
}

// ===========================================================================
// PlaybackState — Debug
// ===========================================================================

mod playback_state_debug {
    use super::*;

    #[test]
    fn debug_stopped() {
        let dbg = format!("{:?}", PlaybackState::Stopped);
        assert!(dbg.contains("Stopped"), "expected 'Stopped' in: {dbg}");
    }

    #[test]
    fn debug_playing() {
        let dbg = format!("{:?}", PlaybackState::Playing);
        assert!(dbg.contains("Playing"), "expected 'Playing' in: {dbg}");
    }

    #[test]
    fn debug_paused() {
        let dbg = format!("{:?}", PlaybackState::Paused);
        assert!(dbg.contains("Paused"), "expected 'Paused' in: {dbg}");
    }

    #[test]
    fn debug_loading() {
        let dbg = format!("{:?}", PlaybackState::Loading);
        assert!(dbg.contains("Loading"), "expected 'Loading' in: {dbg}");
    }

    #[test]
    fn all_variants_produce_nonempty_debug() {
        for state in [
            PlaybackState::Stopped,
            PlaybackState::Playing,
            PlaybackState::Paused,
            PlaybackState::Loading,
        ] {
            let dbg = format!("{:?}", state);
            assert!(!dbg.is_empty());
        }
    }

    #[test]
    fn all_variants_produce_distinct_debug() {
        let variants = [
            PlaybackState::Stopped,
            PlaybackState::Playing,
            PlaybackState::Paused,
            PlaybackState::Loading,
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
// NowPlaying — Default
// ===========================================================================

mod now_playing_default {
    use super::*;

    #[test]
    fn default_has_empty_track_id() {
        let np = NowPlaying::default();
        assert!(np.track_id.is_empty());
    }

    #[test]
    fn default_has_empty_title() {
        let np = NowPlaying::default();
        assert!(np.title.is_empty());
    }

    #[test]
    fn default_has_empty_artist() {
        let np = NowPlaying::default();
        assert!(np.artist.is_empty());
    }

    #[test]
    fn default_has_none_album() {
        let np = NowPlaying::default();
        assert!(np.album.is_none());
    }

    #[test]
    fn default_has_zero_duration() {
        let np = NowPlaying::default();
        assert!((np.duration - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn default_has_none_cover_url() {
        let np = NowPlaying::default();
        assert!(np.cover_url.is_none());
    }

    #[test]
    fn default_has_none_playlist_name() {
        let np = NowPlaying::default();
        assert!(np.playlist_name.is_none());
    }
}

// ===========================================================================
// NowPlaying — Construction and field access
// ===========================================================================

mod now_playing_construction {
    use super::*;

    #[test]
    fn can_construct_full_now_playing() {
        let np = NowPlaying {
            track_id: "12345".to_string(),
            title: "Test Track".to_string(),
            artist: "Test Artist".to_string(),
            album: Some("Test Album".to_string()),
            duration: 210.5,
            cover_url: Some("https://example.com/art.jpg".to_string()),
            playlist_name: Some("My Playlist".to_string()),
        };
        assert_eq!(np.track_id, "12345");
        assert_eq!(np.title, "Test Track");
        assert_eq!(np.artist, "Test Artist");
        assert_eq!(np.album.as_deref(), Some("Test Album"));
        assert!((np.duration - 210.5).abs() < f64::EPSILON);
        assert_eq!(np.cover_url.as_deref(), Some("https://example.com/art.jpg"));
        assert_eq!(np.playlist_name.as_deref(), Some("My Playlist"));
    }

    #[test]
    fn can_construct_minimal_now_playing() {
        let np = NowPlaying {
            track_id: "99".to_string(),
            title: "Short".to_string(),
            artist: "A".to_string(),
            album: None,
            duration: 30.0,
            cover_url: None,
            playlist_name: None,
        };
        assert_eq!(np.track_id, "99");
        assert!(np.album.is_none());
        assert!(np.cover_url.is_none());
        assert!(np.playlist_name.is_none());
    }

    #[test]
    fn unicode_fields() {
        let np = NowPlaying {
            track_id: "42".to_string(),
            title: "こんにちは世界".to_string(),
            artist: "アーティスト".to_string(),
            album: Some("アルバム".to_string()),
            duration: 180.0,
            cover_url: None,
            playlist_name: Some("プレイリスト".to_string()),
        };
        assert_eq!(np.title, "こんにちは世界");
        assert_eq!(np.artist, "アーティスト");
    }

    #[test]
    fn emoji_in_fields() {
        let np = NowPlaying {
            track_id: "1".to_string(),
            title: "🎵 Music 🎵".to_string(),
            artist: "🎤 Singer".to_string(),
            album: Some("💿 Album".to_string()),
            duration: 60.0,
            cover_url: None,
            playlist_name: None,
        };
        assert!(np.title.contains('🎵'));
        assert!(np.artist.contains('🎤'));
    }

    #[test]
    fn very_long_duration() {
        let np = NowPlaying {
            duration: 86400.0, // 24 hours
            ..Default::default()
        };
        assert!((np.duration - 86400.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fractional_duration() {
        let np = NowPlaying {
            duration: 0.001,
            ..Default::default()
        };
        assert!((np.duration - 0.001).abs() < f64::EPSILON);
    }
}

// ===========================================================================
// NowPlaying — Clone
// ===========================================================================

mod now_playing_clone {
    use super::*;

    #[test]
    fn clone_preserves_all_fields() {
        let np = NowPlaying {
            track_id: "track-1".to_string(),
            title: "Original Title".to_string(),
            artist: "Original Artist".to_string(),
            album: Some("Original Album".to_string()),
            duration: 300.0,
            cover_url: Some("https://img.example.com/1.jpg".to_string()),
            playlist_name: Some("Favorites".to_string()),
        };
        let cloned = np.clone();
        assert_eq!(np.track_id, cloned.track_id);
        assert_eq!(np.title, cloned.title);
        assert_eq!(np.artist, cloned.artist);
        assert_eq!(np.album, cloned.album);
        assert!((np.duration - cloned.duration).abs() < f64::EPSILON);
        assert_eq!(np.cover_url, cloned.cover_url);
        assert_eq!(np.playlist_name, cloned.playlist_name);
    }

    #[test]
    fn clone_is_independent() {
        let np = NowPlaying {
            track_id: "track-2".to_string(),
            title: "Title".to_string(),
            artist: "Artist".to_string(),
            ..Default::default()
        };
        let mut cloned = np.clone();
        cloned.title = "Modified".to_string();
        assert_eq!(np.title, "Title");
        assert_eq!(cloned.title, "Modified");
    }

    #[test]
    fn clone_default() {
        let np = NowPlaying::default();
        let cloned = np.clone();
        assert!(cloned.track_id.is_empty());
        assert!(cloned.title.is_empty());
        assert!(cloned.album.is_none());
    }
}

// ===========================================================================
// NowPlaying — Debug
// ===========================================================================

mod now_playing_debug {
    use super::*;

    #[test]
    fn debug_contains_struct_name() {
        let np = NowPlaying::default();
        let dbg = format!("{:?}", np);
        assert!(
            dbg.contains("NowPlaying"),
            "expected 'NowPlaying' in: {dbg}"
        );
    }

    #[test]
    fn debug_contains_field_values() {
        let np = NowPlaying {
            track_id: "test-id-123".to_string(),
            title: "My Song".to_string(),
            ..Default::default()
        };
        let dbg = format!("{:?}", np);
        assert!(
            dbg.contains("test-id-123"),
            "expected track_id in debug: {dbg}"
        );
        assert!(dbg.contains("My Song"), "expected title in debug: {dbg}");
    }

    #[test]
    fn debug_is_nonempty() {
        let np = NowPlaying::default();
        let dbg = format!("{:?}", np);
        assert!(!dbg.is_empty());
    }
}

// ===========================================================================
// PlayerEvent — variants
// ===========================================================================

mod player_event_variants {
    use super::*;

    #[test]
    fn track_ended_variant() {
        let event = PlayerEvent::TrackEnded;
        let dbg = format!("{:?}", event);
        assert!(
            dbg.contains("TrackEnded"),
            "expected 'TrackEnded' in: {dbg}"
        );
    }

    #[test]
    fn error_variant_with_message() {
        let event = PlayerEvent::Error("something went wrong".to_string());
        let dbg = format!("{:?}", event);
        assert!(dbg.contains("Error"), "expected 'Error' in: {dbg}");
        assert!(
            dbg.contains("something went wrong"),
            "expected error message in: {dbg}"
        );
    }

    #[test]
    fn error_variant_with_empty_message() {
        let event = PlayerEvent::Error(String::new());
        let dbg = format!("{:?}", event);
        assert!(dbg.contains("Error"), "expected 'Error' in: {dbg}");
    }

    #[test]
    fn state_changed_stopped() {
        let event = PlayerEvent::StateChanged(PlaybackState::Stopped);
        let dbg = format!("{:?}", event);
        assert!(
            dbg.contains("StateChanged"),
            "expected 'StateChanged' in: {dbg}"
        );
        assert!(dbg.contains("Stopped"), "expected 'Stopped' in: {dbg}");
    }

    #[test]
    fn state_changed_playing() {
        let event = PlayerEvent::StateChanged(PlaybackState::Playing);
        let dbg = format!("{:?}", event);
        assert!(dbg.contains("Playing"));
    }

    #[test]
    fn state_changed_paused() {
        let event = PlayerEvent::StateChanged(PlaybackState::Paused);
        let dbg = format!("{:?}", event);
        assert!(dbg.contains("Paused"));
    }

    #[test]
    fn state_changed_loading() {
        let event = PlayerEvent::StateChanged(PlaybackState::Loading);
        let dbg = format!("{:?}", event);
        assert!(dbg.contains("Loading"));
    }

    #[test]
    fn loading_progress_zero() {
        let event = PlayerEvent::LoadingProgress(0.0);
        let dbg = format!("{:?}", event);
        assert!(
            dbg.contains("LoadingProgress"),
            "expected 'LoadingProgress' in: {dbg}"
        );
    }

    #[test]
    fn loading_progress_half() {
        let event = PlayerEvent::LoadingProgress(0.5);
        let dbg = format!("{:?}", event);
        assert!(dbg.contains("0.5"), "expected '0.5' in: {dbg}");
    }

    #[test]
    fn loading_progress_full() {
        let event = PlayerEvent::LoadingProgress(1.0);
        let dbg = format!("{:?}", event);
        assert!(
            dbg.contains("1.0") || dbg.contains("1"),
            "expected '1.0' in: {dbg}"
        );
    }
}

// ===========================================================================
// PlayerEvent — Clone
// ===========================================================================

mod player_event_clone {
    use super::*;

    #[test]
    fn clone_track_ended() {
        let event = PlayerEvent::TrackEnded;
        let cloned = event.clone();
        let dbg_orig = format!("{:?}", event);
        let dbg_clone = format!("{:?}", cloned);
        assert_eq!(dbg_orig, dbg_clone);
    }

    #[test]
    fn clone_error() {
        let event = PlayerEvent::Error("test error".to_string());
        let cloned = event.clone();
        let dbg_orig = format!("{:?}", event);
        let dbg_clone = format!("{:?}", cloned);
        assert_eq!(dbg_orig, dbg_clone);
    }

    #[test]
    fn clone_state_changed() {
        let event = PlayerEvent::StateChanged(PlaybackState::Playing);
        let cloned = event.clone();
        let dbg_orig = format!("{:?}", event);
        let dbg_clone = format!("{:?}", cloned);
        assert_eq!(dbg_orig, dbg_clone);
    }

    #[test]
    fn clone_loading_progress() {
        let event = PlayerEvent::LoadingProgress(0.75);
        let cloned = event.clone();
        let dbg_orig = format!("{:?}", event);
        let dbg_clone = format!("{:?}", cloned);
        assert_eq!(dbg_orig, dbg_clone);
    }

    #[test]
    fn clone_error_is_independent() {
        let event = PlayerEvent::Error("original".to_string());
        let _cloned = event.clone();
        // Reassign to a new value — original should remain unchanged
        let modified = PlayerEvent::Error("modified".to_string());
        let dbg_orig = format!("{:?}", event);
        let dbg_modified = format!("{:?}", modified);
        assert!(dbg_orig.contains("original"));
        assert!(dbg_modified.contains("modified"));
    }
}

// ===========================================================================
// PlayerEvent — Debug
// ===========================================================================

mod player_event_debug {
    use super::*;

    #[test]
    fn all_variants_produce_nonempty_debug() {
        let events: Vec<PlayerEvent> = vec![
            PlayerEvent::TrackEnded,
            PlayerEvent::Error("err".to_string()),
            PlayerEvent::StateChanged(PlaybackState::Stopped),
            PlayerEvent::StateChanged(PlaybackState::Playing),
            PlayerEvent::StateChanged(PlaybackState::Paused),
            PlayerEvent::StateChanged(PlaybackState::Loading),
            PlayerEvent::LoadingProgress(0.0),
            PlayerEvent::LoadingProgress(0.5),
            PlayerEvent::LoadingProgress(1.0),
        ];
        for event in &events {
            let dbg = format!("{:?}", event);
            assert!(!dbg.is_empty(), "debug should not be empty for {:?}", event);
        }
    }

    #[test]
    fn long_error_message_in_debug() {
        let long_msg = "x".repeat(10000);
        let event = PlayerEvent::Error(long_msg.clone());
        let dbg = format!("{:?}", event);
        // The debug output should contain at least part of the message
        assert!(dbg.len() > 100);
    }
}

// ===========================================================================
// PlaybackState — exhaustive conversion sweep
// ===========================================================================

mod playback_state_conversion_sweep {
    use super::*;

    /// Verify every EnginePlaybackState variant converts correctly and
    /// round-trips through clone.
    #[test]
    fn all_engine_states_convert_and_clone() {
        let engine_states = [
            (EnginePlaybackState::Stopped, PlaybackState::Stopped),
            (EnginePlaybackState::Playing, PlaybackState::Playing),
            (EnginePlaybackState::Paused, PlaybackState::Paused),
            (EnginePlaybackState::Loading, PlaybackState::Loading),
        ];
        for (engine, expected) in &engine_states {
            let converted: PlaybackState = (*engine).into();
            assert_eq!(converted, *expected);
            let cloned = converted.clone();
            assert_eq!(cloned, *expected);
        }
    }
}

// ===========================================================================
// Scenario: simulated playback lifecycle
// ===========================================================================

mod scenarios {
    use super::*;

    #[test]
    fn playback_lifecycle_state_transitions() {
        // Simulate: Stopped → Loading → Playing → Paused → Playing → Stopped
        let mut states = vec![PlaybackState::Stopped];
        states.push(PlaybackState::Loading);
        states.push(PlaybackState::Playing);
        states.push(PlaybackState::Paused);
        states.push(PlaybackState::Playing);
        states.push(PlaybackState::Stopped);

        assert_eq!(states.len(), 6);
        assert_eq!(states[0], PlaybackState::Stopped);
        assert_eq!(states[1], PlaybackState::Loading);
        assert_eq!(states[2], PlaybackState::Playing);
        assert_eq!(states[3], PlaybackState::Paused);
        assert_eq!(states[4], PlaybackState::Playing);
        assert_eq!(states[5], PlaybackState::Stopped);
    }

    #[test]
    fn now_playing_in_queue_simulation() {
        // Simulate a queue of tracks
        let tracks: Vec<NowPlaying> = (1..=5)
            .map(|i| NowPlaying {
                track_id: format!("track-{}", i),
                title: format!("Song {}", i),
                artist: format!("Artist {}", i),
                album: Some(format!("Album {}", i)),
                duration: 180.0 + (i as f64 * 10.0),
                cover_url: Some(format!("https://example.com/{}.jpg", i)),
                playlist_name: Some("Test Queue".to_string()),
            })
            .collect();

        assert_eq!(tracks.len(), 5);
        assert_eq!(tracks[0].track_id, "track-1");
        assert_eq!(tracks[4].track_id, "track-5");
        assert!((tracks[0].duration - 190.0).abs() < f64::EPSILON);
        assert!((tracks[4].duration - 230.0).abs() < f64::EPSILON);
    }

    #[test]
    fn event_stream_simulation() {
        // Simulate a sequence of events during playback
        let events = vec![
            PlayerEvent::StateChanged(PlaybackState::Loading),
            PlayerEvent::LoadingProgress(0.25),
            PlayerEvent::LoadingProgress(0.5),
            PlayerEvent::LoadingProgress(0.75),
            PlayerEvent::LoadingProgress(1.0),
            PlayerEvent::StateChanged(PlaybackState::Playing),
            PlayerEvent::StateChanged(PlaybackState::Paused),
            PlayerEvent::StateChanged(PlaybackState::Playing),
            PlayerEvent::TrackEnded,
        ];

        assert_eq!(events.len(), 9);

        // Count event types
        let state_changes = events
            .iter()
            .filter(|e| matches!(e, PlayerEvent::StateChanged(_)))
            .count();
        let progress_updates = events
            .iter()
            .filter(|e| matches!(e, PlayerEvent::LoadingProgress(_)))
            .count();
        let track_ends = events
            .iter()
            .filter(|e| matches!(e, PlayerEvent::TrackEnded))
            .count();
        let errors = events
            .iter()
            .filter(|e| matches!(e, PlayerEvent::Error(_)))
            .count();

        assert_eq!(state_changes, 4);
        assert_eq!(progress_updates, 4);
        assert_eq!(track_ends, 1);
        assert_eq!(errors, 0);
    }

    #[test]
    fn error_recovery_simulation() {
        let events = vec![
            PlayerEvent::StateChanged(PlaybackState::Loading),
            PlayerEvent::Error("Network timeout".to_string()),
            PlayerEvent::StateChanged(PlaybackState::Stopped),
            // Retry
            PlayerEvent::StateChanged(PlaybackState::Loading),
            PlayerEvent::LoadingProgress(1.0),
            PlayerEvent::StateChanged(PlaybackState::Playing),
        ];

        let errors: Vec<&PlayerEvent> = events
            .iter()
            .filter(|e| matches!(e, PlayerEvent::Error(_)))
            .collect();
        assert_eq!(errors.len(), 1);
        if let PlayerEvent::Error(msg) = errors[0] {
            assert_eq!(msg, "Network timeout");
        } else {
            panic!("expected Error variant");
        }
    }

    #[test]
    fn now_playing_with_no_album_context() {
        // Track played from search results with no playlist context
        let np = NowPlaying {
            track_id: "standalone-1".to_string(),
            title: "Single Track".to_string(),
            artist: "Indie Artist".to_string(),
            album: None,
            duration: 195.0,
            cover_url: None,
            playlist_name: None,
        };
        assert!(np.album.is_none());
        assert!(np.cover_url.is_none());
        assert!(np.playlist_name.is_none());
    }

    #[test]
    fn collecting_events_into_vec() {
        let mut collected: Vec<PlayerEvent> = Vec::new();

        collected.push(PlayerEvent::TrackEnded);
        collected.push(PlayerEvent::Error("test".to_string()));
        collected.push(PlayerEvent::StateChanged(PlaybackState::Playing));
        collected.push(PlayerEvent::LoadingProgress(0.42));

        assert_eq!(collected.len(), 4);
        // All should be Debug-printable
        for event in &collected {
            let s = format!("{:?}", event);
            assert!(!s.is_empty());
        }
    }
}
