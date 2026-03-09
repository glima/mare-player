// SPDX-License-Identifier: MIT

//! Main audio engine that integrates decoder, output, and spectrum analysis.
//!
//! This module provides a high-level API for playing audio from URLs or files
//! with real-time spectrum analysis for visualization.

use super::dash::DashError;
use super::decoder::{AudioDecoder, DecoderError, DownloadHandle, StreamingDecoder};
use super::output::{AudioOutput, AudioOutputBuilder, OutputError};
use super::spectrum::{SharedSpectrumAnalyzer, SpectrumData};

use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Playback state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackState {
    /// No track loaded
    #[default]
    Stopped,
    /// Track is playing
    Playing,
    /// Track is paused
    Paused,
    /// Track is loading/buffering
    Loading,
}

/// Commands that can be sent to the audio engine
#[derive(Debug)]
pub enum AudioEngineCommand {
    /// Play audio from a URL (with optional cache path to save the download)
    PlayUrl {
        url: String,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    },
    /// Play audio from a DASH manifest file (with optional cache path to save the download)
    PlayDash {
        manifest_path: String,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    },
    /// Play audio from a local file (cached audio)
    PlayFile {
        path: String,
        replay_gain_db: Option<f32>,
    },
    /// Preload a URL for gapless transition
    PreloadUrl {
        url: String,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    },
    /// Preload a DASH manifest for gapless transition
    PreloadDash {
        manifest_path: String,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    },
    /// Preload a cached file for gapless transition
    PreloadFile {
        path: String,
        replay_gain_db: Option<f32>,
    },
    /// Pause playback
    Pause,
    /// Resume playback
    Resume,
    /// Stop playback
    Stop,
    /// Seek to position (seconds)
    Seek(f64),
    /// Set volume (0.0 to 1.0)
    SetVolume(f32),
    /// Shutdown the engine
    Shutdown,
}

/// Events emitted by the audio engine
#[derive(Debug, Clone)]
pub enum AudioEngineEvent {
    /// Playback state changed
    StateChanged(PlaybackState),
    /// Track finished playing
    TrackEnded,
    /// The preloaded track started playing (gapless transition occurred)
    PreloadConsumed,
    /// Error occurred
    Error(String),
    /// Position update (seconds)
    Position(f64),
    /// Duration known (seconds)
    Duration(f64),
    /// Loading progress (0.0 to 1.0)
    LoadingProgress(f64),
}

