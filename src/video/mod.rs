// SPDX-License-Identifier: MIT

//! GStreamer-backed video playback for TIDAL music videos.
//!
//! TIDAL music videos are delivered as **DRM-free HLS** (H.264 video + AAC
//! audio in MPEG-TS segments), so they can be played with a stock GStreamer
//! `playbin` pipeline — no Widevine CDM required.
//!
//! A ready-made video-player crate isn't an option here: the ones that
//! exist target upstream `iced`, whereas the app runs on libcosmic's
//! incompatible `iced` fork. So this module reimplements the essential
//! slice: a `playbin` whose **video**
//! is routed to an `appsink` that hands us raw **RGBA** frames, while its
//! **audio** is split by a `tee` between the default audio sink (so you hear
//! it) and a second `appsink` that feeds decoded **PCM** into the shared
//! spectrum analyzer (so the HUD visualizer reacts to it). The latest decoded
//! frame is published into a shared buffer that the UI samples each redraw and
//! paints with a normal `cosmic::widget::image`.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;

use crate::audio::spectrum::SharedSpectrumAnalyzer;

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
    // Only yield a complete frame whose byte length matches its dimensions.
    (rgba.len() == row_bytes * h).then_some(VideoFrame {
        width: w as u32,
        height: h as u32,
        rgba: Arc::new(rgba),
    })
}

/// Sample rate the audio tap resamples to before feeding the analyzer.
///
/// Forcing a constant rate (rather than whatever the AAC stream happens to
/// use) means the shared [`SharedSpectrumAnalyzer`] — created at 44.1 kHz by
/// the audio engine — maps FFT bins to frequency bands with the rate it
/// expects, so the bars line up the same way they do for audio tracks.
const TAP_RATE: i32 = 44_100;

/// Interleaved stereo, the layout [`SharedSpectrumAnalyzer::push_stereo_samples`]
/// expects.
const TAP_CHANNELS: i32 = 2;

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

/// Fixed size every frame is scaled to before it reaches the UI.
///
/// HLS is adaptive: playbin switches between 320×180, 640×360, 720p… as
/// bandwidth allows. If we forwarded each variant's native size, the wgpu
/// image atlas would re-allocate a different-sized texture on every switch,
/// fragmenting until uploads fail — which made the pane flicker and blank on
/// exactly the videos that change resolution. Scaling to one constant 16:9
/// size keeps the atlas seeing a single, stable texture.
const FRAME_W: i32 = 640;
const FRAME_H: i32 = 360;

/// Build the audio-sink bin: `audioconvert ! audioresample ! capsfilter ! tee`
/// fanning out to (1) the default audio sink so the user hears the video, and
/// (2) a second `appsink` that copies decoded PCM into `analyzer` so the HUD
/// visualizer animates to the music.
///
/// Returned as a generic [`gst::Element`] ready to hand to `playbin` via its
/// `audio-sink` property.
fn build_audio_tap_bin(analyzer: SharedSpectrumAnalyzer) -> Result<gst::Element, String> {
    let convert = gst::ElementFactory::make("audioconvert")
        .build()
        .map_err(|e| format!("failed to create audioconvert: {e}"))?;
    let resample = gst::ElementFactory::make("audioresample")
        .build()
        .map_err(|e| format!("failed to create audioresample: {e}"))?;

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
    // from stalling the tee.
    let tap_queue = gst::ElementFactory::make("queue")
        .build()
        .map_err(|e| format!("failed to create tap queue: {e}"))?;
    let tap_sink = gst_app::AppSink::builder()
        .caps(&caps)
        .max_buffers(8)
        .drop(true)
        .sync(false)
        .build();
    tap_sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let Ok(sample) = sink.pull_sample() else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                if let Some(samples) = extract_f32_samples(&sample) {
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
        &capsfilter,
        &tee,
        &play_queue,
        &audiosink,
        &tap_queue,
        tap_sink_el,
    ])
    .map_err(|e| format!("failed to assemble audio sink bin: {e}"))?;

    gst::Element::link_many([&convert, &resample, &capsfilter, &tee])
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

    Ok(bin.upcast())
}

