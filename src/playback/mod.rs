// SPDX-License-Identifier: MIT

//! GStreamer playback engine for both audio tracks and music videos.
//!
//! A single [`MediaPlayer`] plays **both** audio and video through GStreamer
//! `playbin3` (falling back to `playbin`), so volume, seeking, the visualizer
//! tap, position/duration, pause, and gapless are implemented once.
//!
//! The pipeline shape, regardless of media kind:
//!
//! ```text
//! playbin3
//! ├── audio-sink = bin:  audioconvert ! audioresample ! volume(name=rg)
//! │                      ! tee
//! │                        ├─ queue ! autoaudiosink          (audible)
//! │                        └─ queue ! appsink(F32LE/stereo)  (PCM → analyzer)
//! └── video-sink = bin:  videoconvert ! videoscale ! appsink(RGBA, fixed-w)  [video only]
//! ```
//!
//! - `volume(name=rg)` carries **replay-gain normalization**: tracks use
//!   TIDAL's authored album replay gain; videos use a fixed configurable
//!   pre-amp (TIDAL authors no replay-gain for videos). It's a single named
//!   element, so swapping in computed loudness later is a one-element change.
//! - **User volume** is the `playbin.volume` property, kept separate from the
//!   `rg` element so the two compose multiplicatively.
//! - The audio `appsink` tap feeds the same [`SharedSpectrumAnalyzer`] the
//!   visualizer reads, so the HUD reacts identically for audio and video.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;

use crate::audio::spectrum::SharedSpectrumAnalyzer;

mod video_window;
pub use video_window::VideoWindowChild;

/// A single decoded video frame in tightly-packed RGBA8.
#[derive(Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major, no stride padding.
    pub rgba: Arc<Vec<u8>>,
}

/// Shared handle to the most-recently decoded frame.
pub type FrameBuffer = Arc<Mutex<Option<VideoFrame>>>;

/// Sample rate the audio tap resamples to before feeding the analyzer.
///
/// Forcing a constant rate (rather than whatever the stream happens to use)
/// keeps the shared [`SharedSpectrumAnalyzer`] — created at 44.1 kHz — mapping
/// FFT bins to frequency bands the way it expects, so the bars line up the
/// same way for every track and video.
const TAP_RATE: i32 = 44_100;

/// Interleaved stereo, the layout
/// [`SharedSpectrumAnalyzer::push_stereo_samples`] expects.
const TAP_CHANNELS: i32 = 2;

/// Fixed width every embedded video frame is scaled to before it reaches the
/// UI; the height follows the stream's aspect ratio so the inline view fits the
/// picture with no letterboxing. (The pop-out window plays full, native pixels
/// separately.)
///
/// Adaptive streaming switches resolutions mid-playback, but the variants share
/// an aspect ratio, so scaling to one constant width keeps the wgpu image atlas
/// seeing a stable texture instead of reallocating on every variant switch.
const FRAME_W: i32 = 640;

/// Convert a replay-gain value in decibels to a linear `volume` multiplier.
///
/// `volume = 10^(dB / 20)`. A gain of `0.0` dB maps to `1.0` (unity).
pub fn db_to_linear(db: f32) -> f64 {
    10f64.powf(db as f64 / 20.0)
}

/// Build a GStreamer `file://` URI for a local media path (a DASH manifest
/// written to the cache, or a previously-downloaded cached audio file).
///
/// Uses GStreamer's own path-to-URI conversion (which percent-escapes spaces
/// and other special characters) and falls back to a naive `file://` prefix if
/// that fails — e.g. on a relative path.
pub fn file_uri(path: &std::path::Path) -> String {
    gst::glib::filename_to_uri(path, None)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

/// Convert a *perceptual* volume (0.0..=1.0, what the UI slider shows) into the
/// **linear gain** playbin's `volume` property expects.
///
/// playbin/pulsesink treat `volume` as a linear gain and map it back to a
/// **cubic** value for the mixer display (pavucontrol/wiremix). Cubing the
/// perceptual value here cancels that out, so the mixer shows the same
/// percentage as our slider and the loudness tapers evenly toward silence
/// instead of cliffing to zero near the bottom — matching how the previous
/// engine drove the PulseAudio stream volume.
pub fn perceptual_to_gst_volume(perceptual: f64) -> f64 {
    let v = perceptual.clamp(0.0, 1.0);
    v * v * v
}

/// What kind of media a [`MediaPlayer`] is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// Audio-only track: no video sink is attached.
    Audio,
    /// Music video: a video sink bin produces RGBA frames for the HUD.
    Video,
}

impl MediaKind {
    fn has_video(self) -> bool {
        matches!(self, MediaKind::Video)
    }
}

