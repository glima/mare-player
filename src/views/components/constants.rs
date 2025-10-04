// SPDX-License-Identifier: MIT

//! Shared constants and small utility functions for Maré Player views.
//!
//! Gathers all magic numbers (sizes, thresholds, steps) and the tiny helpers
//! that only depend on them into one place so they are easy to find and tune
//! without touching layout or widget code.

use cosmic::iced::mouse::ScrollDelta;

// =============================================================================
// Size Constants
// =============================================================================

/// Size for album art thumbnails in list views.
pub const THUMBNAIL_SIZE: u16 = 40;

/// Size for album art in the now-playing bar.
pub const NOW_PLAYING_ART_SIZE: u16 = 56;

/// Size for album art in the panel button.
pub const PANEL_ART_SIZE: u16 = 20;

/// Maximum popup height (panel-applet mode only).
pub const MAX_POPUP_HEIGHT: f32 = 500.0;

/// Size for the large album cover at the top of the album detail view.
pub const ALBUM_COVER_SIZE: u16 = 96;

/// Size for the large artist picture at the top of the artist detail view.
pub const ARTIST_PICTURE_SIZE: u16 = 96;

// =============================================================================
// Panel Constants
// =============================================================================

/// Width of the volume bar in the panel applet (pixels).
pub const VOLUME_BAR_WIDTH: f32 = 4.0;

// =============================================================================
// Volume Helpers
// =============================================================================

/// Volume change per mouse-wheel scroll step (5% per line).
///
/// Used by both the panel button (scroll on icon) and the standalone
/// now-playing bar (scroll on volume icon) to keep the feel consistent.
pub const VOLUME_STEP: f32 = 0.05;

/// Convert a mouse-wheel [`ScrollDelta`] into a signed volume change
/// suitable for [`Message::AdjustVolume`](crate::messages::Message::AdjustVolume).
///
/// Line-based scrolling (most mice) maps one line to [`VOLUME_STEP`].
/// Pixel-based scrolling (trackpads) is normalised so that ~15 px of
/// movement equals one step.
pub fn scroll_to_volume_delta(delta: ScrollDelta) -> f32 {
    match delta {
        ScrollDelta::Lines { y, .. } => y * VOLUME_STEP,
        ScrollDelta::Pixels { y, .. } => (y / 15.0) * VOLUME_STEP,
    }
}

// =============================================================================
// Cache Directory
// =============================================================================

/// XDG cache subdirectory name, matching the application identity.
///
/// Resolves to `$XDG_CACHE_HOME/<CACHE_DIR_NAME>/` at runtime
/// (e.g. `~/.cache/cosmic-applet-mare/` or `~/.cache/mare-player/`).
#[cfg(feature = "panel-applet")]
pub const CACHE_DIR_NAME: &str = "cosmic-applet-mare";
#[cfg(not(feature = "panel-applet"))]
pub const CACHE_DIR_NAME: &str = "mare-player";
