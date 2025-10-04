// SPDX-License-Identifier: MIT

//! Maré Player library crate.
//!
//! Re-exports all internal modules so that integration tests (under `tests/`)
//! can exercise the public API without needing to be inside the binary crate.

pub mod app;
pub mod audio;
pub mod config;
pub mod disk_cache;
pub mod handlers;
pub mod helpers;
pub mod i18n;
pub mod image_cache;
#[cfg(not(feature = "panel-applet"))]
pub mod menu;
pub mod messages;
pub mod state;
pub mod tidal;
pub mod views;
