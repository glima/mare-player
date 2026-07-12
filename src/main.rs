// SPDX-License-Identifier: MIT

//! Maré Player — a panel applet for the COSMIC™ desktop that streams
//! music from the TIDAL service.
//!
//! The applet provides library browsing, search, Hi-Res audio playback
//! (symphonia + PulseAudio), a real-time spectrum visualizer, MPRIS2 media
//! control, and local disk caching — all integrated into the COSMIC panel.

#[cfg(not(feature = "panel-applet"))]
use cosmic::iced::core::layout::Limits;

// On-demand profiling (debug builds only, Linux).
//   cargo build              → profiler is embedded automatically
//   kill -USR1 <pid>         → samples for 10 s
//   open /tmp/mare-flamegraph.svg
#[cfg(all(debug_assertions, target_os = "linux"))]
use pprof::ProfilerGuard;
#[cfg(all(debug_assertions, target_os = "linux"))]
use signal_hook::{consts::SIGUSR1, iterator::Signals};
#[cfg(all(debug_assertions, target_os = "linux"))]
use std::sync::atomic::{AtomicBool, Ordering};

use cosmic_applet_mare::i18n;

use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Start a background thread that waits for SIGUSR1, then samples for 10 s
/// and writes a flamegraph SVG to `/tmp/mare-flamegraph.svg`.
#[cfg(all(debug_assertions, target_os = "linux"))]
fn start_pprof_profiler() -> std::sync::Arc<AtomicBool> {
    use std::fs::File;

    let running = std::sync::Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    std::thread::spawn(move || {
        let Ok(mut signals) = Signals::new([SIGUSR1]) else {
            tracing::error!("Failed to register SIGUSR1 handler");
            return;
        };
        for _ in signals.forever() {
            if !running_clone.load(Ordering::SeqCst) {
                break;
            }
            tracing::info!("SIGUSR1 received: starting 10 s pprof profile …");
            // 100 Hz sampling rate
            let guard = match ProfilerGuard::new(100) {
                Ok(g) => g,
                Err(e) => {
                    tracing::warn!("Failed to start pprof profiler: {e}");
                    continue;
                }
            };
            std::thread::sleep(std::time::Duration::from_secs(10));
            match guard.report().build() {
                Ok(report) => {
                    let path = "/tmp/mare-flamegraph.svg";
                    match File::create(path) {
                        Ok(mut file) => {
                            if let Err(e) = report.flamegraph(&mut file) {
                                tracing::warn!("Failed to write flamegraph: {e}");
                            } else {
                                tracing::info!("Flamegraph written to {path}");
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to create flamegraph file at {path}: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to build pprof report: {e}");
                }
            }
        }
    });

    running
}

fn main() -> cosmic::iced::Result {
    // Initialize tracing with filters to reduce noise
    // Filter out noisy warnings from iced_futures subscription tracker
    let mut filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // Add directives, ignoring any that fail to parse
    let directives = [
        "iced_futures::subscription::tracker=error",
        "iced_winit=warn",
        "sctk=warn",
        "sctk_adwaita=error",
        "hyper=warn",
        "hyper_util=warn",
        "i18n_embed=warn",
        "cosmic::app=warn",
        "cosmic_config=warn",
        "winit=warn",
        "rustls_platform_verifier=warn",
        "reqwest=warn",
        "cosmic_text=warn",
        "wgpu_core=warn",
        "wgpu_hal=warn",
        // iced_wgpu logs per-frame texture-atlas allocations at DEBUG, which
        // floods the always-DEBUG file log (multi-GB per session). Silence it.
        "iced_wgpu=warn",
        // turso (embedded DB) logs every page/WAL/btree access at DEBUG, which
        // buries the app's own logs whenever the console level is DEBUG/TRACE.
        "turso_core=warn",
    ];

    for directive in directives {
        if let Ok(parsed) = directive.parse() {
            filter = filter.add_directive(parsed);
        }
    }

    // Use local timezone for all log timestamps
    let local_time = ChronoLocal::rfc_3339();

    // Console layer: uses the env filter above, wrapped in a reload layer so
    // the Settings view can change verbosity live (see
    // `logging::set_console_level`). Writes to **stderr** so that when the
    // applet is spawned by cosmic-panel (which forwards a child's stderr to the
    // journal but not its stdout), these logs reach `journalctl`.
    let (console_filter, console_reload) = tracing_subscriber::reload::Layer::new(filter);
    let console_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_timer(local_time)
        .with_filter(console_filter);

    tracing_subscriber::registry().with(console_layer).init();

    // Install the runtime console-level reload hook now that the subscriber is
    // live. It rebuilds the same base-level + noise-directive filter used at
    // startup, so a level change from Settings matches how `main` builds it.
    cosmic_applet_mare::logging::install_reload_hook(Box::new(move |level| {
        let mut filter = EnvFilter::new(level.as_filter_str());
        for directive in directives {
            if let Ok(parsed) = directive.parse() {
                filter = filter.add_directive(parsed);
            }
        }
        let _ = console_reload.reload(filter);
    }));

    // Start the on-demand pprof profiler (debug builds only, no-op in release).
    // Send SIGUSR1 to the process to capture a 10 s flamegraph.
    #[cfg(all(debug_assertions, target_os = "linux"))]
    let _pprof_running = start_pprof_profiler();

    // Get the system's preferred languages.
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();

    // Enable localizations to be applied.
    i18n::init(&requested_languages);

    // Start the event loop — either as a panel applet or a standalone window.
    #[cfg(feature = "panel-applet")]
    let result = cosmic::applet::run::<cosmic_applet_mare::app::AppModel>(());
    // Standalone window mode — enforce a minimum size so the now-playing
    // bar (≈140 px) is always fully visible when music is active.
    //   min 360 × 480  →  header + ≈3 track rows + full now-playing bar
    #[cfg(not(feature = "panel-applet"))]
    let result = cosmic::app::run::<cosmic_applet_mare::app::AppModel>(
        cosmic::app::Settings::default()
            .size(cosmic::iced::Size::new(420.0, 680.0))
            .size_limits(Limits::NONE.min_width(360.0).min_height(480.0))
            .exit_on_close(true),
        (),
    );

    result
}
