// SPDX-License-Identifier: MIT

//! Audio engine for Maré Player.
//!
//! This module provides audio playback using symphonia for decoding, PulseAudio
//! for output (via pipewire-pulse on modern desktops), and rustfft for
//! real-time spectrum analysis.

pub mod dash;
pub mod decoder;
pub mod engine;
pub mod output;
pub mod spectrum;

pub use engine::{AudioEngine, AudioEngineEvent, PlaybackState};
pub use spectrum::SpectrumData;
