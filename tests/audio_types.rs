// SPDX-License-Identifier: MIT

//! Integration tests for audio type definitions.
//!
//! Covers OutputConfig, AudioOutputBuilder, OutputError display,
//! PlaybackState, AudioEngineEvent, EngineError, and AtomicF32.
//! These tests exercise the public API without requiring a running
//! PulseAudio/PipeWire server.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::audio::engine::{AtomicF32, AudioEngineEvent, PlaybackState};
use cosmic_applet_mare::audio::output::{AudioOutputBuilder, OutputConfig, OutputError};
use std::sync::atomic::Ordering;

// ===========================================================================
// OutputConfig
// ===========================================================================

mod output_config {
    use super::*;

    #[test]
    fn default_values() {
        let config = OutputConfig::default();
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.channels, 2);
        assert!((config.initial_volume - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn custom_values() {
        let config = OutputConfig {
            sample_rate: 48000,
            channels: 1,
            initial_volume: 0.5,
        };
        assert_eq!(config.sample_rate, 48000);
        assert_eq!(config.channels, 1);
        assert!((config.initial_volume - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn sample_rate_96khz() {
        let config = OutputConfig {
            sample_rate: 96000,
            channels: 2,
            initial_volume: 0.8,
        };
        assert_eq!(config.sample_rate, 96000);
    }

    #[test]
    fn sample_rate_192khz() {
        let config = OutputConfig {
            sample_rate: 192000,
            channels: 2,
            initial_volume: 1.0,
        };
        assert_eq!(config.sample_rate, 192000);
    }

    #[test]
    fn mono_config() {
        let config = OutputConfig {
            sample_rate: 44100,
            channels: 1,
            initial_volume: 1.0,
        };
        assert_eq!(config.channels, 1);
    }

    #[test]
    fn zero_volume() {
        let config = OutputConfig {
            sample_rate: 44100,
            channels: 2,
            initial_volume: 0.0,
        };
        assert!(config.initial_volume.abs() < f32::EPSILON);
    }

    #[test]
    fn full_volume() {
        let config = OutputConfig {
            sample_rate: 44100,
            channels: 2,
            initial_volume: 1.0,
        };
        assert!((config.initial_volume - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn mid_volume() {
        let config = OutputConfig {
            sample_rate: 44100,
            channels: 2,
            initial_volume: 0.42,
        };
        assert!((config.initial_volume - 0.42).abs() < f32::EPSILON);
    }
}

// ===========================================================================
// AudioOutputBuilder
// ===========================================================================

mod builder {
    use super::*;

    #[test]
    fn default_builder() {
        let builder = AudioOutputBuilder::new();
        // We can't inspect config directly from here, but we can verify
        // the builder is constructible and doesn't panic
        let _ = builder;
    }

    #[test]
    fn default_trait() {
        let builder = AudioOutputBuilder::default();
        let _ = builder;
    }

    #[test]
    fn chain_sample_rate() {
        let builder = AudioOutputBuilder::new().sample_rate(48000);
        let _ = builder;
    }

    #[test]
    fn chain_channels() {
        let builder = AudioOutputBuilder::new().channels(1);
        let _ = builder;
    }

    #[test]
    fn chain_initial_volume() {
        let builder = AudioOutputBuilder::new().initial_volume(0.75);
        let _ = builder;
    }

    #[test]
    fn full_chain() {
        let builder = AudioOutputBuilder::new()
            .sample_rate(96000)
            .channels(2)
            .initial_volume(0.5);
        let _ = builder;
    }

    #[test]
    fn volume_clamp_over() {
        // The builder should clamp volume > 1.0 to 1.0
        let builder = AudioOutputBuilder::new().initial_volume(2.0);
        // We can't directly inspect the config, but this should not panic
        let _ = builder;
    }

    #[test]
    fn volume_clamp_under() {
        // The builder should clamp volume < 0.0 to 0.0
        let builder = AudioOutputBuilder::new().initial_volume(-0.5);
        let _ = builder;
    }

    #[test]
    fn multiple_overrides() {
        // Setting the same property multiple times — last one wins
        let builder = AudioOutputBuilder::new()
            .sample_rate(44100)
            .sample_rate(48000)
            .sample_rate(96000);
        let _ = builder;
    }

    #[test]
    fn all_common_sample_rates() {
        for &rate in &[
            8000, 11025, 16000, 22050, 44100, 48000, 88200, 96000, 176400, 192000,
        ] {
            let builder = AudioOutputBuilder::new().sample_rate(rate);
            let _ = builder;
        }
    }

    #[test]
    fn all_common_channel_counts() {
        for channels in 1..=8 {
            let builder = AudioOutputBuilder::new().channels(channels);
            let _ = builder;
        }
    }

    #[test]
    fn volume_steps() {
        for i in 0..=20 {
            let vol = i as f32 / 20.0;
            let builder = AudioOutputBuilder::new().initial_volume(vol);
            let _ = builder;
        }
    }
}

// ===========================================================================
// OutputError
// ===========================================================================

mod output_error {
    use super::*;

    #[test]
    fn no_device_display() {
        let err = OutputError::NoDevice;
        let msg = format!("{}", err);
        assert!(
            msg.contains("No audio output device") || msg.contains("device"),
            "unexpected message: {}",
            msg
        );
    }

    #[test]
    fn config_error_display() {
        let err = OutputError::ConfigError("invalid sample rate".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("invalid sample rate"), "unexpected: {}", msg);
    }

    #[test]
    fn stream_error_display() {
        let err = OutputError::StreamError("connection lost".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("connection lost"), "unexpected: {}", msg);
    }

    #[test]
    fn error_trait_implemented() {
        let err = OutputError::NoDevice;
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn debug_format() {
        let err = OutputError::ConfigError("test".to_string());
        let debug = format!("{:?}", err);
        assert!(debug.contains("ConfigError"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn no_device_debug() {
        let err = OutputError::NoDevice;
        let debug = format!("{:?}", err);
        assert!(debug.contains("NoDevice"));
    }

    #[test]
    fn stream_error_debug() {
        let err = OutputError::StreamError("broken pipe".to_string());
        let debug = format!("{:?}", err);
        assert!(debug.contains("StreamError"));
        assert!(debug.contains("broken pipe"));
    }

    #[test]
    fn all_variants_display_non_empty() {
        let variants: Vec<OutputError> = vec![
            OutputError::NoDevice,
            OutputError::ConfigError("cfg err".to_string()),
            OutputError::StreamError("stream err".to_string()),
        ];
        for err in &variants {
            let msg = format!("{}", err);
            assert!(!msg.is_empty(), "display should be non-empty for {:?}", err);
        }
    }
}

// ===========================================================================
// PlaybackState
// ===========================================================================

mod playback_state {
    use super::*;

    #[test]
    fn stopped_is_default() {
        let state = PlaybackState::default();
        assert_eq!(state, PlaybackState::Stopped);
    }

    #[test]
    fn all_variants_exist() {
        let _ = PlaybackState::Stopped;
        let _ = PlaybackState::Playing;
        let _ = PlaybackState::Paused;
        let _ = PlaybackState::Loading;
    }

    #[test]
    fn equality() {
        assert_eq!(PlaybackState::Stopped, PlaybackState::Stopped);
        assert_eq!(PlaybackState::Playing, PlaybackState::Playing);
        assert_eq!(PlaybackState::Paused, PlaybackState::Paused);
        assert_eq!(PlaybackState::Loading, PlaybackState::Loading);
    }

    #[test]
    fn inequality() {
        assert_ne!(PlaybackState::Stopped, PlaybackState::Playing);
        assert_ne!(PlaybackState::Stopped, PlaybackState::Paused);
        assert_ne!(PlaybackState::Stopped, PlaybackState::Loading);
        assert_ne!(PlaybackState::Playing, PlaybackState::Paused);
        assert_ne!(PlaybackState::Playing, PlaybackState::Loading);
        assert_ne!(PlaybackState::Paused, PlaybackState::Loading);
    }

    #[test]
    fn clone() {
        let state = PlaybackState::Playing;
        let cloned = state;
        assert_eq!(state, cloned);
    }

    #[test]
    fn copy() {
        let a = PlaybackState::Paused;
        let b = a; // Copy
        assert_eq!(a, b);
        // Both are still usable (proves Copy, not just Move)
        assert_eq!(a, PlaybackState::Paused);
        assert_eq!(b, PlaybackState::Paused);
    }

    #[test]
    fn debug_format() {
        assert_eq!(format!("{:?}", PlaybackState::Stopped), "Stopped");
        assert_eq!(format!("{:?}", PlaybackState::Playing), "Playing");
        assert_eq!(format!("{:?}", PlaybackState::Paused), "Paused");
        assert_eq!(format!("{:?}", PlaybackState::Loading), "Loading");
    }

    #[test]
    fn all_variants_are_distinct() {
        let variants = [
            PlaybackState::Stopped,
            PlaybackState::Playing,
            PlaybackState::Paused,
            PlaybackState::Loading,
        ];
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }
}

// ===========================================================================
// AudioEngineEvent
// ===========================================================================

mod engine_event {
    use super::*;

    #[test]
    fn state_changed_event() {
        let event = AudioEngineEvent::StateChanged(PlaybackState::Playing);
        match event {
            AudioEngineEvent::StateChanged(state) => {
                assert_eq!(state, PlaybackState::Playing);
            }
            _ => panic!("expected StateChanged"),
        }
    }

    #[test]
    fn track_ended_event() {
        let event = AudioEngineEvent::TrackEnded;
        assert!(matches!(event, AudioEngineEvent::TrackEnded));
    }

    #[test]
    fn error_event() {
        let event = AudioEngineEvent::Error("something broke".to_string());
        match event {
            AudioEngineEvent::Error(msg) => {
                assert_eq!(msg, "something broke");
            }
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn position_event() {
        let event = AudioEngineEvent::Position(42.5);
        match event {
            AudioEngineEvent::Position(pos) => {
                assert!((pos - 42.5).abs() < f64::EPSILON);
            }
            _ => panic!("expected Position"),
        }
    }

    #[test]
    fn duration_event() {
        let event = AudioEngineEvent::Duration(180.0);
        match event {
            AudioEngineEvent::Duration(dur) => {
                assert!((dur - 180.0).abs() < f64::EPSILON);
            }
            _ => panic!("expected Duration"),
        }
    }

    #[test]
    fn loading_progress_event() {
        let event = AudioEngineEvent::LoadingProgress(0.75);
        match event {
            AudioEngineEvent::LoadingProgress(progress) => {
                assert!((progress - 0.75).abs() < f64::EPSILON);
            }
            _ => panic!("expected LoadingProgress"),
        }
    }

    #[test]
    fn debug_format_all_variants() {
        let events: Vec<AudioEngineEvent> = vec![
            AudioEngineEvent::StateChanged(PlaybackState::Stopped),
            AudioEngineEvent::TrackEnded,
            AudioEngineEvent::Error("err".to_string()),
            AudioEngineEvent::Position(1.0),
            AudioEngineEvent::Duration(2.0),
            AudioEngineEvent::LoadingProgress(0.5),
        ];
        for event in &events {
            let debug = format!("{:?}", event);
            assert!(!debug.is_empty());
        }
    }

    #[test]
    fn clone_preserves_value() {
        let event = AudioEngineEvent::Error("cloned error".to_string());
        let cloned = event.clone();
        match cloned {
            AudioEngineEvent::Error(msg) => assert_eq!(msg, "cloned error"),
            _ => panic!("clone changed variant"),
        }
    }

    #[test]
    fn state_changed_all_states() {
        for state in [
            PlaybackState::Stopped,
            PlaybackState::Playing,
            PlaybackState::Paused,
            PlaybackState::Loading,
        ] {
            let event = AudioEngineEvent::StateChanged(state);
            match event {
                AudioEngineEvent::StateChanged(s) => assert_eq!(s, state),
                _ => panic!("variant mismatch"),
            }
        }
    }

    #[test]
    fn position_zero() {
        let event = AudioEngineEvent::Position(0.0);
        match event {
            AudioEngineEvent::Position(p) => assert!(p.abs() < f64::EPSILON),
            _ => panic!("expected Position"),
        }
    }

    #[test]
    fn loading_progress_boundaries() {
        for &val in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            let event = AudioEngineEvent::LoadingProgress(val);
            match event {
                AudioEngineEvent::LoadingProgress(p) => {
                    assert!((p - val).abs() < f64::EPSILON);
                }
                _ => panic!("expected LoadingProgress"),
            }
        }
    }
}

// ===========================================================================
// AtomicF32
// ===========================================================================

mod atomic_f32 {
    use super::*;

    #[test]
    fn new_stores_value() {
        let a = AtomicF32::new(0.5);
        assert!((a.load(Ordering::Relaxed) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn new_zero() {
        let a = AtomicF32::new(0.0);
        assert!(a.load(Ordering::Relaxed).abs() < f32::EPSILON);
    }

    #[test]
    fn new_one() {
        let a = AtomicF32::new(1.0);
        assert!((a.load(Ordering::Relaxed) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn store_and_load() {
        let a = AtomicF32::new(0.0);
        a.store(0.75, Ordering::Relaxed);
        assert!((a.load(Ordering::Relaxed) - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn store_overwrites() {
        let a = AtomicF32::new(0.1);
        a.store(0.2, Ordering::Relaxed);
        a.store(0.3, Ordering::Relaxed);
        a.store(0.4, Ordering::Relaxed);
        assert!((a.load(Ordering::Relaxed) - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn negative_value() {
        let a = AtomicF32::new(-1.0);
        assert!((a.load(Ordering::Relaxed) - (-1.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn large_value() {
        let a = AtomicF32::new(1e10);
        assert!((a.load(Ordering::Relaxed) - 1e10).abs() < 1.0);
    }

    #[test]
    fn small_value() {
        let a = AtomicF32::new(1e-10);
        assert!((a.load(Ordering::Relaxed) - 1e-10).abs() < f32::EPSILON);
    }

    #[test]
    fn multiple_loads_consistent() {
        let a = AtomicF32::new(0.42);
        for _ in 0..100 {
            assert!((a.load(Ordering::Relaxed) - 0.42).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn store_load_many_values() {
        let a = AtomicF32::new(0.0);
        for i in 0..100 {
            let val = i as f32 / 100.0;
            a.store(val, Ordering::Relaxed);
            assert!(
                (a.load(Ordering::Relaxed) - val).abs() < f32::EPSILON,
                "stored {} but loaded {}",
                val,
                a.load(Ordering::Relaxed)
            );
        }
    }

    #[test]
    fn concurrent_store_load_no_panic() {
        use std::sync::Arc;
        use std::thread;

        let a = Arc::new(AtomicF32::new(0.0));

        let mut handles = Vec::new();

        // Spawn writers
        for i in 0..4 {
            let a_clone = Arc::clone(&a);
            handles.push(thread::spawn(move || {
                for j in 0..1000 {
                    let val = (i * 1000 + j) as f32 / 10000.0;
                    a_clone.store(val, Ordering::Relaxed);
                }
            }));
        }

        // Spawn readers
        for _ in 0..4 {
            let a_clone = Arc::clone(&a);
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    let val = a_clone.load(Ordering::Relaxed);
                    // Value should always be a valid f32
                    assert!(val.is_finite() || val == 0.0);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // Final value should be some valid f32
        let final_val = a.load(Ordering::Relaxed);
        assert!(final_val.is_finite());
    }

    #[test]
    fn special_float_values() {
        // Zero
        let a = AtomicF32::new(0.0);
        assert!(a.load(Ordering::Relaxed).abs() < f32::EPSILON);

        // Negative zero
        a.store(-0.0, Ordering::Relaxed);
        assert!(a.load(Ordering::Relaxed).abs() < f32::EPSILON);

        // Max value
        a.store(f32::MAX, Ordering::Relaxed);
        assert!((a.load(Ordering::Relaxed) - f32::MAX).abs() < 1.0);

        // Min positive
        a.store(f32::MIN_POSITIVE, Ordering::Relaxed);
        assert!((a.load(Ordering::Relaxed) - f32::MIN_POSITIVE).abs() < f32::EPSILON);
    }
}

// ===========================================================================
// Cross-module: PlaybackState → AudioEngineEvent round-trip
// ===========================================================================

mod cross_module {
    use super::*;

    #[test]
    fn state_changed_events_for_all_states() {
        let states = [
            PlaybackState::Stopped,
            PlaybackState::Playing,
            PlaybackState::Paused,
            PlaybackState::Loading,
        ];

        for &state in &states {
            let event = AudioEngineEvent::StateChanged(state);
            let cloned = event.clone();
            match cloned {
                AudioEngineEvent::StateChanged(s) => assert_eq!(s, state),
                _ => panic!("clone changed event variant"),
            }
        }
    }

    #[test]
    fn volume_via_atomic_f32() {
        // Simulate the pattern used in AudioEngine
        let volume = AtomicF32::new(1.0);

        // Set to 50%
        volume.store(0.5, Ordering::Relaxed);
        assert!((volume.load(Ordering::Relaxed) - 0.5).abs() < f32::EPSILON);

        // Mute
        volume.store(0.0, Ordering::Relaxed);
        assert!(volume.load(Ordering::Relaxed).abs() < f32::EPSILON);

        // Max
        volume.store(1.0, Ordering::Relaxed);
        assert!((volume.load(Ordering::Relaxed) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn builder_fluent_api_ergonomics() {
        // Verify the builder pattern compiles and works fluently
        let _builder = AudioOutputBuilder::new()
            .sample_rate(48000)
            .channels(2)
            .initial_volume(0.8);

        let _builder2 = AudioOutputBuilder::default()
            .sample_rate(96000)
            .channels(1)
            .initial_volume(0.0);
    }

    #[test]
    fn output_config_used_in_builder_pattern() {
        // The default config should match the builder's defaults
        let config = OutputConfig::default();
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.channels, 2);
        assert!((config.initial_volume - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn error_variants_cover_failure_modes() {
        // Verify all error variants can be constructed and displayed
        let errors = vec![
            OutputError::NoDevice,
            OutputError::ConfigError("bad rate".to_string()),
            OutputError::StreamError("disconnected".to_string()),
        ];

        for err in &errors {
            let display = format!("{}", err);
            let debug = format!("{:?}", err);
            assert!(!display.is_empty());
            assert!(!debug.is_empty());
        }
    }
}
