// SPDX-License-Identifier: MIT

//! Audio output using the full PulseAudio async API.
//!
//! This module provides audio output with first-class PipeWire integration
//! using the PulseAudio async API via `libpulse-binding`.  Unlike the simple
//! API, this gives us:
//!
//! - **Stream volume control** visible in PipeWire / PulseAudio mixers
//!   (pavucontrol, wiremix, helvum, etc.)
//! - **Cork / uncork** for proper pause / resume without tearing down streams
//! - **Better metadata** via property lists
//!
//! On modern Linux desktops running PipeWire, the `pipewire-pulse`
//! compatibility layer intercepts these calls.

use libpulse_binding::context::introspect::Introspector;
use libpulse_binding::context::{Context, FlagSet as ContextFlagSet, State as ContextState};

use libpulse_binding::mainloop::threaded::Mainloop;
use libpulse_binding::proplist::Proplist;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::{FlagSet as StreamFlagSet, SeekMode, State as StreamState, Stream};
use libpulse_binding::volume::{ChannelVolumes, Volume};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Application name reported to PulseAudio / PipeWire.
const PA_APP_NAME: &str = "Maré Player";

/// Stream description shown in mixer UIs (pavucontrol, wiremix, etc.).
const PA_STREAM_NAME: &str = "Playback Stream";

/// How long to wait for the PA context or stream to become ready.
const PA_READY_TIMEOUT: Duration = Duration::from_secs(5);

/// Polling interval when waiting for PA state transitions during setup.
const STATE_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Polling interval when waiting for write space in the PA buffer.
const WRITE_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Maximum time to block in [`AudioOutput::write`] waiting for buffer space.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Audio output configuration.
#[derive(Debug, Clone)]
pub struct OutputConfig {
    /// Desired sample rate.
    pub sample_rate: u32,
    /// Number of channels (typically 2 for stereo).
    pub channels: u16,
    /// Initial volume level (0.0 – 1.0).  Applied atomically at stream
    /// creation so the very first samples come out at the right level.
    pub initial_volume: f32,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            sample_rate: 44100,
            channels: 2,
            initial_volume: 1.0,
        }
    }
}

/// Result type for output operations.
pub type OutputResult<T> = Result<T, OutputError>;

/// Errors that can occur during audio output.
#[derive(Debug)]
pub enum OutputError {
    /// No audio output device found.
    NoDevice,
    /// Failed to get device config.
    ConfigError(String),
    /// Failed to build audio stream.
    StreamError(String),
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDevice => write!(f, "No audio output device found"),
            Self::ConfigError(msg) => write!(f, "Config error: {msg}"),
            Self::StreamError(msg) => write!(f, "Stream error: {msg}"),
        }
    }
}

impl std::error::Error for OutputError {}

// ---------------------------------------------------------------------------
// Mainloop lock guard
// ---------------------------------------------------------------------------

/// RAII guard for the PulseAudio threaded-mainloop lock.
///
/// Guarantees that the lock is released even on early returns or panics.
/// The PA threaded mainloop's `lock()` / `unlock()` require `&mut self`,
/// so this guard stores a mutable reference.
struct MainloopGuard<'a> {
    mainloop: &'a mut Mainloop,
    locked: bool,
}

impl<'a> MainloopGuard<'a> {
    /// Acquire the mainloop lock.
    fn lock(mainloop: &'a mut Mainloop) -> Self {
        mainloop.lock();
        Self {
            mainloop,
            locked: true,
        }
    }

    /// Temporarily release the lock (e.g. before sleeping in a poll loop).
    fn unlock(&mut self) {
        if self.locked {
            self.mainloop.unlock();
            self.locked = false;
        }
    }

    /// Re-acquire the lock after a temporary release.
    fn relock(&mut self) {
        if !self.locked {
            self.mainloop.lock();
            self.locked = true;
        }
    }
}

impl Drop for MainloopGuard<'_> {
    fn drop(&mut self) {
        self.unlock();
    }
}

// ---------------------------------------------------------------------------
// AudioOutput
// ---------------------------------------------------------------------------

/// Audio output stream backed by the full PulseAudio async API.
///
/// The threaded mainloop runs PA's event loop on a dedicated background
/// thread.  All PA API calls are serialised through its lock.  Writes are
/// blocking (poll for writable space) so the engine's playback thread gets
/// natural back-pressure.
///
/// # Drop order
///
/// Fields are dropped in declaration order.  `stream` must be dropped before
/// `context`, and `context` before `mainloop`, so they are declared in that
/// order.  The custom [`Drop`] impl stops the mainloop thread first.
pub struct AudioOutput {
    /// The playback stream – dropped first.
    stream: Stream,
    /// The PA context – dropped second.
    context: Context,
    /// The threaded mainloop – dropped last.
    mainloop: Mainloop,
    /// Configuration this output was created with.
    config: OutputConfig,
}

