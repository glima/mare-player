// SPDX-License-Identifier: GPL-3.0-only

//! Maré Player library crate.
//!
//! Re-exports all internal modules so that integration tests (under `tests/`)
//! can exercise the public API without needing to be inside the binary crate.

pub mod app;
pub mod audio;
pub mod cache;
pub mod config;
pub mod handlers;
pub mod helpers;
pub mod i18n;
pub mod image_cache;
pub mod logging;
#[cfg(not(feature = "panel-applet"))]
pub mod menu;
pub mod messages;
pub mod playback;
pub mod state;
pub mod tidal;
pub mod views;
