// SPDX-License-Identifier: MIT

//! List-item wrappers, fading text helpers, and track-row configuration.
//!
//! This module contains the composable helpers that sit between the low-level
//! [`FadingClip`] widget and the high-level
//! row builders in [`super::rows`].  Everything that *constructs* a reusable
//! list element lives here; everything that *fills* one with domain data
//! (tracks, albums, playlists) lives in `rows`.

use cosmic::Element;
use cosmic::iced::widget::scrollable;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, container, text};

use cosmic::widget::button::Catalog;

use crate::messages::Message;
use std::sync::Arc;

use crate::tidal::models::Track;

use super::fading_clip::FadingClip;

/// Width (in pixels) of the gradient fade overlay on text columns.
const FADE_WIDTH: f32 = 32.0;

/// Maximum width (in pixels) for the text portion of the panel button.
const MAX_PANEL_TEXT_WIDTH: f32 = 300.0;

// =============================================================================
// TrackRowOptions
// =============================================================================

/// Options for rendering a track row via [`AppModel::track_row`](crate::state::AppModel::track_row).
///
/// Use [`Default::default()`] for sensible defaults, then override as needed.
pub struct TrackRowOptions {
    /// The full track list for queue context when clicked.
    pub tracks: Arc<[Track]>,
    /// Container source for the track list (album / playlist / mix / etc.).
    /// Threaded into `PlayTrackList` so TIDAL play attribution and the
    /// now-playing bar label both see the right container.
    pub source: Option<crate::tidal::models::PlaybackSource>,
    /// Fallback icon name when cover art is not cached.
    pub fallback_icon: &'static str,
    /// Whether to show the "Go to track radio" button. Default `true`.
    /// Set to `false` in the track radio view to prevent recursive radios.
    pub show_radio_button: bool,
}

impl Default for TrackRowOptions {
    fn default() -> Self {
        Self { tracks: Arc::from([]), source: None, fallback_icon: "audio-x-generic-symbolic", show_radio_button: true }
    }
}

impl TrackRowOptions {
    /// Compute the fixed width for the duration column based on the longest
    /// duration string in the current track list.
    ///
    /// Digits and colons are measured separately: digits are ~6 px wide at
    /// size 11 in a typical COSMIC font, while colons are only ~3 px.
    pub fn duration_column_width(&self) -> f32 {
        let max_str =
            self.tracks.iter().map(|t| t.duration_display()).max_by_key(|s| s.len()).unwrap_or_else(|| "0:00".to_string());

        let digits = max_str.chars().filter(|c| c.is_ascii_digit()).count();
        let colons = max_str.chars().filter(|c| *c == ':').count();

        digits as f32 * 6.0 + colons as f32 * 3.0 + 1.0
    }
}

// =============================================================================
// List Item Wrapper
// =============================================================================

/// Wrap any content in a standard pill-shaped list item button.
///
/// This is the **single source of truth** for list item styling across the
/// entire applet. Every clickable row in a list (tracks, albums, playlists,
/// menu entries, search results, discography items) should go through here.
pub fn list_item<'a>(content: impl Into<Element<'a, Message>>, on_press: Message, padding: u16) -> Element<'a, Message> {
    // Use a custom button class that delegates to MenuItem for every state
    // except pressed, which reuses the hovered style.  This ensures the
    // FadingClip gradient (which uses the hover colour) matches the button
    // background in ALL interactive states — base, hover, AND press —
    // without needing fragile pressed-state tracking in the widget tree.
    let class = cosmic::theme::Button::Custom {
        active: Box::new(|focused, theme| Catalog::active(theme, focused, false, &cosmic::theme::Button::MenuItem)),
        disabled: Box::new(|theme| Catalog::disabled(theme, &cosmic::theme::Button::MenuItem)),
        hovered: Box::new(|focused, theme| Catalog::hovered(theme, focused, false, &cosmic::theme::Button::MenuItem)),
        pressed: Box::new(|focused, theme| Catalog::hovered(theme, focused, false, &cosmic::theme::Button::MenuItem)),
    };

    button::custom(content).on_press(on_press).width(Length::Fill).padding(padding).class(class).into()
}

