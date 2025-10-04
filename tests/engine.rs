// SPDX-License-Identifier: MIT

//! Integration tests for `audio::engine` public API.
//!
//! These tests exercise `AudioEngine`, `PlaybackState`, `AudioEngineEvent`,
//! `EngineError`, and `AtomicF32` **without** requiring a running audio output
//! server.  Error paths (invalid files, unreachable URLs) are exercised through
//! the real playback loop — they fail at the decoder stage before any
//! `AudioOutput` is created, so they work in headless CI.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use cosmic_applet_mare::audio::engine::{
    AtomicF32, AudioEngine, AudioEngineCommand, AudioEngineEvent, EngineError, PlaybackState,
};

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Drain events from the engine until we find one matching `predicate`,
/// or time out after `timeout`.
fn wait_for_event<F>(engine: &AudioEngine, timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut(&AudioEngineEvent) -> bool,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(evt) = engine.try_recv_event() {
            if predicate(&evt) {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Collect all events within `duration`, returning them as a Vec.
fn collect_events(engine: &AudioEngine, duration: Duration) -> Vec<AudioEngineEvent> {
    let mut events = Vec::new();
    let start = Instant::now();
    while start.elapsed() < duration {
        if let Some(evt) = engine.try_recv_event() {
            events.push(evt);
        } else {
            thread::sleep(Duration::from_millis(5));
        }
    }
    events
}

/// Spawn an HTTP server that responds with an error status.
fn spawn_http_error(status: u16) -> (String, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{}/audio", port);

    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut req = [0u8; 4096];
            let _ = stream.read(&mut req);
            let resp = format!(
                "HTTP/1.1 {} Error\r\n\
                 Content-Length: 0\r\n\
                 Connection: close\r\n\
                 \r\n",
                status,
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    thread::sleep(Duration::from_millis(20));
    (url, port)
}

/// Spawn an HTTP server that serves garbage bytes (not valid audio).
fn spawn_http_garbage() -> (String, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{}/audio", port);

    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut req = [0u8; 4096];
            let _ = stream.read(&mut req);
            let garbage = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02, 0x03];
            let resp = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: audio/mpeg\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n",
                garbage.len(),
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(&garbage);
            let _ = stream.flush();
        }
    });
    thread::sleep(Duration::from_millis(20));
    (url, port)
}

// ═══════════════════════════════════════════════════════════════════════════
// PlaybackState
// ═══════════════════════════════════════════════════════════════════════════

mod playback_state {
    use super::*;

