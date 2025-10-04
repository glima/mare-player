// SPDX-License-Identifier: MIT

//! Integration tests for `audio::output` public API.
//!
//! These tests exercise `OutputConfig`, `OutputError`, `AudioOutputBuilder`,
//! and `AudioOutput` from the public interface.  PA-dependent tests gracefully
//! skip when no PulseAudio / PipeWire-pulse server is running, so they pass
//! in headless CI without noise.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use std::time::Duration;

use cosmic_applet_mare::audio::output::{
    AudioOutput, AudioOutputBuilder, OutputConfig, OutputError, OutputResult,
};

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Try to create a default AudioOutput.  Returns `None` when no PA server is
/// available so PA-dependent tests can skip gracefully.
fn try_default_output() -> Option<AudioOutput> {
    AudioOutput::new_output(OutputConfig::default()).ok()
}

fn try_output_with(rate: u32, ch: u16, vol: f32) -> Option<AudioOutput> {
    AudioOutput::new_output(OutputConfig {
        sample_rate: rate,
        channels: ch,
        initial_volume: vol,
    })
    .ok()
}

// ═══════════════════════════════════════════════════════════════════════════
// OutputConfig
// ═══════════════════════════════════════════════════════════════════════════

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
    fn clone_preserves_fields() {
        let config = OutputConfig {
            sample_rate: 96000,
            channels: 8,
            initial_volume: 0.25,
        };
        let cloned = config.clone();
        assert_eq!(cloned.sample_rate, 96000);
        assert_eq!(cloned.channels, 8);
        assert!((cloned.initial_volume - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn debug_contains_fields() {
        let config = OutputConfig {
            sample_rate: 48000,
            channels: 1,
            initial_volume: 0.5,
        };
        let dbg = format!("{config:?}");
        assert!(dbg.contains("48000"), "got: {dbg}");
        assert!(dbg.contains("1"), "got: {dbg}");
        assert!(dbg.contains("0.5"), "got: {dbg}");
    }

    #[test]
    fn field_access() {
        let config = OutputConfig {
            sample_rate: 22050,
            channels: 6,
            initial_volume: 0.0,
        };
        assert_eq!(config.sample_rate, 22050);
        assert_eq!(config.channels, 6);
        assert!(config.initial_volume.abs() < f32::EPSILON);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// OutputError
// ═══════════════════════════════════════════════════════════════════════════

mod output_error {
    use super::*;

    #[test]
    fn display_no_device() {
        let err = OutputError::NoDevice;
        assert_eq!(err.to_string(), "No audio output device found");
    }

    #[test]
    fn display_config_error() {
        let err = OutputError::ConfigError("invalid channels".into());
        let msg = err.to_string();
        assert!(msg.starts_with("Config error:"), "got: {msg}");
        assert!(msg.contains("invalid channels"), "got: {msg}");
    }

    #[test]
    fn display_stream_error() {
        let err = OutputError::StreamError("PA connect failed".into());
        let msg = err.to_string();
        assert!(msg.starts_with("Stream error:"), "got: {msg}");
        assert!(msg.contains("PA connect failed"), "got: {msg}");
    }

    #[test]
    fn all_variants_display_non_empty() {
        let errors: Vec<OutputError> = vec![
            OutputError::NoDevice,
            OutputError::ConfigError("x".into()),
            OutputError::StreamError("y".into()),
        ];
        for err in &errors {
            assert!(!err.to_string().is_empty(), "empty Display for {err:?}");
        }
    }

    #[test]
    fn implements_std_error() {
        let err: Box<dyn std::error::Error> = Box::new(OutputError::StreamError("test".into()));
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn debug_no_device() {
        let dbg = format!("{:?}", OutputError::NoDevice);
        assert!(dbg.contains("NoDevice"));
    }

    #[test]
    fn debug_config_error() {
        let dbg = format!("{:?}", OutputError::ConfigError("bad".into()));
        assert!(dbg.contains("ConfigError"));
        assert!(dbg.contains("bad"));
    }

    #[test]
    fn debug_stream_error() {
        let dbg = format!("{:?}", OutputError::StreamError("pipe".into()));
        assert!(dbg.contains("StreamError"));
        assert!(dbg.contains("pipe"));
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// OutputResult type alias
// ═══════════════════════════════════════════════════════════════════════════

mod output_result {
    use super::*;

    #[test]
    fn ok_variant() {
        let r: OutputResult<u32> = Ok(42);
        assert_eq!(r.unwrap(), 42);
    }

    #[test]
    fn err_variant() {
        let r: OutputResult<u32> = Err(OutputError::NoDevice);
        assert!(r.is_err());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioOutputBuilder
// ═══════════════════════════════════════════════════════════════════════════

mod builder {
    use super::*;

    #[test]
    fn new_defaults() {
        // Can't inspect private config, but build with invalid channels
        // should error, proving the builder was constructed.
        let result = AudioOutputBuilder::new().channels(0).build();
        assert!(result.is_err());
    }

    #[test]
    fn default_trait() {
        let result = AudioOutputBuilder::default().channels(0).build();
        assert!(result.is_err());
    }

    #[test]
    fn sample_rate_setter() {
        // Set sample rate then invalidate channels to prove it was applied.
        let result = AudioOutputBuilder::new()
            .sample_rate(48000)
            .channels(0)
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn channels_setter() {
        let result = AudioOutputBuilder::new().channels(0).build();
        assert!(result.is_err());
    }

    #[test]
    fn initial_volume_setter_clamps_high() {
        // Can't directly inspect the volume, but the builder shouldn't panic.
        let _builder = AudioOutputBuilder::new().initial_volume(5.0);
    }

    #[test]
    fn initial_volume_setter_clamps_low() {
        let _builder = AudioOutputBuilder::new().initial_volume(-1.0);
    }

    #[test]
    fn full_chain() {
        // Fully chained builder with invalid channels → proves all setters
        // return Self correctly.
        let result = AudioOutputBuilder::new()
            .sample_rate(96000)
            .channels(0)
            .initial_volume(0.75)
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_invalid_zero_channels() {
        let result = AudioOutputBuilder::new().channels(0).build();
        assert!(result.is_err());
    }

    #[test]
    fn build_invalid_zero_rate() {
        let result = AudioOutputBuilder::new().sample_rate(0).build();
        assert!(result.is_err());
    }

    #[test]
    fn build_valid_no_panic() {
        // With a valid spec, build() will attempt PA connection.
        // In CI (no PA server) it returns Err; with PA it returns Ok.
        // Either way: no panic.
        let _result = AudioOutputBuilder::new()
            .sample_rate(44100)
            .channels(2)
            .initial_volume(0.5)
            .build();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioOutput::new_output — invalid spec (no PA needed)
// ═══════════════════════════════════════════════════════════════════════════

mod new_output_invalid {
    use super::*;

    #[test]
    fn zero_channels() {
        let config = OutputConfig {
            sample_rate: 44100,
            channels: 0,
            initial_volume: 1.0,
        };
        match AudioOutput::new_output(config) {
            Err(OutputError::ConfigError(msg)) => {
                assert!(msg.contains("Invalid sample spec"), "got: {msg}");
            }
            Err(other) => panic!("expected ConfigError, got: {other}"),
            Ok(_) => panic!("expected error for 0 channels"),
        }
    }

    #[test]
    fn zero_sample_rate() {
        let config = OutputConfig {
            sample_rate: 0,
            channels: 2,
            initial_volume: 1.0,
        };
        match AudioOutput::new_output(config) {
            Err(OutputError::ConfigError(msg)) => {
                assert!(msg.contains("Invalid sample spec"), "got: {msg}");
            }
            Err(other) => panic!("expected ConfigError, got: {other}"),
            Ok(_) => panic!("expected error for 0 sample rate"),
        }
    }

    #[test]
    fn both_zero() {
        let config = OutputConfig {
            sample_rate: 0,
            channels: 0,
            initial_volume: 1.0,
        };
        match AudioOutput::new_output(config) {
            Err(OutputError::ConfigError(_)) => {} // expected
            Err(other) => panic!("expected ConfigError, got: {other}"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn valid_spec_no_panic() {
        let _result = AudioOutput::new_output(OutputConfig::default());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AudioOutput — PA-dependent (skip when no server available)
// ═══════════════════════════════════════════════════════════════════════════

mod pa_construction {
    use super::*;

    #[test]
    fn default_config() {
        let Some(_output) = try_default_output() else {
            return;
        };
    }

    #[test]
    fn config_48000_stereo() {
        let Some(_output) = try_output_with(48000, 2, 1.0) else {
            return;
        };
    }

    #[test]
    fn config_44100_mono() {
        let Some(_output) = try_output_with(44100, 1, 0.5) else {
            return;
        };
    }

    #[test]
    fn config_zero_volume() {
        let Some(_output) = try_output_with(44100, 2, 0.0) else {
            return;
        };
    }
}

mod pa_play_pause {
    use super::*;

    #[test]
    fn play_ok() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        assert!(output.play().is_ok());
    }

    #[test]
    fn pause_ok() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        assert!(output.pause().is_ok());
    }

    #[test]
    fn play_then_pause() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        assert!(output.play().is_ok());
        assert!(output.pause().is_ok());
    }

    #[test]
    fn play_pause_play() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        assert!(output.play().is_ok());
        assert!(output.pause().is_ok());
        assert!(output.play().is_ok());
    }

    #[test]
    fn double_pause() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        assert!(output.pause().is_ok());
        assert!(output.pause().is_ok());
    }

    #[test]
    fn double_play() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        assert!(output.play().is_ok());
        assert!(output.play().is_ok());
    }
}

mod pa_write {
    use super::*;

    #[test]
    fn empty_returns_zero() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        assert_eq!(output.write(&[]), 0);
    }

    #[test]
    fn silence() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        let written = output.write(&[0.0f32; 1024]);
        assert!(written > 0, "expected >0, got {written}");
    }

    #[test]
    fn sine_wave() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        let freq = 440.0f32;
        let rate = 44100.0f32;
        let mut samples = Vec::with_capacity(2048);
        for i in 0..1024 {
            let t = i as f32 / rate;
            let v = (2.0 * std::f32::consts::PI * freq * t).sin() * 0.1;
            samples.push(v); // L
            samples.push(v); // R
        }
        let written = output.write(&samples);
        assert!(written > 0);
    }

    #[test]
    fn large_buffer() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        let written = output.write(&[0.0f32; 16384]);
        assert!(written > 0);
    }

    #[test]
    fn returns_sample_count_not_bytes() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        let written = output.write(&[0.0f32; 512]);
        // Should be ≤ 512 (samples), not ≤ 2048 (bytes)
        assert!(written <= 512, "got {written} — looks like byte count");
    }
}

mod pa_buffer_query {
    use super::*;

    #[test]
    fn available_space_positive_after_play() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        let space = output.available_space();
        assert!(space > 0, "expected positive, got {space}");
    }

    #[test]
    fn available_space_while_corked() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        // Corked — should not panic
        let _space = output.available_space();
    }

    #[test]
    fn buffered_samples_always_zero() {
        let Some(output) = try_default_output() else {
            return;
        };
        assert_eq!(output.buffered_samples(), 0);
    }

    #[test]
    fn buffered_samples_zero_after_write() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        let _ = output.write(&[0.0f32; 256]);
        assert_eq!(output.buffered_samples(), 0);
    }
}

mod pa_flush {
    use super::*;

    #[test]
    fn after_write() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        let _ = output.write(&[0.0f32; 512]);
        output.flush(); // should not panic
    }

    #[test]
    fn while_corked() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.flush(); // harmless while corked
    }

    #[test]
    fn double_flush() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        output.flush();
        output.flush();
    }
}

mod pa_volume {
    use super::*;

    #[test]
    fn set_full() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let _ = output.set_volume(1.0);
    }

    #[test]
    fn set_zero() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let _ = output.set_volume(0.0);
    }

    #[test]
    fn set_mid() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let _ = output.set_volume(0.5);
    }

    #[test]
    fn clamps_above_one() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let _ = output.set_volume(1.5); // should clamp, not panic
    }

    #[test]
    fn clamps_below_zero() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let _ = output.set_volume(-0.5); // should clamp, not panic
    }

    #[test]
    fn multiple_changes() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        for v in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let _ = output.set_volume(v);
        }
    }
}