/// Bake an inter-row gap into a virtual-`List` row as non-interactive bottom
/// padding, instead of using `List::spacing()`.
///
/// **Why this exists:** the virtual `List` widget (iced/libcosmic) is buggy.
/// Its event pass (`update`) positions each visible child at
/// `offset + spacing * index`, while `draw`, `mouse_interaction` and `operate`
/// position them at just `offset` (the per-row layout already bakes in
/// `index * spacing`).  So with `spacing > 0` the click hit-boxes drift
/// *downward* by `index * spacing` pixels relative to what's painted, and you
/// end up activating a row *above* the one you clicked — the error growing the
/// further you scroll.  (Observed: clicking "1950s" in Explore loaded "Focus".)
///
/// We therefore keep `List` spacing at 0 (where `update`/`draw` agree exactly)
/// and reproduce the visual gap here as bottom padding *outside* the
/// interactive button, so the gap stays dead space and hit-testing matches
/// drawing. Remove this and restore `List::spacing()` once the widget is fixed
/// upstream.
pub fn virtual_list_row<'a>(content: impl Into<Element<'a, Message>>, gap: u16) -> Element<'a, Message> {
    container(content).width(Length::Fill).padding([0, 0, gap, 0]).into()
}

// =============================================================================
// Header Back Button
// =============================================================================

/// Build the header back button used by every non-root view.
///
/// Left-click sends `on_press` — whatever "back" means for that view (pop the
/// nav stack, pop the explore stack, cancel a prompt, or go straight home).
/// **Right-click always jumps to the main collection view**, collapsing the
/// whole stack in one gesture instead of hopping up one view at a time; deep
/// chains (search → artist → album → track → credits) otherwise take five
/// clicks to escape.
///
/// The right-click lives on a [`mouse_area`](widget::mouse_area) wrapper rather
/// than the button itself, because iced buttons only handle the left button.
/// `on_right_release` (not `on_right_press`) keeps it abortable by moving off
/// the button before letting go, and matches the panel button's existing
/// right-click gesture.
pub fn back_button<'a>(on_press: Message) -> Element<'a, Message> {
    widget::mouse_area(
        button::icon(widget::icon::from_name("go-previous-symbolic"))
            .on_press(on_press)
            .tooltip(crate::fl!("tooltip-back"))
            .padding(4),
    )
    .on_right_release(Message::ShowMain)
    .into()
}

// =============================================================================
// Fading Text Helpers
// =============================================================================

/// Create a text column that alpha-fades overflow on its right edge.
///
/// Uses [`FadingClip`] to GPU-clip overflowing text and alpha-fade its right
/// edge to transparency, so long labels dissolve cleanly regardless of the
/// (possibly translucent) background behind them.
pub fn fading_text_column<'a>(children: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let text_col = widget::Column::with_children(children).width(Length::Fill);

    FadingClip::new(text_col, FADE_WIDTH).width(Length::Fill).into()
}

/// Wrap a single element in a **shrink-width** [`FadingClip`], for text placed
/// *inside* a clickable button.
///
/// The alpha-ramp fade works by re-drawing the text with a reduced-alpha
/// `text_color`, which only takes effect if the text inherits the ambient
/// colour. A `button` sets its *own* `text_color`, so an outer fade around the
/// button is overridden (you get a hard clip). Placing the fade **inside** the
/// button instead ramps the colour the button already applied — so clickable
/// labels fade correctly and keep their hover highlight.
///
/// Shrink width keeps short labels hugging their content (so the button's
/// hover highlight hugs them too); a label long enough to overflow fills the
/// available width and fades at the clipped edge.
pub fn fading_text<'a>(child: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    FadingClip::new(child, FADE_WIDTH).into()
}

/// Wrap text in a [`FadingClip`] that alpha-fades overflow. For text inside
/// suggested (accent) action buttons.
pub fn fading_suggested_text<'a>(child: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    FadingClip::new(child, FADE_WIDTH).width(Length::Fill).into()
}

/// Wrap text in a [`FadingClip`] that alpha-fades overflow. For text inside
/// standard action buttons.
pub fn fading_standard_text<'a>(child: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    FadingClip::new(child, FADE_WIDTH).width(Length::Fill).into()
}

/// Wrap panel button text in a width-limited [`FadingClip`] that alpha-fades
/// overflow on the right.
pub fn fading_panel_text<'a>(child: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(FadingClip::new(child, FADE_WIDTH).width(Length::Shrink)).max_width(MAX_PANEL_TEXT_WIDTH).into()
}