    #[test]
    fn default_is_stopped() {
        assert_eq!(PlaybackState::default(), PlaybackState::Stopped);
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

    #[test]
    fn clone_preserves_value() {
        let state = PlaybackState::Playing;
        let cloned = state;
        assert_eq!(state, cloned);
    }

    #[test]
    fn debug_format() {
        assert_eq!(format!("{:?}", PlaybackState::Stopped), "Stopped");
        assert_eq!(format!("{:?}", PlaybackState::Playing), "Playing");
        assert_eq!(format!("{:?}", PlaybackState::Paused), "Paused");
        assert_eq!(format!("{:?}", PlaybackState::Loading), "Loading");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngineEvent
// ═══════════════════════════════════════════════════════════════════════════

mod engine_event {
    use super::*;

    #[test]
    fn state_changed_debug() {
        let e = AudioEngineEvent::StateChanged(PlaybackState::Playing);
        let dbg = format!("{:?}", e);
        assert!(dbg.contains("StateChanged"));
        assert!(dbg.contains("Playing"));
    }

    #[test]
    fn track_ended_clone() {
        let e = AudioEngineEvent::TrackEnded;
        let e2 = e.clone();
        assert!(format!("{:?}", e2).contains("TrackEnded"));
    }

    #[test]
    fn error_event_clone() {
        let e = AudioEngineEvent::Error("boom".into());
        let e2 = e.clone();
        let dbg = format!("{:?}", e2);
        assert!(dbg.contains("boom"));
    }

    #[test]
    fn position_event() {
        let e = AudioEngineEvent::Position(99.5);
        assert!(format!("{:?}", e.clone()).contains("99.5"));
    }

    #[test]
    fn duration_event() {
        let e = AudioEngineEvent::Duration(300.0);
        assert!(format!("{:?}", e.clone()).contains("300"));
    }

    #[test]
    fn loading_progress_event() {
        let e = AudioEngineEvent::LoadingProgress(0.75);
        assert!(format!("{:?}", e.clone()).contains("0.75"));
    }

    #[test]
    fn all_variants_debug_non_empty() {
        let events: Vec<AudioEngineEvent> = vec![
            AudioEngineEvent::StateChanged(PlaybackState::Stopped),
            AudioEngineEvent::TrackEnded,
            AudioEngineEvent::Error("test".into()),
            AudioEngineEvent::Position(0.0),
            AudioEngineEvent::Duration(0.0),
            AudioEngineEvent::LoadingProgress(0.0),
        ];
        for e in events {
            assert!(!format!("{:?}", e).is_empty());
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngineCommand
// ═══════════════════════════════════════════════════════════════════════════

mod engine_command {
    use super::*;

    #[test]
    fn play_url_debug() {
        let cmd = AudioEngineCommand::PlayUrl {
            url: "https://cdn.example.com/t.mp3".into(),
            cache_path: Some("/tmp/t.mp3".into()),
            replay_gain_db: None,
        };
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("PlayUrl"));
        assert!(dbg.contains("cdn.example.com"));
    }

    #[test]
    fn play_dash_debug() {
        let cmd = AudioEngineCommand::PlayDash {
            manifest_path: "/tmp/m.mpd".into(),
            cache_path: None,
            replay_gain_db: None,
        };
        assert!(format!("{:?}", cmd).contains("PlayDash"));
    }

    #[test]
    fn play_file_debug() {
        let cmd = AudioEngineCommand::PlayFile {
            path: "/music/a.flac".into(),
            replay_gain_db: None,
        };
        assert!(format!("{:?}", cmd).contains("PlayFile"));
    }

    #[test]
    fn simple_variants_debug() {
        assert!(format!("{:?}", AudioEngineCommand::Pause).contains("Pause"));
        assert!(format!("{:?}", AudioEngineCommand::Resume).contains("Resume"));
        assert!(format!("{:?}", AudioEngineCommand::Stop).contains("Stop"));
        assert!(format!("{:?}", AudioEngineCommand::Shutdown).contains("Shutdown"));
    }

    #[test]
    fn seek_debug() {
        let cmd = AudioEngineCommand::Seek(33.3);
        assert!(format!("{:?}", cmd).contains("33.3"));
    }

    #[test]
    fn set_volume_debug() {
        let cmd = AudioEngineCommand::SetVolume(0.5);
        assert!(format!("{:?}", cmd).contains("0.5"));
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// EngineError
// ═══════════════════════════════════════════════════════════════════════════

mod engine_error {
    use super::*;
    use cosmic_applet_mare::audio::dash::DashError;
    use cosmic_applet_mare::audio::decoder::DecoderError;
    use cosmic_applet_mare::audio::output::OutputError;

    #[test]
    fn display_channel() {
        let err = EngineError::Channel("broken pipe".into());
        let msg = err.to_string();
        assert!(msg.contains("Channel error:"), "got: {msg}");
        assert!(msg.contains("broken pipe"), "got: {msg}");
    }

    #[test]
    fn display_decoder() {
        let err = EngineError::Decoder(DecoderError::NoAudioTrack);
        assert!(err.to_string().starts_with("Decoder error:"));
    }

    #[test]
    fn display_output() {
        let err = EngineError::Output(OutputError::NoDevice);
        assert!(err.to_string().starts_with("Output error:"));
    }

    #[test]
    fn display_dash() {
        let err = EngineError::Dash(DashError::NoAudioTrack);
        assert!(err.to_string().starts_with("DASH error:"));
    }

    #[test]
    fn implements_std_error() {
        let err: Box<dyn std::error::Error> = Box::new(EngineError::Channel("x".into()));
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn debug_is_non_empty() {
        let err = EngineError::Channel("y".into());
        assert!(!format!("{:?}", err).is_empty());
    }

    #[test]
    fn from_decoder_error() {
        let inner = DecoderError::NoAudioTrack;
        let err: EngineError = inner.into();
        assert!(matches!(err, EngineError::Decoder(_)));
    }

    #[test]
    fn from_output_error() {
        let inner = OutputError::ConfigError("bad".into());
        let err: EngineError = inner.into();
        assert!(matches!(err, EngineError::Output(_)));
    }

    #[test]
    fn from_dash_error() {
        let inner = DashError::ParseError("xml".into());
        let err: EngineError = inner.into();
        assert!(matches!(err, EngineError::Dash(_)));
    }

    #[test]
    fn from_preserves_inner_message() {
        let inner = DashError::InvalidManifest("missing segment".into());
        let err: EngineError = inner.into();
        assert!(err.to_string().contains("missing segment"));
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AtomicF32
// ═══════════════════════════════════════════════════════════════════════════

mod atomic_f32 {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn new_and_load() {
        let a = AtomicF32::new(0.5);
        let v = a.load(Ordering::SeqCst);
        assert!((v - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn store_and_load() {
        let a = AtomicF32::new(0.0);
        a.store(0.75, Ordering::SeqCst);
        assert!((a.load(Ordering::SeqCst) - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn roundtrip_zero() {
        let a = AtomicF32::new(0.0);
        assert!(a.load(Ordering::SeqCst).abs() < f32::EPSILON);
    }

    #[test]
    fn roundtrip_one() {
        let a = AtomicF32::new(1.0);
        assert!((a.load(Ordering::SeqCst) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn negative_value() {
        let a = AtomicF32::new(-3.14);
        assert!((a.load(Ordering::SeqCst) - (-3.14)).abs() < 0.01);
    }

    #[test]
    fn overwrite() {
        let a = AtomicF32::new(0.1);
        a.store(0.9, Ordering::SeqCst);
        a.store(0.42, Ordering::SeqCst);
        assert!((a.load(Ordering::SeqCst) - 0.42).abs() < f32::EPSILON);
    }

    #[test]
    fn shared_across_threads() {
        let a = Arc::new(AtomicF32::new(0.0));
        let a2 = Arc::clone(&a);

        let handle = thread::spawn(move || {
            a2.store(0.99, Ordering::SeqCst);
        });
        handle.join().unwrap();

        assert!((a.load(Ordering::SeqCst) - 0.99).abs() < 0.01);
    }

    #[test]
    fn concurrent_writes_no_panic() {
        let a = Arc::new(AtomicF32::new(0.0));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let a2 = Arc::clone(&a);
                thread::spawn(move || {
                    a2.store(i as f32 * 0.1, Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // Value should be one of the written values (0.0 .. 0.7)
        let v = a.load(Ordering::SeqCst);
        assert!(v >= 0.0 && v <= 0.7);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — construction & initial state
// ═══════════════════════════════════════════════════════════════════════════

mod engine_construction {
    use super::*;

    #[test]
    fn new_succeeds() {
        let engine = AudioEngine::new();
        assert!(engine.is_ok(), "AudioEngine::new() should succeed");
    }

    #[test]
    fn initial_state_is_stopped() {
        let engine = AudioEngine::new().unwrap();
        assert_eq!(engine.state(), PlaybackState::Stopped);
    }

    #[test]
    fn initial_is_playing_false() {
        let engine = AudioEngine::new().unwrap();
        assert!(!engine.is_playing());
    }

    #[test]
    fn initial_position_is_zero() {
        let engine = AudioEngine::new().unwrap();
        assert!((engine.position() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn initial_volume_is_one() {
        let engine = AudioEngine::new().unwrap();
        assert!((engine.volume() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn initial_no_events() {
        let engine = AudioEngine::new().unwrap();
        assert!(engine.try_recv_event().is_none());
    }

    #[test]
    fn initial_spectrum_all_zeros() {
        let engine = AudioEngine::new().unwrap();
        let spectrum = engine.spectrum();
        assert!(spectrum.left_bands.iter().all(|&v| v == 0.0));
        assert!(spectrum.right_bands.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn drop_is_graceful() {
        // Create and immediately drop — should not panic or hang.
        let engine = AudioEngine::new().unwrap();
        drop(engine);
    }

    #[test]
    fn multiple_engines_sequential() {
        // Creating engines in sequence should work fine.
        for _ in 0..3 {
            let engine = AudioEngine::new().unwrap();
            assert_eq!(engine.state(), PlaybackState::Stopped);
            drop(engine);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — volume control
// ═══════════════════════════════════════════════════════════════════════════

mod engine_volume {
    use super::*;

    #[test]
    fn set_volume_updates_getter() {
        let engine = AudioEngine::new().unwrap();
        // set_volume stores the clamped value immediately (before command)
        engine.set_volume(0.5).unwrap();
        assert!((engine.volume() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn set_volume_clamps_high() {
        let engine = AudioEngine::new().unwrap();
        engine.set_volume(2.0).unwrap();
        assert!((engine.volume() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn set_volume_clamps_low() {
        let engine = AudioEngine::new().unwrap();
        engine.set_volume(-0.5).unwrap();
        assert!(engine.volume().abs() < f32::EPSILON);
    }

    #[test]
    fn set_volume_zero() {
        let engine = AudioEngine::new().unwrap();
        engine.set_volume(0.0).unwrap();
        assert!(engine.volume().abs() < f32::EPSILON);
    }

    #[test]
    fn set_volume_one() {
        let engine = AudioEngine::new().unwrap();
        engine.set_volume(0.3).unwrap();
        engine.set_volume(1.0).unwrap();
        assert!((engine.volume() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn set_volume_multiple_times() {
        let engine = AudioEngine::new().unwrap();
        for v in [0.0, 0.25, 0.5, 0.75, 1.0] {
            engine.set_volume(v).unwrap();
            assert!(
                (engine.volume() - v).abs() < f32::EPSILON,
                "volume should be {} but got {}",
                v,
                engine.volume()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — command senders (verify they succeed without panic)
// ═══════════════════════════════════════════════════════════════════════════

mod engine_commands {
    use super::*;

    #[test]
    fn pause_succeeds() {
        let engine = AudioEngine::new().unwrap();
        assert!(engine.pause().is_ok());
    }

    #[test]
    fn resume_succeeds() {
        let engine = AudioEngine::new().unwrap();
        assert!(engine.resume().is_ok());
    }

    #[test]
    fn stop_succeeds() {
        let engine = AudioEngine::new().unwrap();
        assert!(engine.stop().is_ok());
    }

    #[test]
    fn seek_succeeds() {
        let engine = AudioEngine::new().unwrap();
        assert!(engine.seek(10.0).is_ok());
    }

    #[test]
    fn play_url_cached_succeeds() {
        let engine = AudioEngine::new().unwrap();
        assert!(
            engine
                .play_url_cached("http://example.com/t.mp3", Some("/tmp/c.mp3".into()), None)
                .is_ok()
        );
    }

    #[test]
    fn play_dash_cached_succeeds() {
        let engine = AudioEngine::new().unwrap();
        assert!(
            engine
                .play_dash_cached("/tmp/manifest.mpd", Some("/tmp/c.mp4".into()), None)
                .is_ok()
        );
    }

    #[test]
    fn play_file_succeeds() {
        let engine = AudioEngine::new().unwrap();
        assert!(engine.play_file("/tmp/doesnt_exist.mp3", None).is_ok());
    }

    #[test]
    fn shutdown_succeeds() {
        let engine = AudioEngine::new().unwrap();
        assert!(engine.shutdown().is_ok());
    }

    #[test]
    fn seek_via_typed_method() {
        let engine = AudioEngine::new().unwrap();
        assert!(engine.seek(42.0).is_ok());
    }

    #[test]
    fn set_volume_via_typed_method() {
        let engine = AudioEngine::new().unwrap();
        assert!(engine.set_volume(0.5).is_ok());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — toggle_pause logic
// ═══════════════════════════════════════════════════════════════════════════

mod engine_toggle_pause {
    use super::*;

    #[test]
    fn toggle_while_stopped_is_noop() {
        let engine = AudioEngine::new().unwrap();
        assert_eq!(engine.state(), PlaybackState::Stopped);
        // toggle_pause returns Ok(()) for Stopped/Loading — no command sent
        assert!(engine.toggle_pause().is_ok());
        assert_eq!(engine.state(), PlaybackState::Stopped);
    }

    #[test]
    fn toggle_while_loading_is_noop() {
        // We can't directly set state to Loading from outside, but toggle_pause
        // should not change anything for non-Playing/non-Paused states.
        // Since we start Stopped, this effectively tests the same branch.
        let engine = AudioEngine::new().unwrap();
        assert!(engine.toggle_pause().is_ok());
    }

    #[test]
    fn toggle_while_paused_sends_resume() {
        let engine = AudioEngine::new().unwrap();

        // First, put the engine into Paused state via the playback loop
        engine.pause().unwrap();
        let found_paused = wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Paused))
        });
        assert!(found_paused, "expected Paused state");
        assert_eq!(engine.state(), PlaybackState::Paused);

        // Now toggle — should send Resume (Paused → Playing branch)
        engine.toggle_pause().unwrap();
        let found_playing = wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Playing))
        });
        assert!(found_playing, "toggle from Paused should resume to Playing");
    }

    #[test]
    fn toggle_while_playing_sends_pause() {
        let engine = AudioEngine::new().unwrap();

        // First, put the engine into Playing state via the playback loop
        engine.resume().unwrap();
        let found_playing = wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Playing))
        });
        assert!(found_playing, "expected Playing state");
        assert_eq!(engine.state(), PlaybackState::Playing);

        // Now toggle — should send Pause (Playing → Paused branch)
        engine.toggle_pause().unwrap();
        let found_paused = wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Paused))
        });
        assert!(found_paused, "toggle from Playing should pause");
    }

    #[test]
    fn toggle_twice_roundtrips() {
        let engine = AudioEngine::new().unwrap();

        // Get into Playing
        engine.resume().unwrap();
        wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Playing))
        });

        // Toggle → Paused
        engine.toggle_pause().unwrap();
        wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Paused))
        });
        assert_eq!(engine.state(), PlaybackState::Paused);

        // Toggle → Playing again
        engine.toggle_pause().unwrap();
        wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Playing))
        });
        assert_eq!(engine.state(), PlaybackState::Playing);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — error paths through the playback loop
// ═══════════════════════════════════════════════════════════════════════════

mod engine_error_paths {
    use super::*;

    #[test]
    fn play_file_nonexistent_emits_error_event() {
        let engine = AudioEngine::new().unwrap();
        engine
            .play_file("/tmp/__mare_player_test_nonexistent_file_12345.flac", None)
            .unwrap();

        // The playback loop should:
        // 1. Set state to Loading (StateChanged(Loading))
        // 2. Fail at AudioDecoder::from_file
        // 3. Emit Error event
        // 4. Set state back to Stopped (StateChanged(Stopped))
        let found_error = wait_for_event(&engine, Duration::from_secs(5), |evt| {
            matches!(evt, AudioEngineEvent::Error(_))
        });
        assert!(found_error, "expected Error event for nonexistent file");

        // State should eventually be Stopped
        thread::sleep(Duration::from_millis(100));
        assert_eq!(engine.state(), PlaybackState::Stopped);
    }

    #[test]
    fn play_file_empty_path_emits_error() {
        let engine = AudioEngine::new().unwrap();
        engine.play_file("", None).unwrap();

        let found_error = wait_for_event(&engine, Duration::from_secs(5), |evt| {
            matches!(evt, AudioEngineEvent::Error(_))
        });
        assert!(found_error, "expected Error event for empty path");
    }

    #[test]
    fn play_file_nonexistent_goes_through_loading() {
        let engine = AudioEngine::new().unwrap();
        engine
            .play_file("/tmp/__mare_player_nonexistent_99999.mp3", None)
            .unwrap();

        // Collect events for a few seconds
        let events = collect_events(&engine, Duration::from_secs(5));

        // Should see Loading -> Error -> Stopped (though order of receipt may
        // interleave depending on timing)
        let has_loading = events
            .iter()
            .any(|e| matches!(e, AudioEngineEvent::StateChanged(PlaybackState::Loading)));
        let has_error = events
            .iter()
            .any(|e| matches!(e, AudioEngineEvent::Error(_)));
        let has_stopped = events
            .iter()
            .any(|e| matches!(e, AudioEngineEvent::StateChanged(PlaybackState::Stopped)));

        assert!(has_loading, "expected Loading state event");
        assert!(has_error, "expected Error event");
        assert!(has_stopped, "expected Stopped state event");
    }

    #[test]
    fn play_url_unreachable_host_emits_error() {
        let engine = AudioEngine::new().unwrap();
        // Use a non-routable address to get a fast connection-refused or timeout
        engine
            .play_url_cached("http://127.0.0.1:1/__does_not_exist", None, None)
            .unwrap();

        let found_error = wait_for_event(&engine, Duration::from_secs(15), |evt| {
            matches!(evt, AudioEngineEvent::Error(_))
        });
        assert!(found_error, "expected Error event for unreachable URL");
        // Allow loop to settle
        thread::sleep(Duration::from_millis(100));
        assert_eq!(engine.state(), PlaybackState::Stopped);
    }

    #[test]
    fn play_url_http_500_emits_error() {
        let (url, _port) = spawn_http_error(500);
        let engine = AudioEngine::new().unwrap();
        engine.play_url_cached(&url, None, None).unwrap();

        let found_error = wait_for_event(&engine, Duration::from_secs(10), |evt| {
            matches!(evt, AudioEngineEvent::Error(_))
        });
        assert!(found_error, "expected Error event for HTTP 500");
    }

    #[test]
    fn play_url_http_404_emits_error() {
        let (url, _port) = spawn_http_error(404);
        let engine = AudioEngine::new().unwrap();
        engine.play_url_cached(&url, None, None).unwrap();

        let found_error = wait_for_event(&engine, Duration::from_secs(10), |evt| {
            matches!(evt, AudioEngineEvent::Error(_))
        });
        assert!(found_error, "expected Error event for HTTP 404");
    }

    #[test]
    fn play_url_garbage_audio_emits_error() {
        let (url, _port) = spawn_http_garbage();
        let engine = AudioEngine::new().unwrap();
        engine.play_url_cached(&url, None, None).unwrap();

        let found_error = wait_for_event(&engine, Duration::from_secs(10), |evt| {
            matches!(evt, AudioEngineEvent::Error(_))
        });
        assert!(found_error, "expected Error event for garbage audio data");
    }

    #[test]
    fn play_dash_nonexistent_manifest_emits_error() {
        let engine = AudioEngine::new().unwrap();
        engine
            .play_dash_cached(
                "/tmp/__mare_player_nonexistent_manifest_99999.mpd",
                None,
                None,
            )
            .unwrap();

        let found_error = wait_for_event(&engine, Duration::from_secs(5), |evt| {
            matches!(evt, AudioEngineEvent::Error(_))
        });
        assert!(
            found_error,
            "expected Error event for nonexistent DASH manifest"
        );
        thread::sleep(Duration::from_millis(100));
        assert_eq!(engine.state(), PlaybackState::Stopped);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — stop command through playback loop
// ═══════════════════════════════════════════════════════════════════════════

mod engine_stop {
    use super::*;

    #[test]
    fn stop_while_stopped_emits_state_event() {
        let engine = AudioEngine::new().unwrap();
        engine.stop().unwrap();

        // The playback loop processes the Stop command and emits
        // StateChanged(Stopped).
        let found = wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Stopped))
        });
        assert!(found, "expected StateChanged(Stopped) event");
    }

    #[test]
    fn stop_resets_position() {
        let engine = AudioEngine::new().unwrap();
        engine.stop().unwrap();
        thread::sleep(Duration::from_millis(200));
        assert!((engine.position() - 0.0).abs() < f64::EPSILON);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — pause/resume commands through playback loop
// ═══════════════════════════════════════════════════════════════════════════

mod engine_pause_resume {
    use super::*;

    #[test]
    fn pause_emits_paused_state() {
        let engine = AudioEngine::new().unwrap();
        engine.pause().unwrap();

        let found = wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Paused))
        });
        assert!(found, "expected StateChanged(Paused) event");
    }

    #[test]
    fn resume_emits_playing_state() {
        let engine = AudioEngine::new().unwrap();
        engine.resume().unwrap();

        let found = wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Playing))
        });
        assert!(found, "expected StateChanged(Playing) event");
    }

    #[test]
    fn pause_then_resume_emits_both_states() {
        let engine = AudioEngine::new().unwrap();
        engine.pause().unwrap();

        let found_paused = wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Paused))
        });
        assert!(found_paused, "expected Paused");

        engine.resume().unwrap();

        let found_playing = wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Playing))
        });
        assert!(found_playing, "expected Playing");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — seek without decoder
// ═══════════════════════════════════════════════════════════════════════════

mod engine_seek {
    use super::*;

    #[test]
    fn seek_without_decoder_no_crash() {
        let engine = AudioEngine::new().unwrap();
        // Seek when there's no decoder should log a warning but not panic
        engine.seek(30.0).unwrap();
        thread::sleep(Duration::from_millis(200));
        // Position should still be zero (no decoder to seek in)
        assert!((engine.position() - 0.0).abs() < f64::EPSILON);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — rapid command sequence
// ═══════════════════════════════════════════════════════════════════════════

mod engine_stress {
    use super::*;

    #[test]
    fn rapid_commands_no_panic() {
        let engine = AudioEngine::new().unwrap();
        // Send a burst of commands — the engine should not panic or deadlock
        for _ in 0..20 {
            let _ = engine.pause();
            let _ = engine.resume();
            let _ = engine.stop();
            let _ = engine.set_volume(0.5);
            let _ = engine.seek(0.0);
        }
        // Drain events to prevent backpressure
        thread::sleep(Duration::from_millis(500));
        while engine.try_recv_event().is_some() {}
    }

    #[test]
    fn rapid_play_file_errors_no_panic() {
        let engine = AudioEngine::new().unwrap();
        // Rapidly send play commands for nonexistent files
        for i in 0..10 {
            let _ = engine.play_file(&format!("/tmp/__cosmic_stress_test_{}.mp3", i), None);
        }
        // Let the loop churn through all the errors
        thread::sleep(Duration::from_secs(3));
        while engine.try_recv_event().is_some() {}

        // Engine should still be alive and responsive
        assert!(engine.stop().is_ok());
    }

    #[test]
    fn interleaved_play_and_stop_no_panic() {
        let engine = AudioEngine::new().unwrap();
        for i in 0..5 {
            let _ = engine.play_file(&format!("/tmp/__cosmic_interleave_{}.mp3", i), None);
            let _ = engine.stop();
        }
        thread::sleep(Duration::from_secs(2));
        assert_eq!(engine.state(), PlaybackState::Stopped);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — spectrum data accessors
// ═══════════════════════════════════════════════════════════════════════════

mod engine_spectrum {
    use super::*;

    #[test]
    fn spectrum_returns_expected_band_count() {
        let engine = AudioEngine::new().unwrap();
        let spectrum = engine.spectrum();
        // Engine creates SharedSpectrumAnalyzer::with_bands(44100, 12)
        assert_eq!(spectrum.left_bands.len(), 12);
        assert_eq!(spectrum.right_bands.len(), 12);
    }

    #[test]
    fn spectrum_initially_silent() {
        let engine = AudioEngine::new().unwrap();
        let spectrum = engine.spectrum();
        for &v in spectrum
            .left_bands
            .iter()
            .chain(spectrum.right_bands.iter())
        {
            assert!(v.abs() < f32::EPSILON, "expected silence, got {v}");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — event sequences for error paths
// ═══════════════════════════════════════════════════════════════════════════

mod engine_event_sequences {
    use super::*;

    #[test]
    fn play_file_error_event_contains_message() {
        let engine = AudioEngine::new().unwrap();
        engine
            .play_file("/tmp/__mare_player_event_msg_test.mp3", None)
            .unwrap();

        let mut error_msg = String::new();
        let found = wait_for_event(&engine, Duration::from_secs(5), |evt| {
            if let AudioEngineEvent::Error(msg) = evt {
                error_msg = msg.clone();
                true
            } else {
                false
            }
        });

        assert!(found, "expected Error event");
        assert!(!error_msg.is_empty(), "error message should be non-empty");
    }

    #[test]
    fn play_url_error_event_contains_message() {
        let (url, _port) = spawn_http_error(503);
        let engine = AudioEngine::new().unwrap();
        engine.play_url_cached(&url, None, None).unwrap();

        let mut error_msg = String::new();
        let found = wait_for_event(&engine, Duration::from_secs(10), |evt| {
            if let AudioEngineEvent::Error(msg) = evt {
                error_msg = msg.clone();
                true
            } else {
                false
            }
        });

        assert!(found, "expected Error event for HTTP 503");
        assert!(!error_msg.is_empty(), "error message should be non-empty");
    }

    #[test]
    fn stop_after_failed_play_file() {
        let engine = AudioEngine::new().unwrap();
        engine
            .play_file("/tmp/__mare_player_stop_after_fail.mp3", None)
            .unwrap();

        // Wait for the error to be processed
        wait_for_event(&engine, Duration::from_secs(5), |evt| {
            matches!(evt, AudioEngineEvent::Error(_))
        });

        // Now send stop — should still work
        engine.stop().unwrap();
        let found_stopped = wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Stopped))
        });
        assert!(found_stopped, "stop after error should still emit Stopped");
    }

    #[test]
    fn multiple_failed_plays_then_stop() {
        let engine = AudioEngine::new().unwrap();

        // Fire off several failures
        for i in 0..3 {
            engine
                .play_file(&format!("/tmp/__mare_player_multi_fail_{}.mp3", i), None)
                .unwrap();
        }

        // Wait for errors to process
        thread::sleep(Duration::from_secs(3));

        // Drain all pending events
        while engine.try_recv_event().is_some() {}

        // Engine should still be responsive
        engine.stop().unwrap();
        let found_stopped = wait_for_event(&engine, Duration::from_secs(3), |evt| {
            matches!(evt, AudioEngineEvent::StateChanged(PlaybackState::Stopped))
        });
        assert!(found_stopped);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — position calculation
// ═══════════════════════════════════════════════════════════════════════════

mod engine_position {
    use super::*;

    #[test]
    fn position_starts_at_zero() {
        let engine = AudioEngine::new().unwrap();
        assert!((engine.position() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn position_remains_zero_without_playback() {
        let engine = AudioEngine::new().unwrap();
        thread::sleep(Duration::from_millis(100));
        assert!((engine.position() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn position_zero_after_stop() {
        let engine = AudioEngine::new().unwrap();
        engine.stop().unwrap();
        thread::sleep(Duration::from_millis(200));
        assert!((engine.position() - 0.0).abs() < f64::EPSILON);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioEngine — shutdown & drop ordering
// ═══════════════════════════════════════════════════════════════════════════

mod engine_shutdown {
    use super::*;

    #[test]
    fn explicit_shutdown_then_drop() {
        let engine = AudioEngine::new().unwrap();
        engine.shutdown().unwrap();
        // Give the loop time to exit
        thread::sleep(Duration::from_millis(200));
        // Drop should not hang even though the loop already exited
        drop(engine);
    }

    #[test]
    fn commands_after_shutdown_fail_gracefully() {
        let engine = AudioEngine::new().unwrap();
        engine.shutdown().unwrap();
        // Give the loop time to break
        thread::sleep(Duration::from_millis(500));

        // The channel may or may not be closed depending on timing.
        // The important thing is no panic.
        let _ = engine.pause();
        let _ = engine.stop();
        let _ = engine.set_volume(0.5);
    }

    #[test]
    fn double_shutdown_no_panic() {
        let engine = AudioEngine::new().unwrap();
        let _ = engine.shutdown();
        let _ = engine.shutdown();
        thread::sleep(Duration::from_millis(200));
        drop(engine);
    }
}