/// Copy a decoded sample into a tightly-packed RGBA [`VideoFrame`].
///
/// Returns `None` for a malformed/incomplete sample, so the caller can skip it
/// rather than tearing down the pipeline. Honours the plane stride and only
/// yields a frame whose byte length matches its declared dimensions.
fn extract_rgba_frame(sample: &gst::Sample) -> Option<VideoFrame> {
    let buffer = sample.buffer()?;
    let caps = sample.caps()?;
    let info = gst_video::VideoInfo::from_caps(caps).ok()?;
    let vframe = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info).ok()?;

    let w = vframe.width() as usize;
    let h = vframe.height() as usize;
    let stride = vframe.plane_stride().first().copied().unwrap_or(0) as usize;
    let src = vframe.plane_data(0).ok()?;
    let row_bytes = w.checked_mul(4)?;
    if w == 0 || h == 0 || stride < row_bytes {
        return None;
    }

    let mut rgba = Vec::with_capacity(row_bytes * h);
    for row in 0..h {
        let start = row * stride;
        rgba.extend_from_slice(src.get(start..start + row_bytes)?);
    }
    (rgba.len() == row_bytes * h).then_some(VideoFrame {
        width: w as u32,
        height: h as u32,
        rgba: Arc::new(rgba),
    })
}

/// Reinterpret a raw `F32LE` audio sample's bytes as interleaved `f32` PCM.
///
/// Returns `None` for a malformed/empty buffer so the caller can skip it. The
/// caps on the tap appsink guarantee `F32LE` interleaved stereo, so the bytes
/// map directly onto little-endian `f32`s.
fn extract_f32_samples(sample: &gst::Sample) -> Option<Vec<f32>> {
    let buffer = sample.buffer()?;
    let map = buffer.map_readable().ok()?;
    let bytes = map.as_slice();
    if bytes.len() < 4 {
        return None;
    }
    Some(
        bytes
            .chunks_exact(4)
            .filter_map(|c| c.try_into().ok().map(f32::from_le_bytes))
            .collect(),
    )
}

