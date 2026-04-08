// SPDX-License-Identifier: MIT

//! Maré Player — a panel applet for the COSMIC™ desktop that streams
//! music from the TIDAL service.
//!
//! The applet provides library browsing, search, Hi-Res audio playback
//! (symphonia + PulseAudio), a real-time spectrum visualizer, MPRIS2 media
//! control, and local disk caching — all integrated into the COSMIC panel.

#[cfg(not(feature = "panel-applet"))]
use cosmic::iced::core::layout::Limits;
use std::fs::OpenOptions;
use std::sync::Mutex;

use cosmic_applet_mare::disk_cache;
use cosmic_applet_mare::i18n;

use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Maximum log file size in bytes (5 MB).
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

fn main() -> cosmic::iced::Result {
    // Resolve log path under XDG cache dir (~/.cache/cosmic-applet-mare/logs/)
    let log_file_path = disk_cache::log_file_path("cosmic-applet-mare.log");

    // Trim the log file to 5 MB before we open it for appending, so it
    // never grows unboundedly across restarts.
    disk_cache::trim_log_file(&log_file_path, LOG_MAX_BYTES);

    // Initialize tracing with filters to reduce noise
    // Filter out noisy warnings from iced_futures subscription tracker
    let mut filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // Add directives, ignoring any that fail to parse
    let directives = [
        "iced_futures::subscription::tracker=error",
        "iced_winit=warn",
        "sctk=warn",
        "sctk_adwaita=error",
        "h2=error",
        "hyper=warn",
        "hyper_util=warn",
        "i18n_embed=warn",
        "cosmic::app=warn",
        "cosmic_config=warn",
        "winit=warn",
        "rustls_platform_verifier=warn",
        "reqwest=warn",
        "symphonia=warn",
        "cosmic_text=warn",
        "wgpu_core=warn",
        "wgpu_hal=warn",
    ];

    for directive in directives {
        if let Ok(parsed) = directive.parse() {
            filter = filter.add_directive(parsed);
        }
    }

    // Use local timezone for all log timestamps
    let local_time = ChronoLocal::rfc_3339();

    // Console layer: uses the env filter above
    let console_layer = fmt::layer()
        .with_timer(local_time.clone())
        .with_filter(filter);

    // File layer: always DEBUG level, appending to a persistent log file under
    // $XDG_CACHE_HOME/cosmic-applet-mare/logs/ so we can retrieve logs after the fact.
    let file_result = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path);

    let file_opened_ok = file_result.is_ok();

    let file_layer = file_result.ok().map(|file| {
        let mut file_filter = EnvFilter::new("debug");
        for directive in &directives {
            if let Ok(parsed) = directive.parse() {
                file_filter = file_filter.add_directive(parsed);
            }
        }
        fmt::layer()
            .with_ansi(false)
            .with_timer(local_time)
            .with_writer(Mutex::new(file))
            .with_filter(file_filter)
    });

    tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer)
        .init();

    if file_opened_ok {
        tracing::info!(
            "File logging enabled at DEBUG level: {}",
            log_file_path.display()
        );
    } else {
        tracing::warn!(
            "Failed to open log file at {}, file logging disabled",
            log_file_path.display()
        );
    }

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

    // Trim the log file again on shutdown so we don't leave a bloated file
    // behind after a long-running session.
    disk_cache::trim_log_file(&log_file_path, LOG_MAX_BYTES);

    result
}