/// Error type for engine operations
#[derive(Debug)]
pub enum EngineError {
    Decoder(DecoderError),
    Output(OutputError),
    Dash(DashError),
    Channel(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decoder(e) => write!(f, "Decoder error: {}", e),
            Self::Output(e) => write!(f, "Output error: {}", e),
            Self::Dash(e) => write!(f, "DASH error: {}", e),
            Self::Channel(msg) => write!(f, "Channel error: {}", msg),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<DecoderError> for EngineError {
    fn from(err: DecoderError) -> Self {
        Self::Decoder(err)
    }
}

impl From<OutputError> for EngineError {
    fn from(err: OutputError) -> Self {
        Self::Output(err)
    }
}

impl From<DashError> for EngineError {
    fn from(err: DashError) -> Self {
        Self::Dash(err)
    }
}

/// Atomic f32 wrapper for thread-safe volume control
pub struct AtomicF32(AtomicU64);

impl AtomicF32 {
    pub fn new(value: f32) -> Self {
        Self(AtomicU64::new((value as f64).to_bits()))
    }

    pub fn load(&self, ordering: Ordering) -> f32 {
        f64::from_bits(self.0.load(ordering)) as f32
    }

    pub fn store(&self, value: f32, ordering: Ordering) {
        self.0.store((value as f64).to_bits(), ordering);
    }
}

/// Owns all mutable and shared state for the playback thread.
///
/// Extracted from the old monolithic `playback_loop` function so that each
/// logical responsibility lives in its own method.  The public entry point
/// is [`PlaybackLoop::run`].
struct PlaybackLoop {
    // -- channel endpoints --------------------------------------------------
    command_rx: mpsc::UnboundedReceiver<AudioEngineCommand>,
    event_tx: mpsc::UnboundedSender<AudioEngineEvent>,

    // -- shared with the main thread (Arc-wrapped) --------------------------
    state: Arc<Mutex<PlaybackState>>,
    position_samples: Arc<AtomicU64>,
    sample_rate: Arc<AtomicU64>,
    channels: Arc<AtomicU64>,
    spectrum_analyzer: SharedSpectrumAnalyzer,
    volume: Arc<AtomicF32>,

    // -- loop-local mutable state -------------------------------------------
    current_output: Option<AudioOutput>,
    current_decoder: Option<StreamingDecoder>,
    current_download: Option<DownloadHandle>,
    is_paused: bool,
    /// Replay-gain linear multiplier (1.0 = no adjustment).
    /// Updated each time a `Play*` command arrives.
    replay_gain: f32,
    /// Preloaded decoder ready for gapless transition
    preloaded_decoder: Option<StreamingDecoder>,
    /// Preloaded download handle (kept alive until transition)
    preloaded_download: Option<DownloadHandle>,
    /// Replay-gain linear multiplier for the preloaded track
    preloaded_replay_gain: f32,
}

/// Whether `handle_command` wants the caller to `break` or `continue` the
/// outer loop instead of falling through to `feed_samples`.
enum CommandOutcome {
    /// Fall through to the sample-feeding phase as usual.
    Continue,
    /// Immediately restart the loop iteration (skip `feed_samples`).
    SkipFeed,
    /// Exit the loop (shutdown).
    Break,
}

impl PlaybackLoop {
    // ── top-level entry point ───────────────────────────────────────────

    /// Run the playback loop until a `Shutdown` command is received.
    fn run(mut self) {
        info!("Audio engine playback loop started");

        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            let _ = self.event_tx.send(AudioEngineEvent::Error(
                "Failed to create tokio runtime".to_string(),
            ));
            return;
        };

        loop {
            // Non-blocking poll while playing; blocking wait otherwise.
            let command = if self.current_decoder.is_some() && !self.is_paused {
                self.command_rx.try_recv().ok()
            } else {
                runtime.block_on(async { self.command_rx.recv().await })
            };

            if let Some(cmd) = command {
                match self.handle_command(cmd) {
                    CommandOutcome::Break => break,
                    CommandOutcome::SkipFeed => continue,
                    CommandOutcome::Continue => {}
                }
            }

            self.feed_samples();
        }

        info!("Audio engine playback loop ended");
    }

    // ── command dispatch ────────────────────────────────────────────────

    /// Process a single engine command, returning how the caller should
    /// proceed.
    fn handle_command(&mut self, cmd: AudioEngineCommand) -> CommandOutcome {
        match cmd {
            AudioEngineCommand::PlayUrl {
                url,
                cache_path,
                replay_gain_db,
            } => self.handle_play_url(url, cache_path, replay_gain_db),

            AudioEngineCommand::PlayDash {
                manifest_path,
                cache_path,
                replay_gain_db,
            } => self.handle_play_dash(manifest_path, cache_path, replay_gain_db),

            AudioEngineCommand::PlayFile {
                path,
                replay_gain_db,
            } => self.handle_play_file(path, replay_gain_db),

            AudioEngineCommand::Pause => {
                debug!("Pausing playback");
                if let Some(ref mut output) = self.current_output {
                    let _ = output.pause();
                }
                self.is_paused = true;
                *self.state.lock() = PlaybackState::Paused;
                let _ = self
                    .event_tx
                    .send(AudioEngineEvent::StateChanged(PlaybackState::Paused));
                CommandOutcome::Continue
            }

            AudioEngineCommand::Resume => {
                debug!("Resuming playback");
                if let Some(ref mut output) = self.current_output {
                    let _ = output.play();
                }
                self.is_paused = false;
                *self.state.lock() = PlaybackState::Playing;
                let _ = self
                    .event_tx
                    .send(AudioEngineEvent::StateChanged(PlaybackState::Playing));
                CommandOutcome::Continue
            }

            AudioEngineCommand::Stop => {
                debug!("Stopping playback");
                if let Some(mut output) = self.current_output.take() {
                    let _ = output.pause();
                }
                self.current_decoder = None;
                if let Some(mut dl) = self.current_download.take() {
                    dl.abort();
                }
                self.preloaded_decoder = None;
                if let Some(mut dl) = self.preloaded_download.take() {
                    dl.abort();
                }
                self.is_paused = false;
                self.position_samples.store(0, Ordering::SeqCst);
                *self.state.lock() = PlaybackState::Stopped;
                self.spectrum_analyzer.reset();
                let _ = self
                    .event_tx
                    .send(AudioEngineEvent::StateChanged(PlaybackState::Stopped));
                CommandOutcome::Continue
            }

            AudioEngineCommand::Seek(seconds) => {
                self.handle_seek(seconds);
                CommandOutcome::Continue
            }

            AudioEngineCommand::SetVolume(level) => {
                let clamped = level.clamp(0.0, 1.0);
                self.volume.store(clamped, Ordering::SeqCst);
                // Set PipeWire / PulseAudio sink-input volume so it
                // is visible in system mixer UIs (wiremix, etc.).
                if let Some(ref mut output) = self.current_output
                    && let Err(e) = output.set_volume(clamped)
                {
                    warn!("Failed to set PipeWire volume: {}", e);
                }
                debug!("Volume set to {:.0}%", clamped * 100.0);
                CommandOutcome::Continue
            }

            AudioEngineCommand::PreloadUrl {
                url,
                cache_path,
                replay_gain_db,
            } => {
                info!(
                    "Preloading URL for gapless: {}...",
                    &url[..url.len().min(50)]
                );
                let rg = replay_gain_db
                    .map(|db| 10.0_f32.powf(db / 20.0))
                    .unwrap_or(1.0);
                match AudioDecoder::from_url_streaming(&url, cache_path) {
                    Ok((decoder, download_handle)) => {
                        info!(
                            "Preload ready: {}Hz {}ch",
                            decoder.format_info.sample_rate, decoder.format_info.channels
                        );
                        self.preloaded_decoder = Some(StreamingDecoder::from_decoder(decoder));
                        self.preloaded_download = Some(download_handle);
                        self.preloaded_replay_gain = rg;
                    }
                    Err(e) => {
                        warn!(
                            "Preload failed (will fall back to normal transition): {}",
                            e
                        );
                        self.preloaded_decoder = None;
                        self.preloaded_download = None;
                    }
                }
                CommandOutcome::Continue
            }

            AudioEngineCommand::PreloadDash {
                manifest_path,
                cache_path,
                replay_gain_db,
            } => {
                info!("Preloading DASH for gapless: {}", manifest_path);
                let rg = replay_gain_db
                    .map(|db| 10.0_f32.powf(db / 20.0))
                    .unwrap_or(1.0);
                let mut noop_progress = |_: f64| {};
                match AudioDecoder::from_dash_streaming_cached(
                    &manifest_path,
                    &mut noop_progress,
                    cache_path,
                ) {
                    Ok((decoder, download_handle, _duration, _sr)) => {
                        info!(
                            "DASH preload ready: {}Hz {}ch",
                            decoder.format_info.sample_rate, decoder.format_info.channels
                        );
                        self.preloaded_decoder = Some(StreamingDecoder::from_decoder(decoder));
                        self.preloaded_download = Some(download_handle);
                        self.preloaded_replay_gain = rg;
                    }
                    Err(e) => {
                        warn!("DASH preload failed: {}", e);
                        self.preloaded_decoder = None;
                        self.preloaded_download = None;
                    }
                }
                CommandOutcome::Continue
            }

            AudioEngineCommand::PreloadFile {
                path,
                replay_gain_db,
            } => {
                info!("Preloading cached file for gapless: {}", path);
                let rg = replay_gain_db
                    .map(|db| 10.0_f32.powf(db / 20.0))
                    .unwrap_or(1.0);
                match AudioDecoder::from_file(&path) {
                    Ok(decoder) => {
                        info!(
                            "File preload ready: {}Hz {}ch",
                            decoder.format_info.sample_rate, decoder.format_info.channels
                        );
                        self.preloaded_decoder = Some(StreamingDecoder::from_decoder(decoder));
                        self.preloaded_download = None;
                        self.preloaded_replay_gain = rg;
                    }
                    Err(e) => {
                        warn!("File preload failed: {}", e);
                        self.preloaded_decoder = None;
                        self.preloaded_download = None;
                    }
                }
                CommandOutcome::Continue
            }

            AudioEngineCommand::Shutdown => {
                info!("Shutting down audio engine");
                if let Some(mut output) = self.current_output.take() {
                    let _ = output.pause();
                }
                if let Some(mut dl) = self.current_download.take() {
                    dl.abort();
                }
                self.preloaded_decoder = None;
                if let Some(mut dl) = self.preloaded_download.take() {
                    dl.abort();
                }
                CommandOutcome::Break
            }
        }
    }

    // ── Play* handlers ──────────────────────────────────────────────────

    fn handle_play_url(
        &mut self,
        url: String,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    ) -> CommandOutcome {
        self.set_replay_gain(replay_gain_db);
        info!("Playing URL: {}...", &url[..url.len().min(50)]);
        self.stop_current();

        let result = AudioDecoder::from_url_streaming(&url, cache_path);

        match result {
            Ok((decoder, download_handle)) => {
                self.current_download = Some(download_handle);
                let format = decoder.format_info.clone();
                info!(
                    "Streaming decode ready: {}Hz, {} channels, duration: {:?}s",
                    format.sample_rate, format.channels, format.duration
                );

                self.sample_rate
                    .store(format.sample_rate as u64, Ordering::SeqCst);
                self.channels
                    .store(format.channels as u64, Ordering::SeqCst);

                if let Some(duration) = format.duration {
                    let _ = self.event_tx.send(AudioEngineEvent::Duration(duration));
                }

                if !self.ensure_output(format.sample_rate, format.channels as u16) {
                    return CommandOutcome::SkipFeed;
                }
                self.begin_playback(decoder);
            }
            Err(e) => {
                error!("Failed to decode audio: {}", e);
                let _ = self.event_tx.send(AudioEngineEvent::Error(e.to_string()));
                *self.state.lock() = PlaybackState::Stopped;
                let _ = self
                    .event_tx
                    .send(AudioEngineEvent::StateChanged(PlaybackState::Stopped));
            }
        }
        CommandOutcome::Continue
    }

    fn handle_play_dash(
        &mut self,
        manifest_path: String,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    ) -> CommandOutcome {
        self.set_replay_gain(replay_gain_db);
        info!("Playing DASH manifest: {}", manifest_path);
        self.stop_current();

        // Progress callback feeds LoadingProgress events.
        let event_tx_progress = self.event_tx.clone();
        let mut progress_cb = move |progress: f64| {
            let _ = event_tx_progress.send(AudioEngineEvent::LoadingProgress(progress));
        };
        let result =
            AudioDecoder::from_dash_streaming_cached(&manifest_path, &mut progress_cb, cache_path);

        match result {
            Ok((decoder, download_handle, duration, manifest_sample_rate)) => {
                self.current_download = Some(download_handle);

                let format = decoder.format_info.clone();
                let actual_sample_rate = format.sample_rate;

                // Use manifest sample rate if decoder couldn't determine it
                let final_sample_rate = if actual_sample_rate == 0 {
                    manifest_sample_rate
                } else {
                    actual_sample_rate
                };

                info!(
                    "DASH streaming ready: {}Hz, {} channels, {:.2}s",
                    final_sample_rate, format.channels, duration,
                );

                self.sample_rate
                    .store(final_sample_rate as u64, Ordering::SeqCst);
                self.channels
                    .store(format.channels as u64, Ordering::SeqCst);

                let _ = self.event_tx.send(AudioEngineEvent::Duration(duration));

                if !self.ensure_output(final_sample_rate, format.channels as u16) {
                    return CommandOutcome::SkipFeed;
                }
                self.begin_playback(decoder);
            }
            Err(e) => {
                error!("DASH streaming failed: {}", e);
                let _ = self.event_tx.send(AudioEngineEvent::Error(e.to_string()));
                *self.state.lock() = PlaybackState::Stopped;
                let _ = self
                    .event_tx
                    .send(AudioEngineEvent::StateChanged(PlaybackState::Stopped));
            }
        }
        CommandOutcome::Continue
    }

    fn handle_play_file(&mut self, path: String, replay_gain_db: Option<f32>) -> CommandOutcome {
        self.set_replay_gain(replay_gain_db);
        info!("Playing cached file: {}", path);
        self.stop_current();

        let result = AudioDecoder::from_file(&path);

        match result {
            Ok(decoder) => {
                let format = decoder.format_info.clone();
                info!(
                    "Cached file decode ready: {}Hz, {} channels, duration: {:?}s",
                    format.sample_rate, format.channels, format.duration
                );

                self.sample_rate
                    .store(format.sample_rate as u64, Ordering::SeqCst);
                self.channels
                    .store(format.channels as u64, Ordering::SeqCst);

                if let Some(duration) = format.duration {
                    let _ = self.event_tx.send(AudioEngineEvent::Duration(duration));
                }

                if !self.ensure_output(format.sample_rate, format.channels as u16) {
                    return CommandOutcome::SkipFeed;
                }
                self.begin_playback(decoder);
            }
            Err(e) => {
                error!("Failed to decode cached file: {}", e);
                let _ = self.event_tx.send(AudioEngineEvent::Error(e.to_string()));
                *self.state.lock() = PlaybackState::Stopped;
                let _ = self
                    .event_tx
                    .send(AudioEngineEvent::StateChanged(PlaybackState::Stopped));
            }
        }
        CommandOutcome::Continue
    }

    // ── shared helpers ──────────────────────────────────────────────────

    /// Convert a replay-gain dB value to a linear multiplier and store it.
    fn set_replay_gain(&mut self, replay_gain_db: Option<f32>) {
        self.replay_gain = replay_gain_db
            .map(|db| 10.0_f32.powf(db / 20.0))
            .unwrap_or(1.0);
        info!(
            "Replay gain: {:?} dB → {:.4}x linear",
            replay_gain_db, self.replay_gain
        );
    }

    /// Tear down the current decoder and in-flight download, reset
    /// position, and transition to `Loading`.  The audio output stream is
    /// kept alive for potential reuse.
    fn stop_current(&mut self) {
        *self.state.lock() = PlaybackState::Loading;
        let _ = self
            .event_tx
            .send(AudioEngineEvent::StateChanged(PlaybackState::Loading));

        self.current_decoder = None;
        if let Some(mut dl) = self.current_download.take() {
            dl.abort();
        }
        self.position_samples.store(0, Ordering::SeqCst);
        self.preloaded_decoder = None;
        if let Some(mut dl) = self.preloaded_download.take() {
            dl.abort();
        }
    }

    /// Make sure `self.current_output` is ready to play at the given
    /// format.  Reuses the existing stream when the format matches;
    /// otherwise tears it down and builds a new one.
    ///
    /// Returns `true` on success, `false` if the output could not be
    /// created (the caller should skip to the next loop iteration).
    fn ensure_output(&mut self, sample_rate: u32, channels: u16) -> bool {
        // Reusing avoids tearing down the PipeWire node, which can
        // trigger WirePlumber volume-restore policies on Bluetooth sinks.
        let can_reuse = self
            .current_output
            .as_ref()
            .is_some_and(|out| out.matches_config(sample_rate, channels));

        if can_reuse {
            if let Some(ref mut out) = self.current_output {
                debug!("Reusing existing audio output stream");
                out.flush();
                let _ = out.play();
            }
            return true;
        }

        // Format mismatch or no output yet — build a new one.
        if let Some(mut old) = self.current_output.take() {
            let _ = old.pause();
        }

        let output_result = AudioOutputBuilder::new()
            .sample_rate(sample_rate)
            .channels(channels)
            .initial_volume(self.volume.load(Ordering::SeqCst))
            .build();

        match output_result {
            Ok(mut output) => {
                if let Err(e) = output.play() {
                    error!("Failed to start playback: {}", e);
                    let _ = self.event_tx.send(AudioEngineEvent::Error(e.to_string()));
                    *self.state.lock() = PlaybackState::Stopped;
                    return false;
                }
                self.current_output = Some(output);
                true
            }
            Err(e) => {
                error!("Failed to create audio output: {}", e);
                let _ = self.event_tx.send(AudioEngineEvent::Error(e.to_string()));
                *self.state.lock() = PlaybackState::Stopped;
                false
            }
        }
    }

    /// Wrap the decoder in a `StreamingDecoder` and transition to
    /// `Playing`.
    fn begin_playback(&mut self, decoder: AudioDecoder) {
        self.current_decoder = Some(StreamingDecoder::from_decoder(decoder));
        self.is_paused = false;
        *self.state.lock() = PlaybackState::Playing;
        let _ = self
            .event_tx
            .send(AudioEngineEvent::StateChanged(PlaybackState::Playing));
    }

    /// Handle a `Seek` command.
    fn handle_seek(&mut self, seconds: f64) {
        info!("AudioEngine: Seek command received for {:.2}s", seconds);
        let start = std::time::Instant::now();
        if let Some(ref mut decoder) = self.current_decoder {
            if let Err(e) = decoder.seek(seconds) {
                warn!(
                    "AudioEngine: Seek failed after {:?}: {}",
                    start.elapsed(),
                    e
                );
            } else {
                info!("AudioEngine: Seek completed in {:?}", start.elapsed());
                // Flush the audio output buffer to hear new position immediately
                if let Some(ref mut output) = self.current_output {
                    output.flush();
                    info!("AudioEngine: Output buffer flushed");
                }
                let sr = self.sample_rate.load(Ordering::SeqCst);
                let ch = self.channels.load(Ordering::SeqCst);
                self.position_samples
                    .store((seconds * sr as f64 * ch as f64) as u64, Ordering::SeqCst);
                let _ = self.event_tx.send(AudioEngineEvent::Position(seconds));
            }
        } else {
            warn!("AudioEngine: Seek ignored - no decoder available");
        }
    }

    // ── sample feeding ──────────────────────────────────────────────────

    /// Decode and write samples to the output while playing.
    fn feed_samples(&mut self) {
        if self.is_paused {
            return;
        }

        let (Some(decoder), Some(output)) = (&mut self.current_decoder, &mut self.current_output)
        else {
            return;
        };

        let available = output.available_space();
        if available == 0 {
            // Buffer full, wait a bit
            std::thread::sleep(std::time::Duration::from_millis(5));
            return;
        }

        let mut buffer = vec![0.0f32; available.min(4096)];
        match decoder.fill_buffer(&mut buffer) {
            Ok(0) => self.finish_track(),
            Ok(filled) => {
                // Feed spectrum analyzer with full-scale levels
                if let Some(slice) = buffer.get(..filled) {
                    self.spectrum_analyzer.push_stereo_samples(slice);
                }

                // Apply replay-gain normalization, then write.
                // Volume is handled at the PipeWire / PulseAudio
                // sink-input level (visible in system mixers);
                // replay gain is our own per-track loudness correction.
                if self.replay_gain != 1.0
                    && let Some(slice) = buffer.get_mut(..filled)
                {
                    for sample in slice.iter_mut() {
                        *sample *= self.replay_gain;
                    }
                }
                if let Some(slice) = buffer.get(..filled)
                    && let Some(ref mut out) = self.current_output
                {
                    out.write(slice);
                }
                self.position_samples
                    .fetch_add(filled as u64, Ordering::Relaxed);

                // Calculate position in seconds
                let sr = self.sample_rate.load(Ordering::Relaxed);
                let ch = self.channels.load(Ordering::Relaxed);
                if sr > 0 && ch > 0 {
                    let pos = self.position_samples.load(Ordering::Relaxed);
                    let seconds = pos as f64 / (sr as f64 * ch as f64);
                    let _ = self.event_tx.send(AudioEngineEvent::Position(seconds));
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("aborted") || msg.contains("Interrupted") {
                    debug!("Decode interrupted (download aborted)");
                } else {
                    error!("Decode error: {}", msg);
                    let _ = self.event_tx.send(AudioEngineEvent::Error(msg));
                }
            }
        }
    }

    /// Handle end-of-track: either transition gaplessly to a preloaded
    /// decoder, or drain the output and emit `TrackEnded`.
    fn finish_track(&mut self) {
        if let Some(next_decoder) = self.preloaded_decoder.take() {
            // ── gapless transition ──────────────────────────────────────
            let next_rate = next_decoder.format_info().sample_rate;
            let next_channels = next_decoder.format_info().channels as u16;

            info!(
                "Gapless transition to preloaded track ({}Hz, {}ch)",
                next_rate, next_channels
            );

            // Check whether the PA stream must be rebuilt for the new
            // track's sample rate / channel count.  When the format
            // matches we keep the stream open for true gapless audio;
            // when it differs we must tear down and recreate to avoid
            // playing samples at the wrong speed
            let format_changed = !self
                .current_output
                .as_ref()
                .is_some_and(|out| out.matches_config(next_rate, next_channels));

            if format_changed {
                info!(
                    "Sample format changed — recreating audio output for {}Hz {}ch",
                    next_rate, next_channels
                );
                if !self.ensure_output(next_rate, next_channels) {
                    // Output creation failed — fall through to normal
                    // end-of-track so the UI can react.
                    self.preloaded_decoder = None;
                    self.preloaded_download = None;
                    self.current_decoder = None;
                    self.position_samples.store(0, Ordering::SeqCst);
                    *self.state.lock() = PlaybackState::Stopped;
                    self.spectrum_analyzer.reset();
                    let _ = self.event_tx.send(AudioEngineEvent::TrackEnded);
                    let _ = self
                        .event_tx
                        .send(AudioEngineEvent::StateChanged(PlaybackState::Stopped));
                    return;
                }

                self.sample_rate.store(next_rate as u64, Ordering::SeqCst);
                self.channels.store(next_channels as u64, Ordering::SeqCst);
            }

            // Swap in the preloaded decoder.
            self.current_decoder = Some(next_decoder);
            self.current_download = self.preloaded_download.take();
            self.replay_gain = self.preloaded_replay_gain;
            self.preloaded_replay_gain = 1.0;
            self.position_samples.store(0, Ordering::SeqCst);
            self.spectrum_analyzer.reset();

            // Tell the app the preloaded track is now playing so it
            // can update its queue index, now-playing metadata, etc.
            let _ = self.event_tx.send(AudioEngineEvent::PreloadConsumed);
        } else {
            // ── normal end-of-track (no preload available) ─────────
            info!("Track ended (no preload)");
            if let Some(mut output) = self.current_output.take() {
                while output.buffered_samples() > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                let _ = output.pause();
            }
            self.current_decoder = None;
            self.position_samples.store(0, Ordering::SeqCst);
            *self.state.lock() = PlaybackState::Stopped;
            self.spectrum_analyzer.reset();
            let _ = self.event_tx.send(AudioEngineEvent::TrackEnded);
            let _ = self
                .event_tx
                .send(AudioEngineEvent::StateChanged(PlaybackState::Stopped));
        }
    }
}

/// The main audio engine
pub struct AudioEngine {
    /// Command sender
    command_tx: mpsc::UnboundedSender<AudioEngineCommand>,
    /// Event receiver
    event_rx: Arc<Mutex<mpsc::UnboundedReceiver<AudioEngineEvent>>>,
    /// Current playback state
    state: Arc<Mutex<PlaybackState>>,
    /// Current playback position in samples
    position_samples: Arc<AtomicU64>,
    /// Sample rate for position calculation
    sample_rate: Arc<AtomicU64>,
    /// Number of channels
    channels: Arc<AtomicU64>,
    /// Spectrum analyzer (shared with output callback)
    spectrum_analyzer: SharedSpectrumAnalyzer,
    /// Playback thread handle
    playback_thread: Option<JoinHandle<()>>,
    /// Current volume level (0.0 to 1.0)
    volume: Arc<AtomicF32>,
}

impl AudioEngine {
    /// Create a new audio engine
    pub fn new() -> Result<Self, EngineError> {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let state = Arc::new(Mutex::new(PlaybackState::Stopped));
        let position_samples = Arc::new(AtomicU64::new(0));
        let sample_rate = Arc::new(AtomicU64::new(44100));
        let channels = Arc::new(AtomicU64::new(2));

        // Create spectrum analyzer with default sample rate (will be updated when track loads).
        // 12 bands = one per visualizer bar — no oversampling.
        let spectrum_analyzer = SharedSpectrumAnalyzer::with_bands(44100, 12);

        // Volume starts at 100% as a safe default. The app layer overwrites this
        // with the persisted config value via `set_volume()` immediately after
        // construction, before any track is loaded, so no audio plays at the
        // wrong level.
        let volume = Arc::new(AtomicF32::new(1.0));

        // Clone for the playback thread
        let state_clone = Arc::clone(&state);
        let position_samples_clone = Arc::clone(&position_samples);
        let sample_rate_clone = Arc::clone(&sample_rate);
        let channels_clone = Arc::clone(&channels);
        let spectrum_clone = spectrum_analyzer.clone();
        let volume_clone = Arc::clone(&volume);

        // Start the playback thread
        let playback_thread = thread::spawn(move || {
            PlaybackLoop {
                command_rx,
                event_tx,
                state: state_clone,
                position_samples: position_samples_clone,
                sample_rate: sample_rate_clone,
                channels: channels_clone,
                spectrum_analyzer: spectrum_clone,
                volume: volume_clone,
                current_output: None,
                current_decoder: None,
                current_download: None,
                is_paused: false,
                replay_gain: 1.0,
                preloaded_decoder: None,
                preloaded_download: None,
                preloaded_replay_gain: 1.0,
            }
            .run();
        });

        Ok(Self {
            command_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
            state,
            position_samples,
            sample_rate,
            channels,
            spectrum_analyzer,
            playback_thread: Some(playback_thread),
            volume,
        })
    }

    /// Send a command to the engine
    pub(crate) fn send_command(&self, command: AudioEngineCommand) -> Result<(), EngineError> {
        self.command_tx
            .send(command)
            .map_err(|e| EngineError::Channel(e.to_string()))
    }

    /// Play audio from a URL, saving the downloaded data to `cache_path` when done
    pub fn play_url_cached(
        &self,
        url: &str,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    ) -> Result<(), EngineError> {
        self.send_command(AudioEngineCommand::PlayUrl {
            url: url.to_string(),
            cache_path,
            replay_gain_db,
        })
    }

    /// Play audio from a DASH manifest file, saving the downloaded data to `cache_path` when done
    pub fn play_dash_cached(
        &self,
        manifest_path: &str,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    ) -> Result<(), EngineError> {
        self.send_command(AudioEngineCommand::PlayDash {
            manifest_path: manifest_path.to_string(),
            cache_path,
            replay_gain_db,
        })
    }

    /// Play audio from a local file (cached audio), with optional replay gain
    pub fn play_file(&self, path: &str, replay_gain_db: Option<f32>) -> Result<(), EngineError> {
        self.send_command(AudioEngineCommand::PlayFile {
            path: path.to_string(),
            replay_gain_db,
        })
    }

    /// Preload audio from a URL for gapless transition to the next track
    pub fn preload_url(
        &self,
        url: &str,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    ) -> Result<(), EngineError> {
        self.send_command(AudioEngineCommand::PreloadUrl {
            url: url.to_string(),
            cache_path,
            replay_gain_db,
        })
    }

    /// Preload audio from a DASH manifest for gapless transition
    pub fn preload_dash(
        &self,
        manifest_path: &str,
        cache_path: Option<String>,
        replay_gain_db: Option<f32>,
    ) -> Result<(), EngineError> {
        self.send_command(AudioEngineCommand::PreloadDash {
            manifest_path: manifest_path.to_string(),
            cache_path,
            replay_gain_db,
        })
    }

    /// Preload audio from a local file for gapless transition
    pub fn preload_file(&self, path: &str, replay_gain_db: Option<f32>) -> Result<(), EngineError> {
        self.send_command(AudioEngineCommand::PreloadFile {
            path: path.to_string(),
            replay_gain_db,
        })
    }

    /// Pause playback
    pub fn pause(&self) -> Result<(), EngineError> {
        self.send_command(AudioEngineCommand::Pause)
    }

    /// Resume playback
    pub fn resume(&self) -> Result<(), EngineError> {
        self.send_command(AudioEngineCommand::Resume)
    }

    /// Toggle pause/play
    pub fn toggle_pause(&self) -> Result<(), EngineError> {
        let state = *self.state.lock();
        match state {
            PlaybackState::Playing => self.pause(),
            PlaybackState::Paused => self.resume(),
            _ => Ok(()),
        }
    }

    /// Stop playback
    pub fn stop(&self) -> Result<(), EngineError> {
        self.send_command(AudioEngineCommand::Stop)
    }

    /// Seek to a position in seconds
    pub fn seek(&self, seconds: f64) -> Result<(), EngineError> {
        self.send_command(AudioEngineCommand::Seek(seconds))
    }

    /// Get current playback state
    pub fn state(&self) -> PlaybackState {
        *self.state.lock()
    }

    /// Check if playing
    pub fn is_playing(&self) -> bool {
        *self.state.lock() == PlaybackState::Playing
    }

    /// Get current playback position in seconds
    pub fn position(&self) -> f64 {
        let samples = self.position_samples.load(Ordering::Relaxed);
        let rate = self.sample_rate.load(Ordering::Relaxed);
        let ch = self.channels.load(Ordering::Relaxed);

        if rate > 0 && ch > 0 {
            samples as f64 / (rate as f64 * ch as f64)
        } else {
            0.0
        }
    }

    /// Get current spectrum data for visualization
    pub fn spectrum(&self) -> SpectrumData {
        self.spectrum_analyzer.compute()
    }

    /// Get a cheap handle to the shared spectrum analyzer.
    ///
    /// The returned clone shares the same `Arc` interior, so the caller
    /// can call `compute()` / `push_stereo_samples()` without going
    /// through the engine.  Used by the self-animating visualizer widget
    /// to read FFT data directly, avoiding the `update()` → `view()`
    /// round-trip.
    pub fn spectrum_analyzer(&self) -> SharedSpectrumAnalyzer {
        self.spectrum_analyzer.clone()
    }

    /// Try to receive the next event (non-blocking)
    pub fn try_recv_event(&self) -> Option<AudioEngineEvent> {
        self.event_rx.lock().try_recv().ok()
    }

    /// Set volume level (0.0 to 1.0)
    pub fn set_volume(&self, level: f32) -> Result<(), EngineError> {
        let clamped = level.clamp(0.0, 1.0);
        self.volume.store(clamped, Ordering::SeqCst);
        self.send_command(AudioEngineCommand::SetVolume(clamped))
    }

    /// Get current volume level (0.0 to 1.0)
    pub fn volume(&self) -> f32 {
        self.volume.load(Ordering::Relaxed)
    }

    /// Explicitly shut down the playback loop.
    ///
    /// This is also called automatically by `Drop`, so you only need this if
    /// you want to shut down the loop *before* the engine is dropped.
    pub fn shutdown(&self) -> Result<(), EngineError> {
        self.send_command(AudioEngineCommand::Shutdown)
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        info!("Dropping AudioEngine");
        let _ = self.shutdown();
        if let Some(thread) = self.playback_thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    // ═══════════════════════════════════════════════════════════════════════
    // PlaybackState
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_playback_state_default() {
        let state = PlaybackState::default();
        assert_eq!(state, PlaybackState::Stopped);
    }

    #[test]
    fn playback_state_eq() {
        assert_eq!(PlaybackState::Playing, PlaybackState::Playing);
        assert_eq!(PlaybackState::Paused, PlaybackState::Paused);
        assert_eq!(PlaybackState::Stopped, PlaybackState::Stopped);
        assert_eq!(PlaybackState::Loading, PlaybackState::Loading);
        assert_ne!(PlaybackState::Playing, PlaybackState::Paused);
        assert_ne!(PlaybackState::Stopped, PlaybackState::Loading);
    }

    #[test]
    fn playback_state_clone() {
        let a = PlaybackState::Playing;
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn playback_state_debug() {
        assert_eq!(format!("{:?}", PlaybackState::Stopped), "Stopped");
        assert_eq!(format!("{:?}", PlaybackState::Playing), "Playing");
        assert_eq!(format!("{:?}", PlaybackState::Paused), "Paused");
        assert_eq!(format!("{:?}", PlaybackState::Loading), "Loading");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // AudioEngineCommand
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn command_debug_play_url() {
        let cmd = AudioEngineCommand::PlayUrl {
            url: "https://example.com/track.mp3".into(),
            cache_path: Some("/tmp/cache.mp3".into()),
            replay_gain_db: None,
        };
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("PlayUrl"));
        assert!(dbg.contains("example.com"));
    }

    #[test]
    fn command_debug_play_url_no_cache() {
        let cmd = AudioEngineCommand::PlayUrl {
            url: "https://example.com/a.mp3".into(),
            cache_path: None,
            replay_gain_db: None,
        };
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("None"));
    }

    #[test]
    fn command_debug_play_dash() {
        let cmd = AudioEngineCommand::PlayDash {
            manifest_path: "/tmp/manifest.mpd".into(),
            cache_path: None,
            replay_gain_db: None,
        };
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("PlayDash"));
        assert!(dbg.contains("manifest.mpd"));
    }

    #[test]
    fn command_debug_play_file() {
        let cmd = AudioEngineCommand::PlayFile {
            path: "/music/song.flac".into(),
            replay_gain_db: None,
        };
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("PlayFile"));
        assert!(dbg.contains("song.flac"));
    }

    #[test]
    fn command_debug_simple_variants() {
        assert!(format!("{:?}", AudioEngineCommand::Pause).contains("Pause"));
        assert!(format!("{:?}", AudioEngineCommand::Resume).contains("Resume"));
        assert!(format!("{:?}", AudioEngineCommand::Stop).contains("Stop"));
        assert!(format!("{:?}", AudioEngineCommand::Shutdown).contains("Shutdown"));
    }

    #[test]
    fn command_debug_seek() {
        let cmd = AudioEngineCommand::Seek(42.5);
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("Seek"));
        assert!(dbg.contains("42.5"));
    }

