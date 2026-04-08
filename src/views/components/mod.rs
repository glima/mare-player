// SPDX-License-Identifier: MIT

//! Reusable UI components and constants for Maré Player views.
//!
//! Every clickable row in the applet — tracks, albums, playlists, menu entries,
//! search results, discography items, inline text links — is built from a small
//! set of composable helpers defined across these sub-modules:
//!
//! - [`constants`] — shared sizing constants and volume helpers
//! - [`icons`] — inline SVG data and icon-handle helpers
//! - [`fading_clip`] — the custom [`FadingClip`](fading_clip::FadingClip) widget
//! - [`list_helpers`] — list-item wrappers, fading text helpers, `TrackRowOptions`
//! - [`rows`] — domain-specific row builders on `AppModel`
//!
//! # Component hierarchy
//!
//! ```text
//! list_item(content, on_press, padding)     ← single source of truth for pill shape
//!   │
//!   ├── track_row(track, index, opts)       ← thumbnail + title + artist + duration + ♥
//!   │
//!   ├── album_row(album)                    ← thumbnail + title + artist
//!   │
//!   ├── playlist_row(playlist)              ← thumbnail + title + track count
//!   │
//!   ├── menu_row(icon, label, on_press)     ← icon + label + chevron
//!   │
//!   └── (direct calls)                      ← artist discography, inline text links
//! ```
//!
//! All public items are re-exported at this level so existing
//! `use crate::views::components::{…}` imports continue to work unchanged.

mod fading_clip;

pub mod constants;
pub mod icons;
pub mod list_helpers;
pub mod rows;

// ---- Re-exports (preserves the original flat public API) --------------------

pub use constants::{
    ALBUM_COVER_SIZE, ARTIST_PICTURE_SIZE, MAX_POPUP_HEIGHT, NOW_PLAYING_ART_SIZE, PANEL_ART_SIZE,
    THUMBNAIL_SIZE, VOLUME_BAR_WIDTH, VOLUME_STEP, scroll_to_volume_delta,
};

pub use icons::{RADIO_SVG, favorite_icon_handle};

pub use list_helpers::{
    TrackRowOptions, app_icon_element, branded_text, branded_title, fading_card_column,
    fading_header_title, fading_panel_text, fading_standard_text, fading_suggested_text,
    fading_text_column, list_item, scrollable_element, scrollable_list,
};

pub use rows::{build_thumbnail, build_track_row};