/// A running video pipeline.
///
/// Dropping the player tears the pipeline down (sets it to `Null`), stopping
/// both audio and video.
pub struct VideoPlayer {
    playbin: gst::Element,
    bus: gst::Bus,
    frame: FrameBuffer,
    seq: Arc<AtomicU64>,
    eos: Arc<AtomicBool>,
    errored: Arc<AtomicBool>,
}

impl VideoPlayer {
    /// Start playing the given HLS (`.m3u8`) URL.
    ///
    /// Audio begins immediately through the default sink; decoded video frames
    /// start landing in the shared [`FrameBuffer`] as soon as the first
    /// keyframe is processed.
    ///
    /// When `analyzer` is `Some`, the pipeline's audio is split so that decoded
    /// PCM is also fed into it, driving the now-playing visualizer. When `None`
    /// (e.g. no audio engine was created), audio plays through playbin's stock
    /// sink and nothing is tapped.
    pub fn new(url: &str, analyzer: Option<SharedSpectrumAnalyzer>) -> Result<Self, String> {
        gst::init().map_err(|e| format!("gstreamer init failed: {e}"))?;

        let playbin = gst::ElementFactory::make("playbin")
            .build()
            .map_err(|e| format!("failed to create playbin: {e}"))?;
        playbin.set_property("uri", url);

        // Accept only RGBA at a fixed size: `videoscale` downscales whatever
        // variant playbin decodes to a constant texture size (see FRAME_W/H).
        let caps = gst_video::VideoCapsBuilder::new()
            .format(gst_video::VideoFormat::Rgba)
            .width(FRAME_W)
            .height(FRAME_H)
            .pixel_aspect_ratio(gst::Fraction::new(1, 1))
            .build();
        let appsink = gst_app::AppSink::builder()
            .caps(&caps)
            .max_buffers(2)
            .drop(true)
            .build();

        let frame: FrameBuffer = Arc::new(Mutex::new(None));
        let seq = Arc::new(AtomicU64::new(0));
        let eos = Arc::new(AtomicBool::new(false));

        {
            let frame = Arc::clone(&frame);
            let seq = Arc::clone(&seq);
            let eos_cb = Arc::clone(&eos);
            appsink.set_callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let Ok(sample) = sink.pull_sample() else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        // Extract a tightly-packed RGBA frame. On any transient
                        // failure we skip this one sample rather than erroring
                        // (and tearing down) the whole pipeline.
                        let extracted = extract_rgba_frame(&sample);

                        if let Some(vframe) = extracted {
                            if let Ok(mut guard) = frame.lock() {
                                *guard = Some(vframe);
                            }
                            seq.fetch_add(1, Ordering::Release);
                        }
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .eos(move |_| {
                        eos_cb.store(true, Ordering::Release);
                    })
                    .build(),
            );
        }

        // Route playbin's decoded video through `videoconvert ! videoscale ! appsink`.
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
            .map_err(|e| format!("failed to create ghost pad: {e}"))?;
        bin.add_pad(&ghost)
            .map_err(|e| format!("failed to add ghost pad: {e}"))?;
        playbin.set_property("video-sink", &bin);

        // When an analyzer is supplied, replace playbin's audio sink with the
        // tee'd tap bin so decoded PCM also feeds the visualizer.  If building
        // the tap fails for any reason, fall back to playbin's default sink
        // (audio still plays; the visualizer just stays idle) rather than
        // failing the whole video.
        if let Some(analyzer) = analyzer {
            match build_audio_tap_bin(analyzer) {
                Ok(audio_bin) => playbin.set_property("audio-sink", &audio_bin),
                Err(e) => tracing::warn!("Audio visualizer tap unavailable: {e}"),
            }
        }