/// Build the audio-sink bin: `audioconvert ! audioresample ! volume(name=rg)
/// ! tee` fanning out to (1) the default audio sink so the user hears it, and
/// (2) an `appsink` that copies decoded PCM into `analyzer` for the visualizer.
///
/// Returns the bin (ready for `playbin`'s `audio-sink` property) together with
/// the named `rg` [`gst::Element`] so the caller can update the replay-gain
/// multiplier at any time via [`MediaPlayer::set_replay_gain`].
fn build_audio_sink_bin(
    analyzer: Option<SharedSpectrumAnalyzer>,
    replay_gain_db: f32,
) -> Result<(gst::Element, gst::Element), String> {
    let convert = gst::ElementFactory::make("audioconvert")
        .build()
        .map_err(|e| format!("failed to create audioconvert: {e}"))?;
    let resample = gst::ElementFactory::make("audioresample")
        .build()
        .map_err(|e| format!("failed to create audioresample: {e}"))?;

    // Replay-gain / pre-amp stage. Named so the player can adjust it live and
    // so a future computed-loudness scheme can swap just this element.
    let rg_volume = gst::ElementFactory::make("volume")
        .name("rg")
        .property("volume", db_to_linear(replay_gain_db))
        .build()
        .map_err(|e| format!("failed to create rg volume: {e}"))?;

    // Force a known PCM layout so the analyzer and the appsink agree on it.
    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .field("channels", TAP_CHANNELS)
        .field("rate", TAP_RATE)
        .build();
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()
        .map_err(|e| format!("failed to create capsfilter: {e}"))?;
    let tee = gst::ElementFactory::make("tee")
        .build()
        .map_err(|e| format!("failed to create tee: {e}"))?;

    // Branch 1 — actually play the audio through the default sink.
    let play_queue = gst::ElementFactory::make("queue")
        .build()
        .map_err(|e| format!("failed to create play queue: {e}"))?;
    let audiosink = gst::ElementFactory::make("autoaudiosink")
        .build()
        .map_err(|e| format!("failed to create autoaudiosink: {e}"))?;

    // Branch 2 — tap PCM into the spectrum analyzer.  `sync(false)` lets the
    // tap pull buffers as they arrive instead of blocking on the pipeline
    // clock (the play branch owns timing); `drop(true)` keeps a slow callback
    // from stalling the tee.  `async=false` is critical: as a secondary sink
    // in the tee it must NOT participate in the pipeline's async state change,
    // otherwise a PAUSED→PLAYING resume can stall forever waiting for this
    // drop-mode tap to preroll, leaving the real audio sink stuck and silent.
    let tap_queue = gst::ElementFactory::make("queue")
        .build()
        .map_err(|e| format!("failed to create tap queue: {e}"))?;
    let tap_sink = gst_app::AppSink::builder()
        .caps(&caps)
        .max_buffers(8)
        .drop(true)
        .sync(false)
        .build();
    tap_sink.set_property("async", false);
    tap_sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let Ok(sample) = sink.pull_sample() else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                // When no analyzer is attached the tap still runs (keeping the
                // tee balanced) but simply discards the PCM.
                if let Some(analyzer) = &analyzer
                    && let Some(samples) = extract_f32_samples(&sample)
                {
                    analyzer.push_stereo_samples(&samples);
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    let bin = gst::Bin::new();
    let tap_sink_el = tap_sink.upcast_ref::<gst::Element>();
    bin.add_many([
        &convert,
        &resample,
        &rg_volume,
        &capsfilter,
        &tee,
        &play_queue,
        &audiosink,
        &tap_queue,
        tap_sink_el,
    ])
    .map_err(|e| format!("failed to assemble audio sink bin: {e}"))?;

    gst::Element::link_many([&convert, &resample, &rg_volume, &capsfilter, &tee])
        .map_err(|e| format!("failed to link audio chain: {e}"))?;
    gst::Element::link_many([&play_queue, &audiosink])
        .map_err(|e| format!("failed to link audio play branch: {e}"))?;
    gst::Element::link_many([&tap_queue, tap_sink_el])
        .map_err(|e| format!("failed to link audio tap branch: {e}"))?;

    // Wire the tee's request pads to each branch's queue.
    let link_branch = |queue: &gst::Element| -> Result<(), String> {
        let tee_src = tee
            .request_pad_simple("src_%u")
            .ok_or_else(|| "tee has no request pad".to_string())?;
        let q_sink = queue
            .static_pad("sink")
            .ok_or_else(|| "queue has no sink pad".to_string())?;
        tee_src
            .link(&q_sink)
            .map_err(|e| format!("failed to link tee branch: {e}"))?;
        Ok(())
    };
    link_branch(&play_queue)?;
    link_branch(&tap_queue)?;

    let sink_pad = convert
        .static_pad("sink")
        .ok_or_else(|| "audioconvert has no sink pad".to_string())?;
    let ghost = gst::GhostPad::with_target(&sink_pad)
        .map_err(|e| format!("failed to create audio ghost pad: {e}"))?;
    bin.add_pad(&ghost)
        .map_err(|e| format!("failed to add audio ghost pad: {e}"))?;

    Ok((bin.upcast(), rg_volume))
}

/// Build the video-sink bin: `videoconvert ! videoscale ! appsink` yielding
/// fixed-width RGBA frames (height follows the stream's aspect ratio) into the
/// shared [`FrameBuffer`].
fn build_video_sink_bin(
    frame: FrameBuffer,
    seq: Arc<AtomicU64>,
    eos: Arc<AtomicBool>,
) -> Result<gst::Element, String> {
    // Constrain only the width; leaving the height free lets videoscale pick a
    // DAR-preserving height (square pixels), so the frame carries the picture's
    // true aspect ratio with no baked-in letterbox bars.
    let caps = gst_video::VideoCapsBuilder::new()
        .format(gst_video::VideoFormat::Rgba)
        .width(FRAME_W)
        .pixel_aspect_ratio(gst::Fraction::new(1, 1))
        .build();
    let appsink = gst_app::AppSink::builder()
        .caps(&caps)
        .max_buffers(2)
        .drop(true)
        .build();

    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let Ok(sample) = sink.pull_sample() else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                if let Some(vframe) = extract_rgba_frame(&sample) {
                    if let Ok(mut guard) = frame.lock() {
                        *guard = Some(vframe);
                    }
                    seq.fetch_add(1, Ordering::Release);
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .eos(move |_| {
                eos.store(true, Ordering::Release);
            })
            .build(),
    );

    let convert = gst::ElementFactory::make("videoconvert")
        .build()
        .map_err(|e| format!("failed to create videoconvert: {e}"))?;
    let scale = gst::ElementFactory::make("videoscale")
        .build()
        .map_err(|e| format!("failed to create videoscale: {e}"))?;
    let bin = gst::Bin::new();
    bin.add_many([&convert, &scale, appsink.upcast_ref::<gst::Element>()])
        .map_err(|e| format!("failed to assemble video sink bin: {e}"))?;
    gst::Element::link_many([&convert, &scale, appsink.upcast_ref::<gst::Element>()])
        .map_err(|e| format!("failed to link video sink bin: {e}"))?;
    let sink_pad = convert
        .static_pad("sink")
        .ok_or_else(|| "videoconvert has no sink pad".to_string())?;
    let ghost = gst::GhostPad::with_target(&sink_pad)
        .map_err(|e| format!("failed to create video ghost pad: {e}"))?;
    bin.add_pad(&ghost)
        .map_err(|e| format!("failed to add video ghost pad: {e}"))?;

    Ok(bin.upcast())
}

/// Create a `playbin3` element, falling back to the older `playbin` if the
/// newer one isn't available in the runtime's plugin set.
fn make_playbin() -> Result<gst::Element, String> {
    if let Ok(pb) = gst::ElementFactory::make("playbin3").build() {
        return Ok(pb);
    }
    gst::ElementFactory::make("playbin")
        .build()
        .map_err(|e| format!("failed to create playbin/playbin3: {e}"))
}

/// A running unified playback pipeline for audio or video.
///
/// Dropping the player tears the pipeline down (sets it to `Null`), stopping
/// both audio and (if present) video.
pub struct MediaPlayer {
    playbin: gst::Element,
    bus: gst::Bus,
    /// The named `rg` `volume` element, for live replay-gain adjustment.
    rg_volume: gst::Element,
    /// Latest decoded video frame (always present, only populated for video).
    frame: FrameBuffer,
    seq: Arc<AtomicU64>,
    eos: Arc<AtomicBool>,
    errored: Arc<AtomicBool>,
    /// What the user wants: `true` = should be playing, `false` = paused.
    /// Drives buffering-aware resume on network streams.
    intended_playing: Arc<AtomicBool>,
    /// Pre-resolved next track for gapless playback: `(uri, replay_gain_db)`.
    /// Consumed by the `about-to-finish` handler. `None` = no gapless next
    /// (the stream will EOS at the end).
    next_uri: Arc<Mutex<Option<(String, f32)>>>,
    /// Count of gapless transitions that have actually started (each non-first
    /// `STREAM_START`). The app compares this against its own counter to learn
    /// when a preloaded track began so it can advance its queue + metadata.
    transitions: Arc<AtomicU64>,
    /// Whether the initial `STREAM_START` has been seen (so it isn't counted
    /// as a transition).
    first_stream_seen: Arc<AtomicBool>,
    kind: MediaKind,
}

impl MediaPlayer {
    /// Start playing an **audio** stream from `uri`.
    ///
    /// `uri` is a GStreamer URI: `file:///tmp/<id>.mpd` for a DASH manifest
    /// written to disk, or a direct `https://…` URL for single-file/BTS
    /// qualities. `replay_gain_db` is TIDAL's authored album replay gain.
    pub fn new_audio(
        uri: &str,
        analyzer: Option<SharedSpectrumAnalyzer>,
        replay_gain_db: f32,
    ) -> Result<Self, String> {
        Self::build(uri, MediaKind::Audio, analyzer, replay_gain_db, None)
    }

    /// Start playing a **video** stream (HLS `.m3u8`) from `uri`.
    ///
    /// `replay_gain_db` is the fixed video pre-amp from config (TIDAL authors
    /// no replay-gain for videos).
    pub fn new_video(
        uri: &str,
        analyzer: Option<SharedSpectrumAnalyzer>,
        replay_gain_db: f32,
    ) -> Result<Self, String> {
        Self::build(uri, MediaKind::Video, analyzer, replay_gain_db, None)
    }

    /// Start a **video** stream that resumes at `position_secs` (e.g. video
    /// pop-in). Unlike [`new_video`](Self::new_video), the pipeline starts
    /// *paused*: it waits until the HLS stream is seekable, seeks to the
    /// position, and only then begins playing — so nothing plays from 0:00
    /// first (which would otherwise flash + jump when the seek lands).
    pub fn new_video_at(
        uri: &str,
        analyzer: Option<SharedSpectrumAnalyzer>,
        replay_gain_db: f32,
        position_secs: f64,
    ) -> Result<Self, String> {
        Self::build(
            uri,
            MediaKind::Video,
            analyzer,
            replay_gain_db,
            Some(position_secs),
        )
    }

    fn build(
        uri: &str,
        kind: MediaKind,
        analyzer: Option<SharedSpectrumAnalyzer>,
        replay_gain_db: f32,
        resume_at: Option<f64>,
    ) -> Result<Self, String> {
        gst::init().map_err(|e| format!("gstreamer init failed: {e}"))?;

        let playbin = make_playbin()?;
        playbin.set_property("uri", uri);

        let frame: FrameBuffer = Arc::new(Mutex::new(None));
        let seq = Arc::new(AtomicU64::new(0));
        let eos = Arc::new(AtomicBool::new(false));

        // Audio sink bin (with the rg volume + tap) is attached for both
        // kinds: video carries audio too, and both feed the visualizer.
        let (audio_bin, rg_volume) = build_audio_sink_bin(analyzer, replay_gain_db)?;
        playbin.set_property("audio-sink", &audio_bin);

        if kind.has_video() {
            let video_bin =
                build_video_sink_bin(Arc::clone(&frame), Arc::clone(&seq), Arc::clone(&eos))?;
            playbin.set_property("video-sink", &video_bin);
        }

        let bus = playbin
            .bus()
            .ok_or_else(|| "playbin has no bus".to_string())?;

        // Gapless playback: when the current stream is about to finish, hand
        // playbin the pre-resolved next URI (if the app staged one). Setting
        // `uri` from this signal is the documented gapless mechanism. We also
        // update the rg gain for the incoming track here.
        let next_uri: Arc<Mutex<Option<(String, f32)>>> = Arc::new(Mutex::new(None));
        {
            let next_uri = Arc::clone(&next_uri);
            let rg = rg_volume.clone();
            playbin.connect("about-to-finish", false, move |values| {
                let pb = values.first()?.get::<gst::Element>().ok()?;
                if let Ok(mut guard) = next_uri.lock()
                    && let Some((uri, rg_db)) = guard.take()
                {
                    tracing::info!("Gapless: staging next stream");
                    rg.set_property("volume", db_to_linear(rg_db));
                    pb.set_property("uri", &uri);
                }
                None
            });
        }

        // For a resume (video pop-in) start *paused* and defer the play until
        // after the seek lands, so nothing plays from 0:00 first. Otherwise
        // start playing immediately.
        let resume = resume_at.filter(|&s| s > 0.5);
        let start_state = if resume.is_some() {
            gst::State::Paused
        } else {
            gst::State::Playing
        };
        // While a resume seek is pending, keep `intended_playing` false: the
        // `poll()` buffering handler only forces PLAYING when it's set, so this
        // stops the app's tick from racing the resume by un-pausing the
        // pipeline before the seek lands. The resume thread sets it true.
        let intended_playing = Arc::new(AtomicBool::new(resume.is_none()));
        playbin
            .set_state(start_state)
            .map_err(|e| format!("failed to start playback: {e}"))?;
        if let Some(pos) = resume {
            let pb = playbin.clone();
            let intended = Arc::clone(&intended_playing);
            std::thread::spawn(move || resume_seek_then_play(&pb, pos, intended));
        }

        Ok(Self {
            playbin,
            bus,
            rg_volume,
            frame,
            seq,
            eos,
            errored: Arc::new(AtomicBool::new(false)),
            intended_playing,
            next_uri,
            transitions: Arc::new(AtomicU64::new(0)),
            first_stream_seen: Arc::new(AtomicBool::new(false)),
            kind,
        })
    }

    /// The media kind this player was created for.
    pub fn kind(&self) -> MediaKind {
        self.kind
    }

    /// Drain the pipeline bus, logging warnings/errors and flagging EOS.
    /// Returns `true` if the pipeline hit a fatal error.
    pub fn poll(&self) -> bool {
        use gst::MessageView;
        while let Some(msg) = self.bus.pop() {
            match msg.view() {
                MessageView::Error(err) => {
                    tracing::error!(
                        "Media pipeline error from {:?}: {} ({:?})",
                        err.src().map(|s| s.path_string()),
                        err.error(),
                        err.debug()
                    );
                    self.errored.store(true, Ordering::Release);
                }
                MessageView::Warning(w) => {
                    tracing::warn!("Media pipeline warning: {} ({:?})", w.error(), w.debug());
                }
                MessageView::Eos(_) => {
                    self.eos.store(true, Ordering::Release);
                }
                MessageView::StreamStart(_) => {
                    // The first stream-start is the initial track; every one
                    // after it marks a gapless transition into a preloaded
                    // track, which the app observes via `transitions()`.
                    if self.first_stream_seen.swap(true, Ordering::AcqRel) {
                        self.transitions.fetch_add(1, Ordering::Release);
                    }
                }
                MessageView::Buffering(b) => {
                    let percent = b.percent();
                    // Fires once per percent (many/sec) — keep it at TRACE so it
                    // doesn't drown DEBUG. State transitions below stay at DEBUG.
                    tracing::trace!("Media pipeline buffering: {percent}%");
                    // Standard streaming protocol: hold the pipeline in PAUSED
                    // while the buffer fills, then return it to PLAYING at 100%
                    // — but only if the user still wants to play (they may have
                    // paused mid-buffer). Without this, a resume on a network
                    // stream that triggers re-buffering gets stuck silent.
                    if self.intended_playing.load(Ordering::Acquire) {
                        let target = if percent < 100 {
                            gst::State::Paused
                        } else {
                            gst::State::Playing
                        };
                        let _ = self.playbin.set_state(target);
                    }
                }
                // Only the top-level pipeline's transitions are interesting.
                MessageView::StateChanged(sc)
                    if sc
                        .src()
                        .is_some_and(|s| s.type_().name().contains("PlayBin")) =>
                {
                    tracing::debug!("Media pipeline state: {:?} -> {:?}", sc.old(), sc.current());
                }
                _ => {}
            }
        }
        self.errored.load(Ordering::Acquire)
    }

    /// Shared handle to the latest decoded video frame (empty for audio).
    pub fn frame_buffer(&self) -> FrameBuffer {
        Arc::clone(&self.frame)
    }

    /// Monotonically increasing decoded-frame counter (video only).
    pub fn frame_seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    /// `true` once the stream has reached its end.
    pub fn is_eos(&self) -> bool {
        self.eos.load(Ordering::Acquire)
    }

    /// Stage the pre-resolved next track for gapless playback. Consumed by the
    /// `about-to-finish` handler when the current stream nears its end.
    pub fn set_next(&self, uri: String, replay_gain_db: f32) {
        if let Ok(mut guard) = self.next_uri.lock() {
            *guard = Some((uri, replay_gain_db));
        }
    }

    /// Drop any staged gapless next track (e.g. the queue or loop mode
    /// changed, so the previously-staged track is no longer correct).
    pub fn clear_next(&self) {
        if let Ok(mut guard) = self.next_uri.lock() {
            *guard = None;
        }
    }

    /// Number of gapless transitions that have started so far (each preloaded
    /// track that began playing without a pipeline rebuild). The app compares
    /// this to its own counter to detect when to advance the queue/metadata.
    pub fn transitions(&self) -> u64 {
        self.transitions.load(Ordering::Acquire)
    }

    /// Pause playback.
    pub fn pause(&self) {
        self.intended_playing.store(false, Ordering::Release);
        match self.playbin.set_state(gst::State::Paused) {
            Ok(change) => tracing::debug!("MediaPlayer pause -> {:?}", change),
            Err(e) => tracing::warn!("MediaPlayer pause failed: {e}"),
        }
    }

    /// Resume playback.
    pub fn resume(&self) {
        self.intended_playing.store(true, Ordering::Release);
        match self.playbin.set_state(gst::State::Playing) {
            Ok(change) => tracing::debug!("MediaPlayer resume -> {:?}", change),
            Err(e) => tracing::warn!("MediaPlayer resume failed: {e}"),
        }
    }

    /// Set user output volume (0.0..=1.0). Composes on top of replay gain.
    ///
    /// `volume` is the perceptual slider level; see [`perceptual_to_gst_volume`]
    /// for why it's cubed before reaching playbin.
    pub fn set_volume(&self, volume: f64) {
        self.playbin
            .set_property("volume", perceptual_to_gst_volume(volume));
    }

    /// Update the replay-gain / pre-amp multiplier from a dB value.
    pub fn set_replay_gain(&self, db: f32) {
        self.rg_volume.set_property("volume", db_to_linear(db));
    }

    /// The current replay-gain multiplier (linear), for inspection/tests.
    pub fn replay_gain(&self) -> f64 {
        self.rg_volume.property::<f64>("volume")
    }

    /// Current playback position in seconds, if known.
    pub fn position_secs(&self) -> Option<f64> {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|t| t.seconds() as f64)
    }

    /// Total duration in seconds, if known.
    pub fn duration_secs(&self) -> Option<f64> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|t| t.seconds() as f64)
    }

    /// Seek to the given position in seconds (best-effort).
    pub fn seek_secs(&self, secs: f64) {
        let pos = gst::ClockTime::from_mseconds((secs.max(0.0) * 1000.0) as u64);
        let _ = self
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, pos);
    }
}