impl AudioOutput {
    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// Create a new audio output connected to the default PulseAudio sink.
    ///
    /// The stream is created in the **corked** (paused) state.  Call
    /// [`play`](Self::play) to start writing / hearing audio.
    pub fn new_output(desired_config: OutputConfig) -> OutputResult<Self> {
        info!(
            "Initializing PulseAudio async output: {}Hz, {} ch, vol {:.0}%",
            desired_config.sample_rate,
            desired_config.channels,
            desired_config.initial_volume * 100.0,
        );

        // -- sample spec --------------------------------------------------
        let spec = Spec {
            format: Format::F32le,
            channels: desired_config.channels as u8,
            rate: desired_config.sample_rate,
        };
        if !spec.is_valid() {
            return Err(OutputError::ConfigError(format!(
                "Invalid sample spec: {}Hz, {} ch",
                desired_config.sample_rate, desired_config.channels,
            )));
        }

        // -- initial volume -----------------------------------------------
        let initial_volume =
            Self::make_channel_volumes(desired_config.channels, desired_config.initial_volume);

        // -- proplist ------------------------------------------------------
        let mut proplist = Proplist::new()
            .ok_or_else(|| OutputError::StreamError("Failed to create PA proplist".into()))?;
        // Best-effort; ignore errors on individual keys.
        let _ = proplist.set_str(
            libpulse_binding::proplist::properties::APPLICATION_NAME,
            PA_APP_NAME,
        );
        let _ = proplist.set_str(libpulse_binding::proplist::properties::MEDIA_ROLE, "music");

        // -- mainloop -----------------------------------------------------
        let mut mainloop = Mainloop::new().ok_or_else(|| {
            OutputError::StreamError("Failed to create PA threaded mainloop".into())
        })?;

        // -- context ------------------------------------------------------
        let mut context = Context::new_with_proplist(&mainloop, "Maré Player Playback", &proplist)
            .ok_or_else(|| OutputError::StreamError("Failed to create PA context".into()))?;

        // Initiate the connection (async – the mainloop will drive it).
        context
            .connect(None, ContextFlagSet::NOFLAGS, None)
            .map_err(|e| OutputError::StreamError(format!("PA context connect failed: {e}")))?;

        // -- start mainloop -----------------------------------------------
        mainloop.lock();
        if let Err(e) = mainloop.start() {
            mainloop.unlock();
            return Err(OutputError::StreamError(format!(
                "Failed to start PA mainloop: {e}"
            )));
        }

        // -- wait for context ready (polling) -----------------------------
        let deadline = Instant::now() + PA_READY_TIMEOUT;
        loop {
            let ctx_state = context.get_state();
            match ctx_state {
                ContextState::Ready => break,
                ContextState::Failed | ContextState::Terminated => {
                    mainloop.unlock();
                    mainloop.stop();
                    return Err(OutputError::NoDevice);
                }
                _ => {
                    mainloop.unlock();
                    if Instant::now() > deadline {
                        mainloop.stop();
                        return Err(OutputError::StreamError(
                            "Timed out waiting for PA context ready".into(),
                        ));
                    }
                    std::thread::sleep(STATE_POLL_INTERVAL);
                    mainloop.lock();
                }
            }
        }

        // -- create stream ------------------------------------------------
        let mut stream =
            Stream::new(&mut context, PA_STREAM_NAME, &spec, None).ok_or_else(|| {
                mainloop.unlock();
                mainloop.stop();
                OutputError::StreamError("Failed to create PA stream".into())
            })?;

        // Connect for playback – start corked so no audio leaks before the
        // engine calls `play()`.  Pass the initial volume so that the very
        // first uncork already has the right level.
        if let Err(e) = stream.connect_playback(
            None,                        // device: default sink
            None,                        // buffer attributes: server defaults
            StreamFlagSet::START_CORKED, // start paused
            Some(&initial_volume),       // initial volume
            None,                        // no sync stream
        ) {
            mainloop.unlock();
            mainloop.stop();
            return Err(OutputError::StreamError(format!(
                "PA stream connect_playback failed: {e}"
            )));
        }

        // -- wait for stream ready (polling) ------------------------------
        let deadline = Instant::now() + PA_READY_TIMEOUT;
        loop {
            let st_state = stream.get_state();
            match st_state {
                StreamState::Ready => break,
                StreamState::Failed | StreamState::Terminated => {
                    mainloop.unlock();
                    mainloop.stop();
                    return Err(OutputError::StreamError(
                        "PA stream entered failed/terminated state".into(),
                    ));
                }
                _ => {
                    mainloop.unlock();
                    if Instant::now() > deadline {
                        mainloop.stop();
                        return Err(OutputError::StreamError(
                            "Timed out waiting for PA stream ready".into(),
                        ));
                    }
                    std::thread::sleep(STATE_POLL_INTERVAL);
                    mainloop.lock();
                }
            }
        }

        mainloop.unlock();

        info!(
            "PulseAudio async output connected: {}Hz, {} ch, F32LE",
            desired_config.sample_rate, desired_config.channels,
        );

        Ok(Self {
            stream,
            context,
            mainloop,
            config: desired_config,
        })
    }