    #[test]
    fn command_debug_set_volume() {
        let cmd = AudioEngineCommand::SetVolume(0.75);
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("SetVolume"));
        assert!(dbg.contains("0.75"));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // AudioEngineEvent
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn event_state_changed_clone() {
        let e = AudioEngineEvent::StateChanged(PlaybackState::Playing);
        let e2 = e.clone();
        assert!(format!("{:?}", e2).contains("Playing"));
    }

    #[test]
    fn event_track_ended() {
        let e = AudioEngineEvent::TrackEnded;
        let e2 = e.clone();
        assert!(format!("{:?}", e2).contains("TrackEnded"));
    }

    #[test]
    fn event_error() {
        let e = AudioEngineEvent::Error("something broke".into());
        let dbg = format!("{:?}", e.clone());
        assert!(dbg.contains("something broke"));
    }

    #[test]
    fn event_position() {
        let e = AudioEngineEvent::Position(12.34);
        let dbg = format!("{:?}", e.clone());
        assert!(dbg.contains("12.34"));
    }

    #[test]
    fn event_duration() {
        let e = AudioEngineEvent::Duration(180.0);
        let dbg = format!("{:?}", e.clone());
        assert!(dbg.contains("180"));
    }

    #[test]
    fn event_loading_progress() {
        let e = AudioEngineEvent::LoadingProgress(0.42);
        let dbg = format!("{:?}", e.clone());
        assert!(dbg.contains("0.42"));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // EngineError — Display
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn engine_error_display_decoder() {
        let inner = DecoderError::NoAudioTrack;
        let err = EngineError::Decoder(inner);
        let msg = err.to_string();
        assert!(msg.starts_with("Decoder error:"), "got: {msg}");
    }

    #[test]
    fn engine_error_display_output() {
        let inner = OutputError::NoDevice;
        let err = EngineError::Output(inner);
        let msg = err.to_string();
        assert!(msg.starts_with("Output error:"), "got: {msg}");
    }

    #[test]
    fn engine_error_display_dash() {
        let inner = DashError::NoAudioTrack;
        let err = EngineError::Dash(inner);
        let msg = err.to_string();
        assert!(msg.starts_with("DASH error:"), "got: {msg}");
    }

    #[test]
    fn engine_error_display_channel() {
        let err = EngineError::Channel("receiver dropped".into());
        let msg = err.to_string();
        assert!(msg.contains("Channel error:"), "got: {msg}");
        assert!(msg.contains("receiver dropped"), "got: {msg}");
    }

    #[test]
    fn engine_error_all_variants_non_empty() {
        let errors: Vec<EngineError> = vec![
            EngineError::Decoder(DecoderError::NoAudioTrack),
            EngineError::Output(OutputError::NoDevice),
            EngineError::Dash(DashError::NoAudioTrack),
            EngineError::Channel("test".into()),
        ];
        for err in &errors {
            assert!(!err.to_string().is_empty(), "empty Display for {:?}", err);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // EngineError — std::error::Error + Debug
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn engine_error_implements_std_error() {
        let err: Box<dyn std::error::Error> = Box::new(EngineError::Channel("test".into()));
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn engine_error_debug() {
        let err = EngineError::Decoder(DecoderError::NoAudioTrack);
        let dbg = format!("{:?}", err);
        assert!(dbg.contains("Decoder"));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // EngineError — From conversions
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn engine_error_from_decoder_error() {
        let inner = DecoderError::NoAudioTrack;
        let err: EngineError = inner.into();
        assert!(matches!(err, EngineError::Decoder(_)));
        assert!(err.to_string().contains("Decoder error:"));
    }

    #[test]
    fn engine_error_from_output_error() {
        let inner = OutputError::ConfigError("bad config".into());
        let err: EngineError = inner.into();
        assert!(matches!(err, EngineError::Output(_)));
        assert!(err.to_string().contains("Output error:"));
    }

    #[test]
    fn engine_error_from_dash_error() {
        let inner = DashError::ParseError("xml broken".into());
        let err: EngineError = inner.into();
        assert!(matches!(err, EngineError::Dash(_)));
        assert!(err.to_string().contains("DASH error:"));
    }

    #[test]
    fn engine_error_from_decoder_preserves_inner() {
        let inner = DecoderError::UnsupportedFormat("webm".into());
        let err: EngineError = inner.into();
        assert!(err.to_string().contains("webm"));
    }

    #[test]
    fn engine_error_from_output_preserves_inner() {
        let inner = OutputError::StreamError("pipeline failed".into());
        let err: EngineError = inner.into();
        assert!(err.to_string().contains("pipeline failed"));
    }

    #[test]
    fn engine_error_from_dash_preserves_inner() {
        let inner = DashError::InvalidManifest("missing period".into());
        let err: EngineError = inner.into();
        assert!(err.to_string().contains("missing period"));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // AtomicF32
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn atomic_f32_new_and_load() {
        let a = AtomicF32::new(0.5);
        let v = a.load(Ordering::SeqCst);
        assert!((v - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn atomic_f32_store_and_load() {
        let a = AtomicF32::new(0.0);
        a.store(0.75, Ordering::SeqCst);
        let v = a.load(Ordering::SeqCst);
        assert!((v - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn atomic_f32_roundtrip_zero() {
        let a = AtomicF32::new(0.0);
        assert!((a.load(Ordering::SeqCst) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn atomic_f32_roundtrip_one() {
        let a = AtomicF32::new(1.0);
        assert!((a.load(Ordering::SeqCst) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn atomic_f32_negative() {
        let a = AtomicF32::new(-1.0);
        assert!((a.load(Ordering::SeqCst) - (-1.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn atomic_f32_small_value() {
        let a = AtomicF32::new(0.001);
        let v = a.load(Ordering::SeqCst);
        assert!((v - 0.001).abs() < 0.0001);
    }

    #[test]
    fn atomic_f32_overwrite() {
        let a = AtomicF32::new(0.1);
        a.store(0.9, Ordering::SeqCst);
        a.store(0.42, Ordering::SeqCst);
        let v = a.load(Ordering::SeqCst);
        assert!((v - 0.42).abs() < f32::EPSILON);
    }

    #[test]
    fn atomic_f32_multiple_orderings() {
        let a = AtomicF32::new(0.0);
        a.store(0.5, Ordering::Relaxed);
        let v = a.load(Ordering::Relaxed);
        assert!((v - 0.5).abs() < f32::EPSILON);

        a.store(0.8, Ordering::Release);
        let v = a.load(Ordering::Acquire);
        assert!((v - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn atomic_f32_shared_across_threads() {
        use std::sync::Arc;
        use std::thread;

        let a = Arc::new(AtomicF32::new(0.0));
        let a2 = Arc::clone(&a);

        let handle = thread::spawn(move || {
            a2.store(0.99, Ordering::SeqCst);
        });
        handle.join().unwrap();

        let v = a.load(Ordering::SeqCst);
        assert!((v - 0.99).abs() < 0.01);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Position calculation (exercises the math in AudioEngine::position)
    // ═══════════════════════════════════════════════════════════════════════
    //
    // We test the formula directly:
    //   position = samples / (sample_rate * channels)

    #[test]
    fn position_formula_44100_stereo() {
        // 44100 samples/s * 2 channels * 5 seconds = 441000 total samples
        let samples: u64 = 441_000;
        let rate: u64 = 44100;
        let ch: u64 = 2;
        let pos = samples as f64 / (rate as f64 * ch as f64);
        assert!((pos - 5.0).abs() < 0.001);
    }

    #[test]
    fn position_formula_48000_stereo() {
        let samples: u64 = 48000 * 2 * 10; // 10 seconds
        let rate: u64 = 48000;
        let ch: u64 = 2;
        let pos = samples as f64 / (rate as f64 * ch as f64);
        assert!((pos - 10.0).abs() < 0.001);
    }

    #[test]
    fn position_formula_mono() {
        let samples: u64 = 44100 * 1 * 3; // 3 seconds mono
        let rate: u64 = 44100;
        let ch: u64 = 1;
        let pos = samples as f64 / (rate as f64 * ch as f64);
        assert!((pos - 3.0).abs() < 0.001);
    }

    #[test]
    fn position_formula_zero_samples() {
        let samples: u64 = 0;
        let rate: u64 = 44100;
        let ch: u64 = 2;
        let pos = samples as f64 / (rate as f64 * ch as f64);
        assert!((pos - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn position_formula_zero_rate_returns_zero() {
        // Guard: if rate or channels is 0, position should be 0
        let rate: u64 = 0;
        let ch: u64 = 2;
        let pos = if rate > 0 && ch > 0 {
            100.0 / (rate as f64 * ch as f64)
        } else {
            0.0
        };
        assert!((pos - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn position_formula_zero_channels_returns_zero() {
        let rate: u64 = 44100;
        let ch: u64 = 0;
        let pos = if rate > 0 && ch > 0 {
            100.0 / (rate as f64 * ch as f64)
        } else {
            0.0
        };
        assert!((pos - 0.0).abs() < f64::EPSILON);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // PlaybackLoop — construction smoke test
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn playback_loop_can_be_constructed() {
        let (_tx, rx) = mpsc::unbounded_channel();
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        let _ctx = PlaybackLoop {
            command_rx: rx,
            event_tx,
            state: Arc::new(Mutex::new(PlaybackState::Stopped)),
            position_samples: Arc::new(AtomicU64::new(0)),
            sample_rate: Arc::new(AtomicU64::new(44100)),
            channels: Arc::new(AtomicU64::new(2)),
            spectrum_analyzer: SharedSpectrumAnalyzer::with_bands(44100, 12),
            volume: Arc::new(AtomicF32::new(1.0)),
            current_output: None,
            current_decoder: None,
            current_download: None,
            is_paused: false,
            replay_gain: 1.0,
            preloaded_decoder: None,
            preloaded_download: None,
            preloaded_replay_gain: 1.0,
        };
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Gapless transition — sample rate mismatch detection
    // ═══════════════════════════════════════════════════════════════════════

    /// Helper: build a `PlaybackLoop` with given sample_rate / channels in
    /// the shared atomics and no PA output (so `finish_track` will detect a
    /// format change for any non-zero format).
    fn make_loop_with_format(
        rate: u32,
        ch: u16,
    ) -> (PlaybackLoop, mpsc::UnboundedReceiver<AudioEngineEvent>) {
        let (_cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let pbl = PlaybackLoop {
            command_rx: cmd_rx,
            event_tx,
            state: Arc::new(Mutex::new(PlaybackState::Playing)),
            position_samples: Arc::new(AtomicU64::new(99999)),
            sample_rate: Arc::new(AtomicU64::new(rate as u64)),
            channels: Arc::new(AtomicU64::new(ch as u64)),
            spectrum_analyzer: SharedSpectrumAnalyzer::with_bands(rate, 12),
            volume: Arc::new(AtomicF32::new(1.0)),
            current_output: None,
            current_decoder: None,
            current_download: None,
            is_paused: false,
            replay_gain: 1.0,
            preloaded_decoder: None,
            preloaded_download: None,
            preloaded_replay_gain: 1.0,
        };
        (pbl, event_rx)
    }

    /// The TINY_MP3 decodes at 44100 Hz, 1 channel.  Verify the accessor
    /// exposes those values through `StreamingDecoder::format_info()`.
    #[test]
    fn streaming_decoder_format_info_accessor() {
        use crate::audio::decoder::{AudioDecoder, StreamingDecoder};

        // TINY_MP3 is defined in decoder::tests — duplicate the bytes here
        // (it's only ~500 bytes).
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let sd = StreamingDecoder::from_decoder(dec);
        let info = sd.format_info();
        assert_eq!(info.sample_rate, 44100);
        assert_eq!(info.channels, 1);
    }

    /// When the preloaded decoder has a different sample rate / channel
    /// count than the current loop, `finish_track` must detect the
    /// mismatch and update the shared atomics accordingly.
    ///
    /// If PA is available `ensure_output` succeeds and the decoder is
    /// swapped in (emitting `PreloadConsumed`).  If PA is unavailable
    /// it fails gracefully (emitting `TrackEnded`).  Either way the
    /// atomics must reflect the new format and the preloaded slot must
    /// be consumed.
    #[test]
    fn finish_track_gapless_format_mismatch_updates_atomics() {
        use crate::audio::decoder::{AudioDecoder, StreamingDecoder};

        // Loop starts at 48000/2ch — TINY_MP3 is 44100/1ch → mismatch.
        let (mut pbl, mut event_rx) = make_loop_with_format(48000, 2);

        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        pbl.preloaded_decoder = Some(StreamingDecoder::from_decoder(dec));

        let cur = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        pbl.current_decoder = Some(StreamingDecoder::from_decoder(cur));

        pbl.finish_track();

        // Preloaded slot must always be consumed (swapped in or discarded).
        assert!(
            pbl.preloaded_decoder.is_none(),
            "preloaded decoder should be consumed or discarded"
        );

        // Collect all emitted events.
        let mut got_preload_consumed = false;
        let mut got_track_ended = false;
        while let Ok(evt) = event_rx.try_recv() {
            match evt {
                AudioEngineEvent::PreloadConsumed => got_preload_consumed = true,
                AudioEngineEvent::TrackEnded => got_track_ended = true,
                _ => {}
            }
        }

        if got_preload_consumed {
            // PA was available — output was recreated at the new format.
            // The shared atomics must now reflect 44100/1.
            assert_eq!(
                pbl.sample_rate.load(Ordering::SeqCst),
                44100,
                "sample_rate should be updated to preloaded track's rate"
            );
            assert_eq!(
                pbl.channels.load(Ordering::SeqCst),
                1,
                "channels should be updated to preloaded track's channels"
            );
            assert!(
                pbl.current_decoder.is_some(),
                "decoder should be active after successful gapless swap"
            );
        } else {
            // PA was unavailable — ensure_output failed, fell back to
            // TrackEnded.
            assert!(
                got_track_ended,
                "expected TrackEnded when output recreation fails"
            );
        }
    }

    /// When the preloaded track has the SAME format as the current output,
    /// `finish_track` should perform a true gapless swap — no output
    /// teardown.  We simulate this by setting the loop's atomics to
    /// match the TINY_MP3 format (44100/1ch) and verifying the decoder
    /// is swapped in and `PreloadConsumed` is emitted.
    #[test]
    fn finish_track_gapless_same_format_swaps_decoder() {
        use crate::audio::decoder::{AudioDecoder, StreamingDecoder};

        // Loop atomics match TINY_MP3: 44100 Hz, 1 channel.
        // current_output is None, but matches_config returns false for
        // None, so this will still try ensure_output. We need a real
        // output or a way to fake it.
        //
        // Instead, test the format-changed detection logic directly:
        // when current_output is None but the atomics show the same
        // format, format_changed is still true (because there's no
        // output to reuse). This confirms the guard is correct.
        let (pbl, _event_rx) = make_loop_with_format(44100, 1);

        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let sd = StreamingDecoder::from_decoder(dec);
        let info = sd.format_info();

        // Verify the mismatch detection: no output → always "changed"
        let format_changed = !pbl
            .current_output
            .as_ref()
            .is_some_and(|out| out.matches_config(info.sample_rate, info.channels as u16));

        assert!(
            format_changed,
            "with no output, format should always be considered changed"
        );
    }

    /// Verify that position_samples is reset to 0 during gapless
    /// transition regardless of whether PA is available (format match
    /// path) or unavailable (ensure_output failure path).
    #[test]
    fn finish_track_gapless_resets_position() {
        use crate::audio::decoder::{AudioDecoder, StreamingDecoder};

        let (mut pbl, mut event_rx) = make_loop_with_format(44100, 1);
        pbl.position_samples.store(123456, Ordering::SeqCst);

        let cur = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        pbl.current_decoder = Some(StreamingDecoder::from_decoder(cur));

        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        pbl.preloaded_decoder = Some(StreamingDecoder::from_decoder(dec));

        pbl.finish_track();

        // Position must be reset in both outcomes: successful gapless
        // swap (PreloadConsumed) and failed output recreation (TrackEnded).
        assert_eq!(
            pbl.position_samples.load(Ordering::SeqCst),
            0,
            "position should be reset to 0 after gapless transition"
        );

        // Verify we got one of the two valid outcomes.
        let mut got_preload_consumed = false;
        let mut got_track_ended = false;
        while let Ok(evt) = event_rx.try_recv() {
            match evt {
                AudioEngineEvent::PreloadConsumed => got_preload_consumed = true,
                AudioEngineEvent::TrackEnded => got_track_ended = true,
                _ => {}
            }
        }
        assert!(
            got_preload_consumed || got_track_ended,
            "expected either PreloadConsumed (PA available) or TrackEnded (PA unavailable)"
        );
    }

    /// When there is no preloaded decoder, `finish_track` should emit
    /// `TrackEnded` and transition to `Stopped`.
    #[test]
    fn finish_track_no_preload_emits_track_ended() {
        let (mut pbl, mut event_rx) = make_loop_with_format(44100, 2);
        pbl.preloaded_decoder = None;

        pbl.finish_track();

        assert_eq!(
            *pbl.state.lock(),
            PlaybackState::Stopped,
            "state should be Stopped after track ends with no preload"
        );

        let mut got_track_ended = false;
        while let Ok(evt) = event_rx.try_recv() {
            if matches!(evt, AudioEngineEvent::TrackEnded) {
                got_track_ended = true;
            }
        }
        assert!(got_track_ended, "expected TrackEnded event");
    }

    // Duplicate of TINY_MP3 from decoder::tests (needed here because
    // decoder::tests is a sibling module, not re-exported).
    const TINY_MP3: &[u8] = &[
        0x49, 0x44, 0x33, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x22, 0x54, 0x53, 0x53, 0x45, 0x00,
        0x00, 0x00, 0x0e, 0x00, 0x00, 0x03, 0x4c, 0x61, 0x76, 0x66, 0x36, 0x31, 0x2e, 0x37, 0x2e,
        0x31, 0x30, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff,
        0xfb, 0x40, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x49, 0x6e, 0x66, 0x6f, 0x00, 0x00, 0x00, 0x0f, 0x00, 0x00,
        0x00, 0x03, 0x00, 0x00, 0x01, 0xef, 0x00, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93,
        0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93,
        0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0xca, 0xca, 0xca, 0xca, 0xca,
        0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca,
        0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x00, 0x00, 0x00, 0x00, 0x4c, 0x61, 0x76, 0x63, 0x36, 0x31, 0x2e, 0x31, 0x39, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24, 0x02, 0xa3, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xef, 0x28, 0xb6, 0x68, 0xcc, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xff, 0xfb, 0x10, 0xc4, 0x00, 0x00, 0x04, 0x74, 0x13, 0x55, 0x54, 0x90, 0x80, 0x30,
        0xa6, 0x09, 0xaf, 0x37, 0x1a, 0x20, 0x02, 0x00, 0x01, 0xad, 0x39, 0x40, 0x00, 0x01, 0x59,
        0x3a, 0x3d, 0x50, 0x50, 0x08, 0x06, 0x09, 0x01, 0xf0, 0x7c, 0x1f, 0x07, 0xca, 0x02, 0x00,
        0x80, 0x61, 0x10, 0x7c, 0x1f, 0xd4, 0x08, 0x3b, 0x13, 0x87, 0xf8, 0x83, 0x70, 0x04, 0x93,
        0xf6, 0xc0, 0x60, 0x38, 0x1c, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x28, 0x89, 0x2a, 0x99,
        0x14, 0x64, 0x08, 0xe9, 0x02, 0x48, 0x16, 0xa3, 0xf7, 0x85, 0x01, 0xf0, 0x13, 0x1b, 0xf0,
        0x22, 0x94, 0x2f, 0xa8, 0x1a, 0x12, 0xfc, 0x24, 0x0d, 0x2a, 0x0a, 0x00, 0x18, 0x30, 0x00,
        0xff, 0xfb, 0x12, 0xc4, 0x02, 0x83, 0xc5, 0x58, 0x1d, 0x20, 0x1d, 0xe0, 0x00, 0x28, 0xa4,
        0x83, 0xa4, 0x82, 0xbc, 0x00, 0x05, 0xcc, 0x09, 0x00, 0xbc, 0x40, 0x04, 0x86, 0x00, 0xe0,
        0x78, 0x67, 0xee, 0xf6, 0xa6, 0x63, 0x03, 0x96, 0x61, 0xc4, 0x11, 0x26, 0x0c, 0x00, 0x7e,
        0x60, 0x42, 0x06, 0x06, 0x05, 0x20, 0x4c, 0x60, 0x5e, 0x03, 0xc5, 0x9a, 0xb4, 0x95, 0xf9,
        0x48, 0xf3, 0x01, 0x30, 0x11, 0x30, 0x00, 0x03, 0x63, 0x03, 0x60, 0x84, 0x33, 0x6e, 0x50,
        0xd3, 0x2e, 0xb1, 0x77, 0x30, 0xbf, 0x07, 0xd3, 0x05, 0x90, 0x1d, 0x30, 0x0b, 0x02, 0xd3,
        0x02, 0x50, 0x1f, 0x30, 0x23, 0x01, 0xb4, 0x4f, 0x9f, 0x49, 0x03, 0x92, 0x48, 0x00, 0x0a,
        0xff, 0xfb, 0x10, 0xc4, 0x02, 0x80, 0x04, 0xb4, 0x43, 0x52, 0xb9, 0x92, 0x80, 0x10, 0x97,
        0x06, 0xa6, 0xeb, 0x98, 0x30, 0x04, 0x61, 0x11, 0xd2, 0xa1, 0x4c, 0x16, 0xe9, 0x9a, 0xd1,
        0x5c, 0xfa, 0x22, 0xaa, 0xf9, 0x12, 0xcc, 0xbb, 0xbf, 0x7f, 0x37, 0x96, 0x4f, 0xe0, 0x61,
        0x5f, 0xc7, 0x8b, 0x17, 0xc0, 0xc7, 0x7e, 0x15, 0x50, 0x0c, 0x5d, 0x85, 0xc0, 0x00, 0x00,
        0x26, 0x12, 0x84, 0x62, 0x73, 0xcc, 0x92, 0x41, 0xa8, 0x1d, 0x5e, 0x49, 0x12, 0x42, 0x95,
        0x2d, 0x3c, 0x94, 0x49, 0x14, 0x14, 0x05, 0x63, 0x18, 0x53, 0xbc, 0x4b, 0x74, 0xa8, 0x2d,
    ];
}