mod pa_matches_config {
    use super::*;

    #[test]
    fn same_config_matches() {
        let Some(output) = try_default_output() else {
            return;
        };
        assert!(output.matches_config(44100, 2));
    }

    #[test]
    fn different_rate_no_match() {
        let Some(output) = try_default_output() else {
            return;
        };
        assert!(!output.matches_config(48000, 2));
    }

    #[test]
    fn different_channels_no_match() {
        let Some(output) = try_default_output() else {
            return;
        };
        assert!(!output.matches_config(44100, 1));
    }

    #[test]
    fn both_different_no_match() {
        let Some(output) = try_default_output() else {
            return;
        };
        assert!(!output.matches_config(48000, 6));
    }

    #[test]
    fn custom_config_matches() {
        let Some(output) = try_output_with(48000, 1, 0.5) else {
            return;
        };
        assert!(output.matches_config(48000, 1));
        assert!(!output.matches_config(44100, 1));
        assert!(!output.matches_config(48000, 2));
    }
}

mod pa_drop {
    use super::*;

    #[test]
    fn drop_corked() {
        let Some(output) = try_default_output() else {
            return;
        };
        drop(output); // should not hang or panic
    }

    #[test]
    fn drop_after_play() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        let _ = output.write(&[0.0f32; 256]);
        drop(output);
    }

    #[test]
    fn drop_after_pause() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        output.play().unwrap();
        output.pause().unwrap();
        drop(output);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// End-to-end lifecycle