    // ------------------------------------------------------------------
    // Playback control
    // ------------------------------------------------------------------

    /// Start (or resume) playback by uncorking the stream.
    pub fn play(&mut self) -> OutputResult<()> {
        let _guard = MainloopGuard::lock(&mut self.mainloop);
        let _op = self.stream.uncork(None);
        debug!("Audio playback started (uncorked)");
        Ok(())
    }

    /// Pause playback by corking the stream and flushing the server-side
    /// buffer so audio stops immediately.
    pub fn pause(&mut self) -> OutputResult<()> {
        let _guard = MainloopGuard::lock(&mut self.mainloop);
        let _op = self.stream.cork(None);
        let _op2 = self.stream.flush(None);
        debug!("Audio playback paused (corked + flushed)");
        Ok(())
    }

    /// Write interleaved F32LE samples to the output.
    ///
    /// This is a **blocking** call – it polls for writable space in the
    /// PulseAudio server-side buffer, providing natural back-pressure for
    /// the engine's decode loop.
    ///
    /// Returns the number of *samples* (not bytes) actually written.
    pub fn write(&mut self, samples: &[f32]) -> usize {
        if samples.is_empty() {
            return 0;
        }

        let byte_len = std::mem::size_of_val(samples);

        // SAFETY: reinterpreting &[f32] as &[u8] — both are Copy, no
        // padding concerns, and PA expects FLOAT32LE which is the native
        // in-memory representation on little-endian (all x86 / ARM Linux).
        let all_bytes: &[u8] =
            unsafe { std::slice::from_raw_parts(samples.as_ptr() as *const u8, byte_len) };

        let mut guard = MainloopGuard::lock(&mut self.mainloop);
        let mut written_bytes: usize = 0;
        let deadline = Instant::now() + WRITE_TIMEOUT;

        while written_bytes < byte_len {
            let writable = self.stream.writable_size().unwrap_or(0);

            if writable > 0 {
                let remaining = byte_len - written_bytes;
                let chunk_len = remaining.min(writable);

                let chunk = match all_bytes.get(written_bytes..written_bytes + chunk_len) {
                    Some(c) => c,
                    None => {
                        warn!("write: slice bounds exceeded, stopping write");
                        break;
                    }
                };

                // `write_copy` passes `None` for the free callback so PA
                // copies the data internally — we keep ownership.
                if let Err(e) = self.stream.write_copy(chunk, 0, SeekMode::Relative) {
                    error!("PA stream write error: {e}");
                    break;
                }

                written_bytes += chunk_len;
            } else {
                // No space yet — release the lock, sleep briefly, re-lock.
                if Instant::now() > deadline {
                    warn!("write: timed out waiting for buffer space");
                    break;
                }
                guard.unlock();
                std::thread::sleep(WRITE_POLL_INTERVAL);
                guard.relock();
            }
        }

        // guard dropped here → unlocks
        drop(guard);

        written_bytes / std::mem::size_of::<f32>()
    }

    /// Return the number of *samples* (not bytes) the server can currently
    /// accept.  The engine uses this to decide how much to decode per
    /// iteration.
    pub fn available_space(&mut self) -> usize {
        let _guard = MainloopGuard::lock(&mut self.mainloop);
        let bytes = self.stream.writable_size().unwrap_or(0);
        bytes / std::mem::size_of::<f32>()
    }

    /// Return the number of samples currently buffered server-side.
    ///
    /// With the async API we don't have a cheap way to query this.  The
    /// engine only uses it to wait for drain at track end; the [`Drop`]
    /// impl handles draining, so we always report zero.
    pub fn buffered_samples(&self) -> usize {
        0
    }

