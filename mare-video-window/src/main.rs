// SPDX-License-Identifier: MIT

//! Maré Player — out-of-process music-video window.
//!
//! A COSMIC **panel applet** cannot spawn a normal toplevel window (the panel
//! runtime parents every surface it creates back into the panel). So when the
//! user "pops out" a music video, the applet launches this tiny companion
//! process instead. As an ordinary process it gets a real, free-floating
//! toplevel window — which is simply the window the GStreamer video sink
//! creates.
//!
//! It is deliberately dumb: no TIDAL client, no auth, no GUI toolkit. The
//! parent resolves the video's HLS URL and hands it down; this just plays it,
//! continuing from the position and volume the inline player had. The two talk
//! over the child's stdio:
//!
//! Parent → child (one command per line on **stdin**):
//!   - `play <position_secs> <url>`  — switch to a new video (used on skip)
//!   - `volume <0.0..1.0>`           — set the user volume
//!   - `seek <secs>`                 — seek
//!   - `pause` / `resume`
//!   - `quit`
//!
//! Child → parent (one event per line on **stdout**):
//!   - `position <secs>`             — current playback position (~2 Hz)
//!   - `eos`                         — the current video reached its end
//!   - `closed`                      — the window was closed / the sink errored
//!
//! When stdin reaches EOF (the parent exited), the child exits too, so it never
//! lingers as an orphan.

use std::io::{BufRead, Write};
use std::thread;
use std::time::Duration;

use gstreamer as gst;
use gstreamer::glib;
use gstreamer::prelude::*;

/// Default loudness pre-amp for videos, in dB. Mirrors the applet's
/// `video_preamp_db` default so popped-out audio matches inline playback.
const DEFAULT_PREAMP_DB: f64 = -8.0;

/// Convert a perceptual volume (0.0..=1.0, what the slider shows) plus a dB
/// pre-amp into the linear gain playbin's `volume` property expects. Cubing
/// the perceptual value cancels PipeWire's cubic display mapping, exactly as
/// the inline engine does.
fn gst_volume(perceptual: f64, preamp_db: f64) -> f64 {
    let v = perceptual.clamp(0.0, 1.0);
    v * v * v * 10f64.powf(preamp_db / 20.0)
}

/// Emit one event line to the parent on stdout, flushing immediately so the
/// pipe isn't buffered.
fn emit(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Set an unsigned-integer GObject property to `value`, matching the property's
/// *actual* type. GStreamer's integer property widths vary across versions and
/// elements (e.g. playbin's `connection-speed` is `guint64` but the adaptive
/// demuxer's is `guint`), and `set_property` panics on a type mismatch — which,
/// in an `element-setup` callback on a streaming thread, aborts the whole
/// process (the window never appears). This reads the param spec and stores a
/// correctly-typed, clamped value; it's a no-op if the property is absent or an
/// unexpected type.
fn set_uint_property(obj: &gst::Element, name: &str, value: u64) {
    let Some(pspec) = obj.find_property(name) else {
        return;
    };
    let t = pspec.value_type();
    if t == glib::Type::U64 {
        obj.set_property(name, value);
    } else if t == glib::Type::U32 {
        obj.set_property(name, value.min(u32::MAX as u64) as u32);
    } else if t == glib::Type::I64 {
        obj.set_property(name, value.min(i64::MAX as u64) as i64);
    } else if t == glib::Type::I32 {
        obj.set_property(name, value.min(i32::MAX as u64) as i32);
    }
}

/// Resume a *paused* pipeline at `secs`: wait until the HLS stream is seekable,
/// seek, then start playing.
///
/// Caller must have already set the pipeline to `Paused` and ensured `secs`
/// is past the start. A seek issued before the stream is seekable is dropped,
/// and HLS isn't seekable until its media playlist is fetched/parsed (a network
/// round-trip after preroll). Seeking the prerolled, paused pipeline lands
/// reliably and avoids the play-from-0:00 flash + stutter. Logs to stderr (the
/// parent inherits it).
fn resume_at(playbin: &gst::Element, secs: f64) {
    let pb = playbin.clone();
    thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut waited_ms = 0u64;
        loop {
            let _ = pb.state(Some(gst::ClockTime::from_mseconds(250)));
            let dur = pb.query_duration::<gst::ClockTime>();
            if dur.is_some_and(|d| d.nseconds() > 0) {
                eprintln!(
                    "mare-video-window: seekable after {waited_ms}ms (duration={:?}s), seeking to {secs:.1}s",
                    dur.map(|d| d.seconds())
                );
                break;
            }
            if std::time::Instant::now() >= deadline {
                eprintln!("mare-video-window: not seekable after {waited_ms}ms; seeking anyway to {secs:.1}s");
                break;
            }
            thread::sleep(Duration::from_millis(150));
            waited_ms += 150;
        }
        let pos = gst::ClockTime::from_mseconds((secs.max(0.0) * 1000.0) as u64);
        let ok = pb.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, pos);
        // Let the flush-seek's preroll settle before we start rolling.
        let _ = pb.state(Some(gst::ClockTime::from_seconds(5)));
        let _ = pb.set_state(gst::State::Playing);
        eprintln!("mare-video-window: resumed playing at {secs:.1}s (seek_ok={})", ok.is_ok());
    });
}