        let bus = playbin
            .bus()
            .ok_or_else(|| "playbin has no bus".to_string())?;

        playbin
            .set_state(gst::State::Playing)
            .map_err(|e| format!("failed to start playback: {e}"))?;

        Ok(Self {
            playbin,
            bus,
            frame,
            seq,
            eos,
            errored: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Drain the pipeline bus, logging any warnings/errors.  Returns `true` if
    /// the pipeline has hit a fatal error (the caller should stop or skip).
    pub fn poll(&self) -> bool {
        use gst::MessageView;
        while let Some(msg) = self.bus.pop() {
            match msg.view() {
                MessageView::Error(err) => {
                    tracing::error!(
                        "Video pipeline error from {:?}: {} ({:?})",
                        err.src().map(|s| s.path_string()),
                        err.error(),
                        err.debug()
                    );
                    self.errored.store(true, Ordering::Release);
                }
                MessageView::Warning(w) => {
                    tracing::warn!("Video pipeline warning: {} ({:?})", w.error(), w.debug());
                }
                _ => {}
            }
        }
        self.errored.load(Ordering::Acquire)
    }

    /// Shared handle to the latest decoded frame, for the UI to sample.
    pub fn frame_buffer(&self) -> FrameBuffer {
        Arc::clone(&self.frame)
    }

    /// Monotonically increasing counter of decoded frames (lets the UI skip a
    /// redraw when nothing new has arrived).
    pub fn frame_seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    /// `true` once the stream has reached its end.
    pub fn is_eos(&self) -> bool {
        self.eos.load(Ordering::Acquire)
    }

    /// Pause both audio and video.
    pub fn pause(&self) {
        let _ = self.playbin.set_state(gst::State::Paused);
    }

    /// Resume playback.
    pub fn resume(&self) {
        let _ = self.playbin.set_state(gst::State::Playing);
    }

    /// Set output volume (0.0..=1.0).
    pub fn set_volume(&self, volume: f64) {
        self.playbin.set_property("volume", volume.clamp(0.0, 1.0));
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

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a synthetic `videotestsrc` through the same
    /// `videoconvert ! videoscale ! appsink` path the player uses, and confirm
    /// a frame is extracted at the requested RGBA size with the right byte
    /// length. No network needed. Skips gracefully if GStreamer or its base
    /// plugins aren't available at runtime.
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

    /// Drive a synthetic `audiotestsrc` through the same `F32LE` interleaved
    /// stereo caps the audio tap enforces, and confirm PCM samples extract as
    /// a non-empty, channel-aligned `f32` slice. No network needed. Skips
    /// gracefully if GStreamer or its base plugins aren't available.
    #[test]
    fn extracts_f32_pcm_from_a_test_source() {
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
        let caps = gst::Caps::builder("audio/x-raw")
            .field("format", "F32LE")
            .field("layout", "interleaved")
            .field("channels", TAP_CHANNELS)
            .field("rate", TAP_RATE)
            .build();
        let appsink = gst_app::AppSink::builder().caps(&caps).build();

        let pipeline = gst::Pipeline::new();
        let sink = appsink.upcast_ref::<gst::Element>();
        if pipeline
            .add_many([&src, &convert, &resample, sink])
            .is_err()
            || gst::Element::link_many([&src, &convert, &resample, sink]).is_err()
            || pipeline.set_state(gst::State::Playing).is_err()
        {
            return;
        }

        let sample = appsink
            .try_pull_sample(gst::ClockTime::from_seconds(5))
            .expect("test source should yield an audio sample within 5s");
        let samples = extract_f32_samples(&sample).expect("PCM should extract");

        assert!(!samples.is_empty(), "expected some PCM samples");
        // Interleaved stereo: the frame count must be a whole number.
        assert_eq!(
            samples.len() % TAP_CHANNELS as usize,
            0,
            "stereo PCM should be channel-aligned"
        );

        let _ = pipeline.set_state(gst::State::Null);
    }
}
