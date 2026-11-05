// SPDX-License-Identifier: GPL-3.0-only

//! Lyrics view for Maré Player.
//!
//! Renders the currently-selected track's lyrics in one of three modes,
//! picked at render time based on what TIDAL returned:
//!
//! 1. **Synced** — when LRC subtitles are available, render each line
//!    as a separate text widget and highlight the currently-active line
//!    based on the live playback position (driven by
//!    `handle_playback_tick`).
//! 2. **Plain** — when only flat text is available, render the whole
//!    block in a single scrollable text widget.
//! 3. **Empty** — when TIDAL has no lyrics for this track, show an
//!    informative empty state with the track title for context.
//!
//! Provider attribution (`MusixMatch`, `TIDAL`, etc.) is shown in a
//! small footer at the bottom of the view per the third-party
//! provider's licensing requirements.

use cosmic::Element;
use cosmic::iced::alignment::Horizontal;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, container, text};

use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::models::TrackLyrics;
use crate::views::components::{back_button, fading_header_title, scrollable_element};

/// Font size for the active synced line (the karaoke "now-playing" line).
const ACTIVE_LINE_SIZE: u16 = 18;
/// Font size for inactive synced lines (past or upcoming).
const INACTIVE_LINE_SIZE: u16 = 14;

impl AppModel {
    /// Render the lyrics view for the currently-selected lyrics track.
    pub fn view_lyrics(&self) -> Element<'_, Message> {
        let track_title =
            self.selected_lyrics_track.as_ref().map(|t| t.title.clone()).unwrap_or_else(|| fl!("lyrics-title-fallback"));
        let header_title = fl!("lyrics-title", title = track_title.clone());

        let header = widget::Row::new()
            .push(back_button(Message::NavigateBack))
            .push(fading_header_title(&header_title))
            .spacing(8)
            .align_y(Alignment::Center);

        // Three render paths, picked from what `handle_track_lyrics_loaded`
        // stored.  `None` = still loading; the loader has been kicked
        // off by `handle_show_lyrics`.
        let body: Element<'_, Message> = match &self.selected_track_lyrics {
            None => container(text(fl!("loading-lyrics")).size(14))
                .width(Length::Fill)
                .align_x(Horizontal::Center)
                .padding([24, 0])
                .into(),
            Some(lyrics) if lyrics.is_empty() => container(text(fl!("no-lyrics-available", title = track_title)).size(14))
                .width(Length::Fill)
                .align_x(Horizontal::Center)
                .padding([24, 0])
                .into(),
            Some(lyrics) if lyrics.is_synced() => self.render_synced_lyrics(lyrics),
            Some(lyrics) => self.render_plain_lyrics(lyrics),
        };

        // Provider attribution footer: shown only when we actually
        // rendered lyrics (loading / empty states have nothing to
        // attribute).
        let attribution: Option<Element<'_, Message>> =
            self.selected_track_lyrics.as_ref().filter(|l| !l.is_empty()).and_then(|l| l.provider.as_deref()).map(|p| {
                container(text(fl!("lyrics-provider", provider = p.to_string())).size(11).wrapping(Wrapping::None))
                    .width(Length::Fill)
                    .align_x(Horizontal::Center)
                    .padding([4, 0])
                    .into()
            });

        let mut column = widget::Column::new().push(header).push(body).spacing(12).padding(12).width(Length::Fill);
        if let Some(footer) = attribution {
            column = column.push(footer);
        }
        column.into()
    }

    /// Render LRC-format synced lyrics with karaoke-style highlighting.
    ///
    /// The currently-active line (per `current_lyric_index`) is rendered
    /// larger and at full opacity; other lines are dimmer and smaller.
    /// Auto-scrolling to the active line is left as a future
    /// enhancement — it requires a stable `scrollable::Id` and a
    /// `scrollable::scroll_to` `Task`, neither of which compose well
    /// with mare's view-tree rebuild model.
    fn render_synced_lyrics(&self, lyrics: &TrackLyrics) -> Element<'_, Message> {
        let active = self.current_lyric_index;
        let alignment = if lyrics.is_right_to_left { Horizontal::Right } else { Horizontal::Center };

        let mut column = widget::Column::new().spacing(8).padding([8, 16]).width(Length::Fill);

        for (i, line) in lyrics.lrc_lines.iter().enumerate() {
            let is_active = active == Some(i);
            let size = if is_active { ACTIVE_LINE_SIZE } else { INACTIVE_LINE_SIZE };
            // Empty LRC lines are common as visual rests between
            // verses; render as a small vertical gap rather than an
            // empty text widget so the gap is consistent.
            let display = if line.text.is_empty() { "·".to_string() } else { line.text.clone() };
            let line_widget = container(text(display).size(size)).width(Length::Fill).align_x(alignment);
            column = column.push(line_widget);
        }

        scrollable_element(column)
    }

    /// Render plain-text (non-synced) lyrics as a single scrollable block.
    ///
    /// Preserves the original line breaks from TIDAL; the wrapping is
    /// handled by the text widget so very long lines (rare in lyrics
    /// but possible in spoken-word tracks) still display correctly.
    fn render_plain_lyrics(&self, lyrics: &TrackLyrics) -> Element<'_, Message> {
        let alignment = if lyrics.is_right_to_left { Horizontal::Right } else { Horizontal::Center };
        let body = text(lyrics.plain_text.clone().unwrap_or_default()).size(14);
        let centered = container(body).width(Length::Fill).align_x(alignment).padding([8, 16]);
        scrollable_element(centered)
    }
}

