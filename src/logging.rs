// SPDX-License-Identifier: GPL-3.0-only

//! Runtime-adjustable console log level.
//!
//! `main` builds the tracing subscriber with the console `EnvFilter` wrapped in
//! a [`tracing_subscriber::reload`] layer, then installs a reload hook here via
//! [`install_reload_hook`]. The Settings view calls [`set_console_level`] to
//! change verbosity **live**, without a restart. The on-disk debug log is a
//! separate layer and is unaffected.

use std::sync::OnceLock;

use crate::config::LogLevel;

/// Rebuilds the console `EnvFilter` for a given level and swaps it into the
/// live subscriber. Installed once by `main`.
type ReloadHook = Box<dyn Fn(LogLevel) + Send + Sync>;

static RELOAD: OnceLock<ReloadHook> = OnceLock::new();

/// Register the console-filter reload hook. Called once from `main` after the
/// tracing subscriber is initialized. Subsequent calls are ignored.
pub fn install_reload_hook(hook: ReloadHook) {
    let _ = RELOAD.set(hook);
}

/// Apply a new console log level at runtime.
///
/// No-op if the hook hasn't been installed (e.g. in integration tests that
/// never initialize tracing), so callers don't need to special-case that.
pub fn set_console_level(level: LogLevel) {
    if let Some(hook) = RELOAD.get() {
        hook(level);
    }
}
