// SPDX-License-Identifier: MIT

//! GStreamer-backed video playback for TIDAL music videos.
//!
//! TIDAL music videos are delivered as **DRM-free HLS** (H.264 video + AAC
//! audio in MPEG-TS segments), so they can be played with a stock GStreamer
//! `playbin` pipeline — no Widevine CDM required.
//!
//! We can't use the `iced_video_player` crate because it targets upstream
//! `iced`, whereas the app runs on libcosmic's incompatible `iced` fork. So
//! this module reimplements the essential slice: a `playbin` whose **video**
//! is routed to an `appsink` that hands us raw **RGBA** frames, while its
//! **audio** plays through the default audio sink. The latest decoded frame is
//! published into a shared buffer that the UI samples each redraw and paints
//! with a normal `cosmic::widget::image`.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;

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
    pub fn new(url: &str) -> Result<Self, String> {
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
}
