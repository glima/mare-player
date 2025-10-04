// SPDX-License-Identifier: MIT

//! SVG icon data and icon-handle helpers for Maré Player.
//!
//! Centralises all inline SVG definitions and the small helper functions that
//! turn them into [`cosmic::widget::icon::Handle`] values.  Keeping these
//! separate from the layout code makes it easier to find, replace, or add
//! icons without wading through widget plumbing.

use cosmic::widget::icon;

// =============================================================================
// Radio Icon
// =============================================================================

/// Radio icon SVG for the "go to track radio" button.
///
/// A classic portable radio silhouette (antenna, speaker circle, display)
/// designed for 16×16 symbolic use. Stroke-based so it recolours with the theme.
pub const RADIO_SVG: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
<path d="M2 5.5h12a1.5 1.5 0 0 1 1.5 1.5v6a1.5 1.5 0 0 1-1.5 1.5H2A1.5 1.5 0 0 1 .5 13V7A1.5 1.5 0 0 1 2 5.5Z" stroke="#232323" stroke-width="1.2" fill="none"/>
<line x1="4" y1="5.5" x2="12" y2="1.5" stroke="#232323" stroke-width="1.2" stroke-linecap="round"/>
<circle cx="5.5" cy="10" r="2.25" stroke="#232323" stroke-width="1.1" fill="none"/>
<rect x="9.5" y="7.75" width="4" height="1.75" rx="0.5" stroke="#232323" stroke-width="0.9" fill="none"/>
<circle cx="10.25" cy="12" r="0.65" fill="#232323"/>
<circle cx="12" cy="12" r="0.65" fill="#232323"/>
<circle cx="13.75" cy="12" r="0.65" fill="#232323"/>
</svg>"##;

// =============================================================================
// Favorite (Heart) Icon
// =============================================================================

/// Outline heart SVG for the "not favorited" state.
///
/// This is a symbolic icon (uses `#232323` fill) so the COSMIC theme engine
/// will recolor it to match the current foreground colour — just like every
/// other `-symbolic` icon shipped with the Cosmic icon theme.
///
/// The path is derived from the filled `emblem-favorite-symbolic` that ships
/// with the Cosmic icon set, converted to a 1.5 px stroke outline.
const HEART_OUTLINE_SVG: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
<path d="M4.78 2C2.698 2 1 3.675 1 5.75c0 1.08.456 2.065 1.187 2.75L7.906 14l5.905-5.5A5.735 5.735 0 0 0 15 5.75C15 3.675 13.3 2 11.219 2c-1.372 0-2.56.721-3.22 1.813A4.756 4.756 0 0 0 4.78 2Z" stroke="#232323" stroke-width="1.5" fill="none"/>
</svg>"##;

/// Return the correct icon [`Handle`](cosmic::widget::icon::Handle) for a favorite toggle button.
///
/// * **favorited** → themed `emblem-favorite-symbolic` (filled heart)
/// * **not favorited** → bundled outline-heart SVG (stroke only)
pub fn favorite_icon_handle(is_favorite: bool) -> icon::Handle {
    if is_favorite {
        icon::from_name("emblem-favorite-symbolic").into()
    } else {
        let mut h = icon::from_svg_bytes(HEART_OUTLINE_SVG);
        h.symbolic = true;
        h
    }
}