// The panel applet's popup sizes itself to its content, so a lyrics view
// that isn't capped grows taller than the screen. Lay it out with the CPU
// renderer (the default applet build) and check the cap holds.
#[cfg(all(test, feature = "panel-applet", not(feature = "wgpu")))]
mod tests {
    use crate::state::AppModel;
    use crate::tidal::models::{LrcLine, TrackLyrics};
    use crate::views::components::MAX_POPUP_HEIGHT;
    use cosmic::Application;
    use cosmic::iced::core::{Size, layout, widget::Tree};

    const LINES: usize = 200;

    fn lyrics_view_height(lyrics: TrackLyrics) -> f32 {
        let (mut app, startup) = AppModel::init(cosmic::Core::default(), ());
        drop(startup);
        app.selected_track_lyrics = Some(lyrics);
        let renderer = cosmic::Renderer::new(cosmic::iced::Font::default(), 14.0.into());
        let mut view = app.view_lyrics();
        let mut tree = Tree::new(view.as_widget());
        // Popups are laid out with no height limit and shrink to fit.
        let limits = layout::Limits::new(Size::ZERO, Size::new(400.0, f32::INFINITY));
        view.as_widget_mut().layout(&mut tree, &renderer, &limits).size().height
    }

    /// Header, spacing and the provider footer around the scroll area.
    const CHROME: f32 = 150.0;

    // `AppModel::init` starts tidlers' play reporter, which needs a runtime.
    #[tokio::test]
    async fn long_plain_lyrics_scroll_within_the_popup() {
        let text = (0..LINES).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let height =
            lyrics_view_height(TrackLyrics { provider: Some("Test".into()), plain_text: Some(text), ..Default::default() });
        assert!(height <= MAX_POPUP_HEIGHT + CHROME, "plain lyrics view is {height}px tall");
    }

    #[tokio::test]
    async fn long_synced_lyrics_scroll_within_the_popup() {
        let lrc_lines = (0..LINES).map(|i| LrcLine { time_ms: i as u64 * 1000, text: format!("line {i}") }).collect();
        let height = lyrics_view_height(TrackLyrics { provider: Some("Test".into()), lrc_lines, ..Default::default() });
        assert!(height <= MAX_POPUP_HEIGHT + CHROME, "synced lyrics view is {height}px tall");
    }
}