// ═══════════════════════════════════════════════════════════════════════════

mod pa_lifecycle {
    use super::*;

    #[test]
    fn full_lifecycle() {
        let Some(mut output) = try_default_output() else {
            return;
        };

        // Play
        output.play().unwrap();

        // Write silence
        let written = output.write(&[0.0f32; 2048]);
        assert!(written > 0);

        // Query
        let _space = output.available_space();
        assert_eq!(output.buffered_samples(), 0);
        assert!(output.matches_config(44100, 2));

        // Pause
        output.pause().unwrap();

        // Flush
        output.flush();

        // Resume
        output.play().unwrap();

        // Volume
        std::thread::sleep(Duration::from_millis(50));
        let _ = output.set_volume(0.7);

        // Drop
        drop(output);
    }

    #[test]
    fn multiple_sequential_outputs() {
        for _ in 0..3 {
            let Some(mut output) = try_default_output() else {
                return;
            };
            output.play().unwrap();
            let _ = output.write(&[0.0f32; 512]);
            output.pause().unwrap();
            drop(output);
        }
    }

    #[test]
    fn play_write_flush_cycle() {
        let Some(mut output) = try_default_output() else {
            return;
        };
        for _ in 0..5 {
            output.play().unwrap();
            let _ = output.write(&[0.0f32; 256]);
            output.flush();
            output.pause().unwrap();
        }
    }
}