/// Wrap a header title in a [`FadingClip`] that alpha-fades overflow on the
/// right. For titles sitting directly on the popup background.
pub fn fading_header_title<'a>(title: &str) -> Element<'a, Message> {
    let label = text(title.to_string()).size(18).wrapping(Wrapping::None);

    FadingClip::new(label, FADE_WIDTH).width(Length::Fill).into()
}

// =============================================================================
// Branded Title
// =============================================================================

/// Build the branded "MARÉ / Player" title block.
///
/// `bottom_size` is the font size for "Player" — the same value the caller
/// was already using (18 in the header row, 24 in the login view).
/// "MARÉ" is rendered at ⅓ that size, horizontally centred on "Player",
/// and placed immediately above it with zero spacing so that Player's own
/// baseline stays in exactly the same place it would occupy if it were the
/// only child of the column.
/// The raw SVG bytes for the app icon (`resources/icon.svg`).
///
/// Exposed so callers (e.g. the main-view header) can build their own layout
/// with [`app_icon_element`] without going through [`branded_title`].
pub static APP_ICON_SVG: &[u8] = include_bytes!("../../../resources/icon.svg");

/// Build just the two-line "MARÉ / PLAYER" text column.
///
/// `big_size` controls the large "MARÉ" line; the small "PLAYER" line is
/// rendered at ⅓ that size (minimum 1).
pub fn branded_text<'a>(big_size: u16) -> Element<'a, Message> {
    let small_size = (big_size / 3).max(1);
    widget::Row::new()
        .push(text("MARÉ").size(big_size))
        .push(text("PLAYER").size(small_size))
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
}

/// Build an app-icon element at the given pixel size.
pub fn app_icon_element<'a>(size: u16) -> Element<'a, Message> {
    let handle = widget::icon::from_svg_bytes(APP_ICON_SVG);
    widget::icon(handle).size(size).into()
}

/// Convenience: text + icon side-by-side (used by the login view).
///
/// `big_size` is the font size for the large "MARÉ" line. The icon is sized to
/// match the total text-block height (big + small lines).
pub fn branded_title<'a>(big_size: u16) -> Element<'a, Message> {
    let small_size = (big_size / 3).max(1);
    let icon_size = big_size + small_size;
    let gap = big_size / 2;

    widget::Row::new()
        .push(branded_text(big_size))
        .push(app_icon_element(icon_size))
        .spacing(gap)
        .align_y(Alignment::Center)
        .into()
}

// =============================================================================
// Scrollable List
// =============================================================================

/// Wrap a content column in a scrollable container that fills available space
/// in standalone mode, or caps at [`MAX_POPUP_HEIGHT`](super::constants::MAX_POPUP_HEIGHT)
/// in panel-applet mode.
pub fn scrollable_list(content: widget::Column<'_, Message, cosmic::Theme>) -> Element<'_, Message> {
    #[cfg(feature = "panel-applet")]
    {
        use super::constants::MAX_POPUP_HEIGHT;
        container(scrollable(content.padding([0, 12, 0, 0])).height(Length::Shrink)).max_height(MAX_POPUP_HEIGHT).into()
    }
    #[cfg(not(feature = "panel-applet"))]
    {
        scrollable(content.padding([0, 12, 0, 0])).height(Length::Fill).into()
    }
}

/// Wrap any widget element in a scrollable container.
///
/// This is the generic counterpart to [`scrollable_list`] — it accepts an
/// already-built [`Element`] (e.g. a virtual `List` converted to Element)
/// and wraps it in the same scrollable + padding + height-capping structure.
pub fn scrollable_element<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    let elem = content.into();
    #[cfg(feature = "panel-applet")]
    {
        use super::constants::MAX_POPUP_HEIGHT;
        container(scrollable(container(elem).width(Length::Fill).padding([0, 12, 0, 0])).height(Length::Shrink))
            .max_height(MAX_POPUP_HEIGHT)
            .into()
    }
    #[cfg(not(feature = "panel-applet"))]
    {
        scrollable(container(elem).width(Length::Fill).padding([0, 12, 0, 0])).height(Length::Fill).into()
    }
}
