// SPDX-License-Identifier: MIT

//! Audio analysis for Maré Player.
//!
//! Playback runs through the GStreamer engine (see [`crate::playback`]); this
//! module only provides real-time spectrum analysis (via rustfft) for the
//! now-playing visualizer, fed by the playback pipeline's PCM tap.

pub mod spectrum;

pub use spectrum::SpectrumData;