    /// Flush the server-side playback buffer (used after seek to discard
    /// stale samples).
    pub fn flush(&mut self) {
        let _guard = MainloopGuard::lock(&mut self.mainloop);
        let _op = self.stream.flush(None);
        debug!("PA output buffer flushed");
    }

    // ------------------------------------------------------------------
    // Volume
    // ------------------------------------------------------------------

    /// Set the stream volume to `level` (0.0 – 1.0).
    ///
    /// This sets the **PipeWire / PulseAudio sink-input volume** so it is
    /// visible in all system mixer UIs (pavucontrol, wiremix, etc.) and
    /// persisted by WirePlumber's volume-restore policy.
    pub fn set_volume(&mut self, level: f32) -> OutputResult<()> {
        let clamped = level.clamp(0.0, 1.0);

        let _guard = MainloopGuard::lock(&mut self.mainloop);

        let index = match self.stream.get_index() {
            Some(idx) => idx,
            None => {
                warn!("set_volume: stream has no sink-input index yet");
                return Err(OutputError::StreamError(
                    "Stream not yet assigned a sink-input index".into(),
                ));
            }
        };

        let cv = Self::make_channel_volumes(self.config.channels, clamped);
        let mut introspect: Introspector = self.context.introspect();
        let _op = introspect.set_sink_input_volume(index, &cv, None);

        debug!(
            "PipeWire volume set to {:.0}% (sink-input #{})",
            clamped * 100.0,
            index,
        );
        Ok(())
    }

    // ------------------------------------------------------------------
    // Query
    // ------------------------------------------------------------------