/// Resume a *paused*, freshly-built pipeline at `secs`: wait until it's
/// seekable, seek, then start playing.
///
/// A seek issued before the stream is seekable is silently dropped — and for
/// **HLS** the stream isn't seekable until its media playlist has been fetched
/// and parsed (a network round-trip after preroll). Seeking the prerolled,
/// *paused* pipeline (rather than one already playing from 0:00) lands reliably
/// in a single flush and avoids the play-from-start flash + jump. Runs on a
/// short-lived background thread; logs progress via `tracing`.
fn resume_seek_then_play(pb: &gst::Element, secs: f64, intended: std::sync::Arc<AtomicBool>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut waited_ms = 0u64;
    loop {
        // Block briefly for the async preroll to PAUSED to settle.
        let _ = pb.state(Some(gst::ClockTime::from_mseconds(250)));
        let dur = pb.query_duration::<gst::ClockTime>();
        if dur.is_some_and(|d| d.nseconds() > 0) {
            tracing::info!(
                "seek-resume: seekable after {}ms (duration={:?}s), seeking to {:.1}s",
                waited_ms,
                dur.map(|d| d.seconds()),
                secs
            );
            break;
        }
        if std::time::Instant::now() >= deadline {
            tracing::warn!(
                "seek-resume: not seekable after {}ms; seeking anyway to {:.1}s",
                waited_ms,
                secs
            );
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
        waited_ms += 150;
    }
    // Seek the prerolled, *paused* pipeline, verifying the position actually
    // moved (a single flush seek can still be dropped before the demuxer has
    // the target segment) and retrying a few times if not.
    let target = gst::ClockTime::from_mseconds((secs.max(0.0) * 1000.0) as u64);
    let mut landed = false;
    for attempt in 1..=8 {
        let ok = pb.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, target);
        // Wait for this flush-seek's preroll to settle, then read the position.
        let _ = pb.state(Some(gst::ClockTime::from_seconds(5)));
        let cur = pb
            .query_position::<gst::ClockTime>()
            .map(|p| p.seconds() as f64);
        tracing::info!(
            "seek-resume: attempt {} seek_ok={} pos={:?} (target {:.1}s)",
            attempt,
            ok.is_ok(),
            cur,
            secs
        );
        if cur.is_some_and(|c| c + 2.0 >= secs) {
            landed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    // Hand control back to the normal play/pause + buffering logic, then roll.
    intended.store(true, Ordering::Release);
    let _ = pb.set_state(gst::State::Playing);
    tracing::info!(
        "seek-resume: now playing (landed={}, target {:.1}s)",
        landed,
        secs
    );
}

impl Drop for MediaPlayer {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_to_linear_unity_at_zero() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn db_to_linear_minus_six_is_about_half() {
        // -6.02 dB ≈ 0.5 linear.
        assert!((db_to_linear(-6.0206) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn db_to_linear_plus_six_is_about_two() {
        assert!((db_to_linear(6.0206) - 2.0).abs() < 1e-3);
    }

    #[test]
    fn media_kind_has_video() {
        assert!(MediaKind::Video.has_video());
        assert!(!MediaKind::Audio.has_video());
    }

    #[test]
    fn perceptual_volume_is_cubed_and_clamped() {
        // Endpoints are preserved; mid-slider tapers cubically so the mixer
        // displays the same percentage as the slider.
        assert!((perceptual_to_gst_volume(0.0) - 0.0).abs() < 1e-9);
        assert!((perceptual_to_gst_volume(1.0) - 1.0).abs() < 1e-9);
        assert!((perceptual_to_gst_volume(0.5) - 0.125).abs() < 1e-9);
        // Out-of-range inputs clamp.
        assert_eq!(perceptual_to_gst_volume(-1.0), 0.0);
        assert_eq!(perceptual_to_gst_volume(2.0), 1.0);
    }

    /// Drive a synthetic `videotestsrc` through the same
    /// `videoconvert ! videoscale ! appsink` path the player uses, and confirm
    /// a fixed-size RGBA frame is extracted. Skips gracefully if GStreamer or
    /// its base plugins aren't available at runtime.
    #[test]
    fn extracts_rgba_frame_from_a_test_source() {
        if gst::init().is_err() {
            return;
        }
        let Ok(src) = gst::ElementFactory::make("videotestsrc")
            .property("num-buffers", 1i32)
            .build()
        else {
            return;
        };
        let (Ok(convert), Ok(scale)) = (
            gst::ElementFactory::make("videoconvert").build(),
            gst::ElementFactory::make("videoscale").build(),
        ) else {
            return;
        };
        let caps = gst_video::VideoCapsBuilder::new()
            .format(gst_video::VideoFormat::Rgba)
            .width(64)
            .height(36)
            .build();
        let appsink = gst_app::AppSink::builder().caps(&caps).build();

        let pipeline = gst::Pipeline::new();
        let sink = appsink.upcast_ref::<gst::Element>();
        if pipeline.add_many([&src, &convert, &scale, sink]).is_err()
            || gst::Element::link_many([&src, &convert, &scale, sink]).is_err()
            || pipeline.set_state(gst::State::Playing).is_err()
        {
            return;
        }

        let sample = appsink
            .try_pull_sample(gst::ClockTime::from_seconds(5))
            .expect("test source should yield a sample within 5s");
        let frame = extract_rgba_frame(&sample).expect("a valid frame should extract");

        assert_eq!(frame.width, 64);
        assert_eq!(frame.height, 36);
        assert_eq!(frame.rgba.len(), 64 * 36 * 4);

        let _ = pipeline.set_state(gst::State::Null);
    }

    /// Drive a synthetic `audiotestsrc` through `audioconvert ! audioresample
    /// ! volume(name=rg) ! capsfilter ! appsink` — the audio chain the player
    /// builds — and confirm: (a) PCM extracts as channel-aligned `f32`, and
    /// (b) the named `rg` volume element is settable. Skips gracefully when
    /// GStreamer/base plugins are unavailable.
    #[test]
    fn extracts_f32_pcm_and_rg_volume_is_settable() {
        if gst::init().is_err() {
            return;
        }
        let Ok(src) = gst::ElementFactory::make("audiotestsrc")
            .property("num-buffers", 1i32)
            .build()
        else {
            return;
        };
        let (Ok(convert), Ok(resample)) = (
            gst::ElementFactory::make("audioconvert").build(),
            gst::ElementFactory::make("audioresample").build(),
        ) else {
            return;
        };
        let Ok(rg) = gst::ElementFactory::make("volume")
            .name("rg")
            .property("volume", db_to_linear(-8.0))
            .build()
        else {
            return;
        };
        let caps = gst::Caps::builder("audio/x-raw")
            .field("format", "F32LE")
            .field("layout", "interleaved")
            .field("channels", TAP_CHANNELS)
            .field("rate", TAP_RATE)
            .build();
        let Ok(capsfilter) = gst::ElementFactory::make("capsfilter")
            .property("caps", &caps)
            .build()
        else {
            return;
        };
        let appsink = gst_app::AppSink::builder().caps(&caps).build();

        // The rg element starts at the -8 dB pre-amp and accepts a live change.
        assert!((rg.property::<f64>("volume") - db_to_linear(-8.0)).abs() < 1e-6);
        rg.set_property("volume", db_to_linear(0.0));
        assert!((rg.property::<f64>("volume") - 1.0).abs() < 1e-6);

        let pipeline = gst::Pipeline::new();
        let sink = appsink.upcast_ref::<gst::Element>();
        if pipeline
            .add_many([&src, &convert, &resample, &rg, &capsfilter, sink])
            .is_err()
            || gst::Element::link_many([&src, &convert, &resample, &rg, &capsfilter, sink]).is_err()
            || pipeline.set_state(gst::State::Playing).is_err()
        {
            return;
        }

        let sample = appsink
            .try_pull_sample(gst::ClockTime::from_seconds(5))
            .expect("test source should yield an audio sample within 5s");
        let samples = extract_f32_samples(&sample).expect("PCM should extract");

        assert!(!samples.is_empty(), "expected some PCM samples");
        assert_eq!(
            samples.len() % TAP_CHANNELS as usize,
            0,
            "stereo PCM should be channel-aligned"
        );

        let _ = pipeline.set_state(gst::State::Null);
    }

    /// Assemble the full audio sink bin the player uses and confirm it builds
    /// and exposes a settable `rg` element. Skips gracefully without plugins.
    #[test]
    fn audio_sink_bin_builds_with_named_rg_element() {
        if gst::init().is_err() {
            return;
        }
        let Ok((bin, rg)) = build_audio_sink_bin(None, -8.0) else {
            return;
        };
        // The returned rg handle drives the bin's gain stage.
        assert!((rg.property::<f64>("volume") - db_to_linear(-8.0)).abs() < 1e-6);
        rg.set_property("volume", db_to_linear(-3.0));
        assert!((rg.property::<f64>("volume") - db_to_linear(-3.0)).abs() < 1e-6);
        // Bin has a ghost sink pad ready for playbin's audio-sink property.
        assert!(bin.static_pad("sink").is_some());
    }

    /// The visualizer tap appsink must be `async=false`: as a secondary sink
    /// in the tee it must not participate in the pipeline's async state change,
    /// or a PAUSED→PLAYING resume can stall forever (the bug this guards).
    #[test]
    fn audio_tap_sink_is_async_false() {
        if gst::init().is_err() {
            return;
        }
        let Ok((bin, _rg)) = build_audio_sink_bin(None, 0.0) else {
            return;
        };
        let bin = bin
            .downcast::<gst::Bin>()
            .expect("audio sink should be a bin");
        let mut found = false;
        let mut it = bin.iterate_elements();
        while let Ok(Some(el)) = it.next() {
            if el.type_().name().contains("AppSink") {
                assert!(
                    !el.property::<bool>("async"),
                    "tap appsink must be async=false so resume never stalls"
                );
                found = true;
            }
        }
        assert!(found, "expected an appsink in the audio sink bin");
    }

    /// Drive the real audio sink bin from `audiotestsrc` and confirm the
    /// pipeline returns to PLAYING after a pause (a smoke test for resume).
    /// Skips gracefully if GStreamer or its base plugins are unavailable.
    #[test]
    fn audio_sink_bin_resumes_after_pause() {
        if gst::init().is_err() {
            return;
        }
        let Ok((bin, _rg)) = build_audio_sink_bin(None, 0.0) else {
            return;
        };
        let Ok(src) = gst::ElementFactory::make("audiotestsrc")
            .property("is-live", false)
            .build()
        else {
            return;
        };
        let pipeline = gst::Pipeline::new();
        if pipeline.add_many([&src, &bin]).is_err() || src.link(&bin).is_err() {
            return;
        }

        let wait = gst::ClockTime::from_seconds(5);
        if pipeline.set_state(gst::State::Playing).is_err() {
            let _ = pipeline.set_state(gst::State::Null);
            return;
        }
        let _ = pipeline.state(wait);
        let _ = pipeline.set_state(gst::State::Paused);
        let _ = pipeline.state(wait);

        // The transition that used to stall: PAUSED -> PLAYING must complete.
        let _ = pipeline.set_state(gst::State::Playing);
        let (res, current, _pending) = pipeline.state(wait);
        assert!(res.is_ok(), "resume state change should not error/stall");
        assert_eq!(
            current,
            gst::State::Playing,
            "pipeline should reach PLAYING after resume"
        );

        let _ = pipeline.set_state(gst::State::Null);
    }
}
