// SPDX-License-Identifier: MIT

//! View rendering modules for Maré Player.
//!
//! This module contains all the UI rendering code, split by screen/component:
//!
//! - `panel` - Panel button view (shown in the system panel)
//! - `popup` - Main popup window structure and now-playing bar
//! - `main` - Main collection view with categories
//! - `mixes` - Mixes & Radio list and detail views
//! - `track_radio` - Track radio view (similar tracks based on a seed track)
//! - `track_detail` - Track detail view (recommendations seeded from a track)
//! - `playlists` - Playlist list and detail views
//! - `albums` - Album list and detail views
//! - `tracks` - Favorite tracks view
//! - `profiles` - Followed artists (profiles) view
//! - `search` - Search view
//! - `settings` - Settings view
//! - `auth` - Login and OAuth waiting views
//! - `share` - Share prompt dialog
//! - `components` - Reusable UI components (thumbnail, track row, header, etc.)
//! - `visualizer` - Audio spectrum visualizer widget

pub mod albums;
pub mod artist;
pub mod auth;
pub mod components;
pub mod history;
pub mod main;
pub mod mixes;
#[cfg(feature = "panel-applet")]
pub mod panel;
pub mod playlists;
pub mod popup;
pub mod profiles;
pub mod search;
pub mod settings;
pub mod share;
pub mod track_detail;
pub mod track_radio;
pub mod tracks;
pub mod visualizer;
