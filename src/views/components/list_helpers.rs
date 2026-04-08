// SPDX-License-Identifier: MIT

//! List-item wrappers, fading text helpers, and track-row configuration.
//!
//! This module contains the composable helpers that sit between the low-level
//! [`FadingClip`](super::fading_clip::FadingClip) widget and the high-level
//! row builders in [`super::rows`].  Everything that *constructs* a reusable
//! list element lives here; everything that *fills* one with domain data
//! (tracks, albums, playlists) lives in `rows`.

use cosmic::Element;
use cosmic::iced::widget::scrollable;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, container, text};

use crate::messages::Message;
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
pub struct TrackRowOptions<'a> {
    /// The full track list for queue context when clicked.
    pub tracks: &'a [Track],
    /// Optional playback context label (e.g. album/playlist name).
    pub context: Option<String>,
    /// Fallback icon name when cover art is not cached.
    pub fallback_icon: &'static str,
    /// Whether to show the "Go to track radio" button. Default `true`.
    /// Set to `false` in the track radio view to prevent recursive radios.
    pub show_radio_button: bool,
}

impl<'a> Default for TrackRowOptions<'a> {
    fn default() -> Self {
        Self {
            tracks: &[],
            context: None,
            fallback_icon: "audio-x-generic-symbolic",
            show_radio_button: true,
        }
    }
}

impl TrackRowOptions<'_> {
    /// Compute the fixed width for the duration column based on the longest
    /// duration string in the current track list.
    ///
    /// Digits and colons are measured separately: digits are ~6 px wide at
    /// size 11 in a typical COSMIC font, while colons are only ~3 px.
    pub fn duration_column_width(&self) -> f32 {
        let max_str = self
            .tracks
            .iter()
            .map(|t| t.duration_display())
            .max_by_key(|s| s.len())
            .unwrap_or_else(|| "0:00".to_string());

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
pub fn list_item<'a>(
    content: impl Into<Element<'a, Message>>,
    on_press: Message,
    padding: u16,
) -> Element<'a, Message> {
    button::custom(content)
        .on_press(on_press)
        .width(Length::Fill)
        .padding(padding)
        .class(cosmic::theme::Button::MenuItem)
        .into()
}

// =============================================================================
// Fading Text Helpers
// =============================================================================

/// Create a text column with a gradient fade-out overlay on the right edge.
///
/// Uses [`FadingClip`] to GPU-clip overflowing text **and** draw a gradient
/// that automatically matches the current button background (normal or
/// hovered), so the fade is invisible in every interactive state.
pub fn fading_text_column<'a>(children: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let text_col = widget::Column::with_children(children).width(Length::Fill);

    FadingClip::new(text_col, FADE_WIDTH)
        .width(Length::Fill)
        .into()
}

/// Wrap any element in a [`FadingClip`] that fades to the card/component
/// background colour.
///
/// Use this for content inside a [`cosmic::theme::Container::Card`], such as
/// the now-playing track info column.
pub fn fading_card_column<'a>(child: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    FadingClip::new(child, FADE_WIDTH)
        .width(Length::Fill)
        .card()
        .into()
}

/// Wrap text in a [`FadingClip`] that fades to the `Button::Suggested`
/// (accent) background colour. For text inside suggested action buttons.
pub fn fading_suggested_text<'a>(child: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    FadingClip::new(child, FADE_WIDTH)
        .width(Length::Fill)
        .suggested()
        .into()
}

/// Wrap text in a [`FadingClip`] that fades to the `Button::Standard`
/// background colour. For text inside standard action buttons.
pub fn fading_standard_text<'a>(child: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    FadingClip::new(child, FADE_WIDTH)
        .width(Length::Fill)
        .standard()
        .into()
}

/// Wrap panel button text in a width-limited [`FadingClip`] that fades to
/// the surface colour.
///
/// The panel background is managed by the compositor and can vary, but
/// `background.base` is the closest match for most configurations.
pub fn fading_panel_text<'a>(child: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(
        FadingClip::new(child, FADE_WIDTH)
            .width(Length::Shrink)
            .panel(),
    )
    .max_width(MAX_PANEL_TEXT_WIDTH)
    .into()
}

/// Wrap a header title in a [`FadingClip`] that fades to the popup surface
/// colour.  Unlike [`fading_text_column`] (designed for list-item buttons),
/// this variant uses `surface_only` so the gradient is correct when the
/// element sits directly on the popup background with no enclosing button.
pub fn fading_header_title<'a>(title: &str) -> Element<'a, Message> {
    let label = text(title.to_string()).size(18).wrapping(Wrapping::None);

    FadingClip::new(label, FADE_WIDTH)
        .width(Length::Fill)
        .surface_only()
        .into()
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
pub fn scrollable_list(
    content: widget::Column<'_, Message, cosmic::Theme>,
) -> Element<'_, Message> {
    #[cfg(feature = "panel-applet")]
    {
        use super::constants::MAX_POPUP_HEIGHT;
        container(scrollable(content.padding([0, 12, 0, 0])).height(Length::Shrink))
            .max_height(MAX_POPUP_HEIGHT)
            .into()
    }
    #[cfg(not(feature = "panel-applet"))]
    {
        scrollable(content.padding([0, 12, 0, 0]))
            .height(Length::Fill)
            .into()
    }
}