    /// Check whether this output can be reused for a new track with the
    /// given sample rate and channel count.
    ///
    /// Reusing avoids tearing down the PipeWire node, which can trigger
    /// WirePlumber volume-restore policies on Bluetooth sinks.
    pub fn matches_config(&self, sample_rate: u32, channels: u16) -> bool {
        self.config.sample_rate == sample_rate && self.config.channels == channels
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Build a [`ChannelVolumes`] with all channels set to the given linear
    /// level (0.0 – 1.0).
    fn make_channel_volumes(channels: u16, level: f32) -> ChannelVolumes {
        let clamped = level.clamp(0.0, 1.0);
        let pa_vol = Volume((Volume::NORMAL.0 as f64 * clamped as f64) as u32);
        let mut cv = ChannelVolumes::default();
        cv.set_len(channels as u8);
        cv.set(channels as u8, pa_vol);
        cv
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        debug!("Dropping AudioOutput (PulseAudio async)");

        // Lock the mainloop so we can safely interact with the stream /
        // context before tearing down the event loop thread.
        self.mainloop.lock();

        // Best-effort drain so the very tail of a track is not clipped.
        let _op = self.stream.drain(None);

        // Disconnect stream and context while still locked.
        let _ = self.stream.disconnect();
        self.context.disconnect();

        self.mainloop.unlock();

        // Stop the background event-loop thread.  After this returns, no
        // more callbacks will fire and it is safe for Rust to drop the
        // Stream → Context → Mainloop in declaration order.
        self.mainloop.stop();
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for creating [`AudioOutput`] with various options.
pub struct AudioOutputBuilder {
    config: OutputConfig,
}

impl AudioOutputBuilder {
    /// Create a new builder with default config.
    pub fn new() -> Self {
        Self {
            config: OutputConfig::default(),
        }
    }

    /// Set the desired sample rate.
    pub fn sample_rate(mut self, rate: u32) -> Self {
        self.config.sample_rate = rate;
        self
    }

    /// Set the number of channels.
    pub fn channels(mut self, channels: u16) -> Self {
        self.config.channels = channels;
        self
    }

    /// Set the initial stream volume (0.0 – 1.0).
    ///
    /// This is applied atomically at stream creation via
    /// `pa_stream_connect_playback` so the first samples come out at the
    /// correct level — no 100→X% glitch.
    pub fn initial_volume(mut self, volume: f32) -> Self {
        self.config.initial_volume = volume.clamp(0.0, 1.0);
        self
    }

    /// Build the audio output.
    pub fn build(self) -> OutputResult<AudioOutput> {
        AudioOutput::new_output(self.config)
    }
}

impl Default for AudioOutputBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Helper: try to create a default AudioOutput ────────────────
    //
    // Returns `Some(output)` when PulseAudio / PipeWire-pulse is running,
    // `None` otherwise.  PA-dependent tests call this and `return` early
    // when it yields `None`, so they pass in headless CI without noise.

    fn try_create_output() -> Option<AudioOutput> {
        AudioOutput::new_output(OutputConfig::default()).ok()
    }

    fn try_create_output_with(rate: u32, ch: u16, vol: f32) -> Option<AudioOutput> {
        AudioOutput::new_output(OutputConfig {
            sample_rate: rate,
            channels: ch,
            initial_volume: vol,
        })
        .ok()
    }

    // ═══════════════════════════════════════════════════════════════════
    // OutputConfig
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn config_default() {
        let config = OutputConfig::default();
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.channels, 2);
        assert!((config.initial_volume - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn config_clone() {
        let config = OutputConfig {
            sample_rate: 48000,
            channels: 1,
            initial_volume: 0.5,
        };
        let cloned = config.clone();
        assert_eq!(cloned.sample_rate, 48000);
        assert_eq!(cloned.channels, 1);
        assert!((cloned.initial_volume - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn config_debug() {
        let config = OutputConfig::default();
        let dbg = format!("{config:?}");
        assert!(dbg.contains("44100"));
        assert!(dbg.contains("2"));
    }

    #[test]
    fn config_custom_values() {
        let config = OutputConfig {
            sample_rate: 96000,
            channels: 6,
            initial_volume: 0.0,
        };
        assert_eq!(config.sample_rate, 96000);
        assert_eq!(config.channels, 6);
        assert!(config.initial_volume.abs() < f32::EPSILON);
    }

    // ═══════════════════════════════════════════════════════════════════
    // OutputError — Display
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn error_display_no_device() {
        let err = OutputError::NoDevice;
        assert_eq!(err.to_string(), "No audio output device found");
    }

    #[test]
    fn error_display_config_error() {
        let err = OutputError::ConfigError("bad sample rate".into());
        let msg = err.to_string();
        assert!(msg.starts_with("Config error:"), "got: {msg}");
        assert!(msg.contains("bad sample rate"), "got: {msg}");
    }

    #[test]
    fn error_display_stream_error() {
        let err = OutputError::StreamError("connection refused".into());
        let msg = err.to_string();
        assert!(msg.starts_with("Stream error:"), "got: {msg}");
        assert!(msg.contains("connection refused"), "got: {msg}");
    }

    #[test]
    fn error_all_variants_non_empty() {
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
    fn error_implements_std_error() {
        let err: Box<dyn std::error::Error> = Box::new(OutputError::NoDevice);
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn error_debug() {
        let err = OutputError::ConfigError("test".into());
        let dbg = format!("{err:?}");
        assert!(dbg.contains("ConfigError"));
        assert!(dbg.contains("test"));
    }

    #[test]
    fn error_debug_no_device() {
        let dbg = format!("{:?}", OutputError::NoDevice);
        assert!(dbg.contains("NoDevice"));
    }

    #[test]
    fn error_debug_stream_error() {
        let dbg = format!("{:?}", OutputError::StreamError("broken".into()));
        assert!(dbg.contains("StreamError"));
        assert!(dbg.contains("broken"));
    }

    // ═══════════════════════════════════════════════════════════════════
    // make_channel_volumes
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn channel_volumes_stereo_full() {
        let cv = AudioOutput::make_channel_volumes(2, 1.0);
        for i in 0..2u8 {
            assert_eq!(cv.get()[i as usize], Volume::NORMAL);
        }
    }

    #[test]
    fn channel_volumes_stereo_zero() {
        let cv = AudioOutput::make_channel_volumes(2, 0.0);
        for i in 0..2u8 {
            assert_eq!(cv.get()[i as usize], Volume(0));
        }
    }

    #[test]
    fn channel_volumes_clamp_above_one() {
        let cv_over = AudioOutput::make_channel_volumes(2, 1.5);
        let cv_norm = AudioOutput::make_channel_volumes(2, 1.0);
        for i in 0..2u8 {
            assert_eq!(cv_over.get()[i as usize], cv_norm.get()[i as usize]);
        }
    }

    #[test]
    fn channel_volumes_clamp_negative() {
        let cv_neg = AudioOutput::make_channel_volumes(2, -0.5);
        let cv_zero = AudioOutput::make_channel_volumes(2, 0.0);
        for i in 0..2u8 {
            assert_eq!(cv_neg.get()[i as usize], cv_zero.get()[i as usize]);
        }
    }

    #[test]
    fn channel_volumes_mono() {
        let cv = AudioOutput::make_channel_volumes(1, 0.5);
        let expected_vol = Volume((Volume::NORMAL.0 as f64 * 0.5) as u32);
        assert_eq!(cv.get()[0], expected_vol);
    }

    #[test]
    fn channel_volumes_surround_51() {
        let cv = AudioOutput::make_channel_volumes(6, 0.75);
        let expected_vol = Volume((Volume::NORMAL.0 as f64 * 0.75) as u32);
        for i in 0..6u8 {
            assert_eq!(cv.get()[i as usize], expected_vol);
        }
    }

    #[test]
    fn channel_volumes_half() {
        let cv = AudioOutput::make_channel_volumes(2, 0.5);
        let expected_vol = Volume((Volume::NORMAL.0 as f64 * 0.5) as u32);
        for i in 0..2u8 {
            assert_eq!(cv.get()[i as usize], expected_vol);
        }
    }

    #[test]
    fn channel_volumes_quarter() {
        let cv = AudioOutput::make_channel_volumes(2, 0.25);
        let expected_vol = Volume((Volume::NORMAL.0 as f64 * 0.25) as u32);
        for i in 0..2u8 {
            assert_eq!(cv.get()[i as usize], expected_vol);
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // AudioOutputBuilder
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn builder_defaults() {
        let builder = AudioOutputBuilder::new();
        assert_eq!(builder.config.sample_rate, 44100);
        assert_eq!(builder.config.channels, 2);
        assert!((builder.config.initial_volume - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn builder_default_trait() {
        let builder = AudioOutputBuilder::default();
        assert_eq!(builder.config.sample_rate, 44100);
        assert_eq!(builder.config.channels, 2);
        assert!((builder.config.initial_volume - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn builder_chain() {
        let builder = AudioOutputBuilder::new()
            .sample_rate(48000)
            .channels(1)
            .initial_volume(0.75);
        assert_eq!(builder.config.sample_rate, 48000);
        assert_eq!(builder.config.channels, 1);
        assert!((builder.config.initial_volume - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn builder_sample_rate() {
        let b = AudioOutputBuilder::new().sample_rate(96000);
        assert_eq!(b.config.sample_rate, 96000);
        // Other fields unchanged
        assert_eq!(b.config.channels, 2);
    }

    #[test]
    fn builder_channels() {
        let b = AudioOutputBuilder::new().channels(6);
        assert_eq!(b.config.channels, 6);
        // Other fields unchanged
        assert_eq!(b.config.sample_rate, 44100);
    }

    #[test]
    fn builder_initial_volume() {
        let b = AudioOutputBuilder::new().initial_volume(0.33);
        assert!((b.config.initial_volume - 0.33).abs() < 0.001);
    }

    #[test]
    fn builder_volume_clamp_high() {
        let builder = AudioOutputBuilder::new().initial_volume(2.0);
        assert!((builder.config.initial_volume - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn builder_volume_clamp_low() {
        let builder = AudioOutputBuilder::new().initial_volume(-0.5);
        assert!(builder.config.initial_volume.abs() < f32::EPSILON);
    }

    #[test]
    fn builder_volume_zero() {
        let b = AudioOutputBuilder::new().initial_volume(0.0);
        assert!(b.config.initial_volume.abs() < f32::EPSILON);
    }

    #[test]
    fn builder_volume_one() {
        let b = AudioOutputBuilder::new().initial_volume(1.0);
        assert!((b.config.initial_volume - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn builder_overwrite_sample_rate() {
        let b = AudioOutputBuilder::new()
            .sample_rate(48000)
            .sample_rate(22050);
        assert_eq!(b.config.sample_rate, 22050);
    }

    #[test]
    fn builder_overwrite_channels() {
        let b = AudioOutputBuilder::new().channels(6).channels(1);
        assert_eq!(b.config.channels, 1);
    }

    // ═══════════════════════════════════════════════════════════════════
    // new_output — invalid spec (no PA needed)
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn new_output_zero_channels_returns_config_error() {
        let config = OutputConfig {
            sample_rate: 44100,
            channels: 0,
            initial_volume: 1.0,
        };
        match AudioOutput::new_output(config) {
            Err(OutputError::ConfigError(msg)) => {
                assert!(msg.contains("Invalid sample spec"), "got: {msg}");
                assert!(msg.contains("0 ch"), "got: {msg}");
            }
            Err(other) => panic!("expected ConfigError, got: {other}"),
            Ok(_) => panic!("expected error for 0 channels"),
        }
    }

    #[test]
    fn new_output_zero_rate_returns_config_error() {
        let config = OutputConfig {
            sample_rate: 0,
            channels: 2,
            initial_volume: 1.0,
        };
        match AudioOutput::new_output(config) {
            Err(OutputError::ConfigError(msg)) => {
                assert!(msg.contains("Invalid sample spec"), "got: {msg}");
                assert!(msg.contains("0Hz"), "got: {msg}");
            }
            Err(other) => panic!("expected ConfigError, got: {other}"),
            Ok(_) => panic!("expected error for 0 sample rate"),
        }
    }

    #[test]
    fn new_output_zero_rate_and_channels_returns_config_error() {
        let config = OutputConfig {
            sample_rate: 0,
            channels: 0,
            initial_volume: 1.0,
        };
        match AudioOutput::new_output(config) {
            Err(OutputError::ConfigError(_)) => {} // expected
            Err(other) => panic!("expected ConfigError, got: {other}"),
            Ok(_) => panic!("expected error for 0 rate + 0 channels"),
        }
    }

    #[test]
    fn new_output_valid_config_no_panic() {
        // With a valid spec, new_output will attempt PA connection.
        // In CI (no PA server) it returns an error; with PA it succeeds.
        // Either way: no panic.
        let _result = AudioOutput::new_output(OutputConfig::default());
    }

    #[test]
    fn builder_build_invalid_spec_returns_error() {
        let result = AudioOutputBuilder::new().channels(0).build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_build_valid_no_panic() {
        // Same as new_output_valid_config_no_panic but via builder.
        let _result = AudioOutputBuilder::new()
            .sample_rate(44100)
            .channels(2)
            .initial_volume(0.5)
            .build();
    }

    // ═══════════════════════════════════════════════════════════════════
    // MainloopGuard (private RAII — exercised indirectly via PA tests)
    // ═══════════════════════════════════════════════════════════════════
    //
    // MainloopGuard's lock / unlock / relock / Drop paths are exercised
    // by every PA-dependent test below that calls play, pause, write,
    // flush, set_volume, or available_space.

    // ═══════════════════════════════════════════════════════════════════
    // PA-dependent tests — skipped when no server is available
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn pa_construction_default() {
        let Some(_output) = try_create_output() else {
            return;
        };
        // Construction succeeded — stream is corked (paused).
    }

    #[test]
    fn pa_construction_48000_stereo() {
        let Some(_output) = try_create_output_with(48000, 2, 1.0) else {
            return;
        };
    }

    #[test]
    fn pa_construction_44100_mono() {
        let Some(_output) = try_create_output_with(44100, 1, 0.5) else {
            return;
        };
    }

    #[test]
    fn pa_construction_zero_volume() {
        let Some(_output) = try_create_output_with(44100, 2, 0.0) else {
            return;
        };
    }

    #[test]
    fn pa_play_and_pause() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        assert!(output.play().is_ok());
        assert!(output.pause().is_ok());
    }

    #[test]
    fn pa_play_pause_play() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        assert!(output.play().is_ok());
        assert!(output.pause().is_ok());
        assert!(output.play().is_ok());
    }

    #[test]
    fn pa_pause_while_already_corked() {
        // Stream starts corked — pause again should be harmless.
        let Some(mut output) = try_create_output() else {
            return;
        };
        assert!(output.pause().is_ok());
    }

    #[test]
    fn pa_write_empty_returns_zero() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        let written = output.write(&[]);
        assert_eq!(written, 0);
    }

    #[test]
    fn pa_write_silence() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        let silence = vec![0.0f32; 1024];
        let written = output.write(&silence);
        assert!(written > 0, "expected >0 samples written, got {written}");
    }

    #[test]
    fn pa_write_sine_wave() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        // Generate a tiny sine wave (440 Hz, ~23 ms at 44100 Hz, stereo)
        let sample_rate = 44100.0f32;
        let freq = 440.0f32;
        let num_frames = 1024;
        let mut samples = Vec::with_capacity(num_frames * 2);
        for i in 0..num_frames {
            let t = i as f32 / sample_rate;
            let val = (2.0 * std::f32::consts::PI * freq * t).sin() * 0.1;
            samples.push(val); // left
            samples.push(val); // right
        }
        let written = output.write(&samples);
        assert!(written > 0);
    }

    #[test]
    fn pa_write_returns_sample_count() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        let samples = vec![0.0f32; 512];
        let written = output.write(&samples);
        // Should have written exactly 512 or close to it
        assert!(
            written <= 512,
            "wrote {written} but only submitted 512 samples"
        );
    }

    #[test]
    fn pa_available_space_positive() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        let space = output.available_space();
        // A freshly opened stream should have some writable space
        assert!(space > 0, "expected positive available space, got {space}");
    }

    #[test]
    fn pa_available_space_while_corked() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        // Stream starts corked — available_space should still not panic
        let _space = output.available_space();
    }

    #[test]
    fn pa_buffered_samples_always_zero() {
        let Some(output) = try_create_output() else {
            return;
        };
        assert_eq!(output.buffered_samples(), 0);
    }

    #[test]
    fn pa_buffered_samples_after_write() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        let _ = output.write(&[0.0f32; 256]);
        // Still returns 0 — our implementation always does
        assert_eq!(output.buffered_samples(), 0);
    }

    #[test]
    fn pa_flush() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        let _ = output.write(&[0.0f32; 256]);
        // flush should not panic
        output.flush();
    }

    #[test]
    fn pa_flush_while_corked() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        // flush while corked — should be harmless
        output.flush();
    }

    #[test]
    fn pa_set_volume_full() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        // Give PA a moment to assign a sink-input index
        std::thread::sleep(Duration::from_millis(50));
        let result = output.set_volume(1.0);
        // May fail if index not yet assigned; that's OK — no panic is key
        if let Err(e) = &result {
            eprintln!("[pa_set_volume_full] set_volume error (expected in some envs): {e}");
        }
    }

    #[test]
    fn pa_set_volume_zero() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let _result = output.set_volume(0.0);
    }

    #[test]
    fn pa_set_volume_mid() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let _result = output.set_volume(0.5);
    }

    #[test]
    fn pa_set_volume_clamps() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        // Should clamp to 1.0, not panic
        let _result = output.set_volume(1.5);
        // Should clamp to 0.0, not panic
        let _result = output.set_volume(-0.5);
    }

    #[test]
    fn pa_matches_config_same() {
        let Some(output) = try_create_output() else {
            return;
        };
        // Default config: 44100 Hz, 2 ch
        assert!(output.matches_config(44100, 2));
    }

    #[test]
    fn pa_matches_config_different_rate() {
        let Some(output) = try_create_output() else {
            return;
        };
        assert!(!output.matches_config(48000, 2));
    }

    #[test]
    fn pa_matches_config_different_channels() {
        let Some(output) = try_create_output() else {
            return;
        };
        assert!(!output.matches_config(44100, 1));
    }

    #[test]
    fn pa_matches_config_both_different() {
        let Some(output) = try_create_output() else {
            return;
        };
        assert!(!output.matches_config(48000, 6));
    }

    #[test]
    fn pa_matches_config_48000() {
        let Some(output) = try_create_output_with(48000, 2, 1.0) else {
            return;
        };
        assert!(output.matches_config(48000, 2));
        assert!(!output.matches_config(44100, 2));
    }

    #[test]
    fn pa_drop_after_play() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        let _ = output.write(&[0.0f32; 256]);
        // Drop should drain, disconnect, and stop cleanly.
        drop(output);
    }

    #[test]
    fn pa_drop_while_corked() {
        let Some(output) = try_create_output() else {
            return;
        };
        // Drop while still corked (never played) — should not hang or panic.
        drop(output);
    }

    #[test]
    fn pa_drop_after_pause() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        output.pause().unwrap();
        drop(output);
    }

    #[test]
    fn pa_full_lifecycle() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        // Play
        output.play().unwrap();
        // Write some silence
        let _ = output.write(&[0.0f32; 2048]);
        // Check space
        let _space = output.available_space();
        // Check buffered (always 0)
        assert_eq!(output.buffered_samples(), 0);
        // Pause
        output.pause().unwrap();
        // Flush
        output.flush();
        // Resume
        output.play().unwrap();
        // Set volume
        std::thread::sleep(Duration::from_millis(50));
        let _vol = output.set_volume(0.7);
        // Config check
        assert!(output.matches_config(44100, 2));
        // Drop
        drop(output);
    }

    #[test]
    fn pa_multiple_sequential_outputs() {
        // Creating and dropping multiple outputs in sequence should work.
        for _ in 0..3 {
            let Some(mut output) = try_create_output() else {
                return;
            };
            output.play().unwrap();
            let _ = output.write(&[0.0f32; 512]);
            output.pause().unwrap();
            drop(output);
        }
    }

    #[test]
    fn pa_write_large_buffer() {
        let Some(mut output) = try_create_output() else {
            return;
        };
        output.play().unwrap();
        // Write a large buffer — tests the write loop's chunking
        let samples = vec![0.0f32; 16384];
        let written = output.write(&samples);
        assert!(written > 0, "expected >0 samples written");
    }
}