fn main() {
    // args: <url> [position_secs] [volume 0..1] [preamp_db]
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_default();
    let position: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let volume: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let preamp_db: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_PREAMP_DB);

    if url.is_empty() {
        eprintln!("mare-video-window: no URL given");
        std::process::exit(2);
    }

    // GStreamer's windowing video sinks title their window after the GLib
    // program name (which defaults to this binary's basename). Set a friendly
    // name so the popped-out video window reads "Maré Player" instead of
    // "mare-video-window".
    glib::set_prgname(Some("Maré Player"));
    glib::set_application_name("Maré Player");

    if let Err(e) = gst::init() {
        eprintln!("mare-video-window: gstreamer init failed: {e}");
        std::process::exit(1);
    }

    // playbin/playbin3 driving an `autovideosink` (set explicitly below): the
    // sink (autovideosink -> a window-creating platform sink) opens and owns the
    // toplevel window for us. The sink renders in-process, so no decoded frames
    // cross the process boundary.
    let playbin = match gst::ElementFactory::make("playbin3").build() {
        Ok(p) => p,
        Err(_) => match gst::ElementFactory::make("playbin").build() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("mare-video-window: failed to create playbin: {e}");
                std::process::exit(1);
            }
        },
    };
    playbin.set_property("uri", &url);
    playbin.set_property("volume", gst_volume(volume, preamp_db));

    // Attach an explicit, persistent video sink. On a skip the parent sends a
    // `play` command, which cycles the pipeline through READY to swap the URI;
    // playbin keeps a *user-provided* sink across that transition, so its window
    // is reused. (Its auto-selected sink is instead rebuilt on a URI change,
    // which destroys the old window and opens a new one — the flicker we want to
    // avoid.) `autovideosink` still picks the right platform sink underneath.
    if let Ok(videosink) = gst::ElementFactory::make("autovideosink").build() {
        playbin.set_property("video-sink", &videosink);
    }

    // This is the sole-window player, shown full-size at native resolution
    // (the inline view downscales to a fixed width), so request the best the
    // stream offers. The HLS master manifest carries several resolution
    // variants; playbin's adaptive demuxer normally picks one from measured
    // bandwidth. Declaring a very high `connection-speed` (kbps) biases it to
    // the top (highest-resolution) variant instead of a smaller one.
    set_uint_property(&playbin, "connection-speed", 100_000_000);

    // Bias the *initial* variant high too. The windowing sink sizes its window
    // to the first decoded frame, and most sinks keep that size even after the
    // demuxer adapts up later — so if we start on a low variant the window
    // opens small and stays small. Setting the adaptive demuxer's
    // `start-bitrate` high makes it begin on the top (largest) variant, so the
    // window opens at full resolution. The demuxer only exists once playbin
    // builds it, hence `element-setup`. (playbin already propagates
    // `connection-speed` down to the demuxer, so we don't re-set it here.)
    playbin.connect("element-setup", false, |values| {
        let elem = values.get(1)?.get::<gst::Element>().ok()?;
        let fname = elem.factory().map(|f| f.name().to_string()).unwrap_or_default();
        if fname.contains("hlsdemux") || fname.contains("dashdemux") || fname.contains("adaptivedemux") {
            set_uint_property(&elem, "start-bitrate", 100_000_000);
            eprintln!("mare-video-window: biased {fname} initial variant high");
        }
        None
    });

    let Some(bus) = playbin.bus() else {
        eprintln!("mare-video-window: playbin has no bus");
        std::process::exit(1);
    };

    // Resume at `position` (pop-out hands us the inline position). Start paused
    // so nothing plays from 0:00 first: a fresh HLS pipeline isn't seekable
    // until its media playlist loads, and playing-then-seeking would flash the
    // start and stutter. `resume_at` waits, seeks, then plays. A fresh launch at
    // 0 just plays immediately.
    let start_paused = position > 0.5;
    let initial_state = if start_paused { gst::State::Paused } else { gst::State::Playing };
    if playbin.set_state(initial_state).is_err() {
        eprintln!("mare-video-window: failed to start playback");
        std::process::exit(1);
    }
    if start_paused {
        resume_at(&playbin, position);
    }

    let main_loop = glib::MainLoop::new(None, false);

    // Bus watch: end-of-stream and errors (a closed sink window surfaces as an
    // error) end this process; the parent reacts to `eos` / `closed`.
    let _bus_watch = {
        let main_loop = main_loop.clone();
        bus.add_watch(move |_bus, msg| {
            use gst::MessageView;
            match msg.view() {
                MessageView::Eos(_) => {
                    emit("eos");
                    main_loop.quit();
                }
                MessageView::Error(err) => {
                    eprintln!("mare-video-window: pipeline error: {} ({:?})", err.error(), err.debug());
                    // Most commonly this is the user closing the sink's window.
                    emit("closed");
                    main_loop.quit();
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        })
    };
    if _bus_watch.is_err() {
        eprintln!("mare-video-window: failed to add bus watch");
        std::process::exit(1);
    }

    // Report position to the parent ~2 Hz so its slider/now-playing stays fresh.
    {
        let pb = playbin.clone();
        glib::timeout_add(Duration::from_millis(500), move || {
            if let Some(t) = pb.query_position::<gst::ClockTime>() {
                emit(&format!("position {:.3}", t.mseconds() as f64 / 1000.0));
            }
            glib::ControlFlow::Continue
        });
    }

    // Read commands from the parent on stdin. gst element ops (property/state/
    // seek) are thread-safe, so we apply them directly here.
    {
        let pb = playbin.clone();
        let main_loop = main_loop.clone();
        thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                let line = line.trim();
                let (cmd, rest) = match line.split_once(' ') {
                    Some((c, r)) => (c, r.trim()),
                    None => (line, ""),
                };
                match cmd {
                    "volume" => {
                        if let Ok(v) = rest.parse::<f64>() {
                            pb.set_property("volume", gst_volume(v, preamp_db));
                        }
                    }
                    "seek" => {
                        if let Ok(s) = rest.parse::<f64>() {
                            let pos = gst::ClockTime::from_mseconds((s.max(0.0) * 1000.0) as u64);
                            let _ = pb.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, pos);
                        }
                    }
                    "pause" => {
                        let _ = pb.set_state(gst::State::Paused);
                    }
                    "resume" => {
                        let _ = pb.set_state(gst::State::Playing);
                    }
                    "play" => {
                        // play <position_secs> <url>
                        let (pos, new_url) = match rest.split_once(' ') {
                            Some((p, u)) => (p.parse::<f64>().unwrap_or(0.0), u.trim()),
                            None => (0.0, rest),
                        };
                        if !new_url.is_empty() {
                            let _ = pb.set_state(gst::State::Ready);
                            pb.set_property("uri", new_url);
                            if pos > 0.5 {
                                // Resume at a position: paused-first (see startup).
                                let _ = pb.set_state(gst::State::Paused);
                                resume_at(&pb, pos);
                            } else {
                                let _ = pb.set_state(gst::State::Playing);
                            }
                        }
                    }
                    "quit" => {
                        main_loop.quit();
                        return;
                    }
                    _ => {}
                }
            }
            // stdin closed → the parent is gone → exit.
            main_loop.quit();
        });
    }

    main_loop.run();
    let _ = playbin.set_state(gst::State::Null);
}
