// SPDX-License-Identifier: MIT

//! Play history view for Maré Player.
//!
//! This module renders the locally-tracked "Recently Played" track list.
//! It mirrors the structure of the favorite tracks view but pulls data
//! from [`PlayHistory`](crate::tidal::play_history::PlayHistory) instead
//! of the TIDAL API.
//!
//! A toggleable search bar filters the history client-side by matching
//! the query against track title, artist name, and album name.

use crate::fl;
use cosmic::Element;
use cosmic::iced::widget::text_input;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::{TrackRowOptions, scrollable_list};

impl AppModel {
    /// Render the play history list view.
    pub fn view_history(&self) -> Element<'_, Message> {
        let all_tracks = self.play_history.tracks();

        // Apply client-side filter when the search bar is visible and non-empty.
        let tracks: Vec<_> = if self.history_filter_visible && !self.history_filter_query.is_empty()
        {
            let query = self.history_filter_query.to_lowercase();
            all_tracks
                .into_iter()
                .filter(|t| {
                    t.title.to_lowercase().contains(&query)
                        || t.artist_name.to_lowercase().contains(&query)
                        || t.album_name
                            .as_deref()
                            .is_some_and(|a| a.to_lowercase().contains(&query))
                })
                .collect()
        } else {
            all_tracks
        };

        // --- header row ---
        let header = widget::row()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::ShowMain)
                    .padding(4),
            )
            .push(text(fl!("history")).size(18))
            .push(widget::space::horizontal())
            .push(
                button::icon(widget::icon::from_name("system-search-symbolic"))
                    .tooltip(fl!("tooltip-search"))
                    .on_press(Message::ToggleHistoryFilter)
                    .padding(4),
            )
            .push(
                button::icon(widget::icon::from_name("media-playlist-shuffle-symbolic"))
                    .tooltip(fl!("tooltip-shuffle-play"))
                    .on_press_maybe(if tracks.is_empty() {
                        None
                    } else {
                        Some(Message::ShufflePlay(
                            tracks.clone(),
                            Some(fl!("context-history")),
                        ))
                    })
                    .padding(4),
            )
            .push(
                button::icon(widget::icon::from_name("edit-clear-all-symbolic"))
                    .tooltip(fl!("clear-history"))
                    .on_press_maybe(if self.play_history.is_empty() {
                        None
                    } else {
                        Some(Message::ClearHistory)
                    })
                    .padding(4),
            )
            .spacing(8)
            .align_y(Alignment::Center);

        // --- optional filter bar ---
        let mut col = widget::column().spacing(12).padding(12).width(Length::Fill);
        col = col.push(header);

        if self.history_filter_visible {
            let filter_bar = text_input(
                &fl!("history-filter-placeholder"),
                &self.history_filter_query,
            )
            .id("history-filter-input")
            .on_input(Message::HistoryFilterChanged)
            .width(Length::Fill);
            col = col.push(filter_bar);
        }

        // --- track list ---
        let content: Element<'_, Message> = if tracks.is_empty() {
            if self.play_history.is_empty() {
                text(fl!("no-history")).size(14).into()
            } else {
                // History has entries but filter matched nothing.
                text(fl!("no-results")).size(14).into()
            }
        } else {
            let track_items: Vec<Element<'_, Message>> = tracks
                .iter()
                .enumerate()
                .map(|(index, track)| {
                    self.track_row(
                        track,
                        index,
                        &TrackRowOptions {
                            tracks: &tracks,
                            context: Some(fl!("context-history")),
                            fallback_icon: "document-open-recent-symbolic",
                            ..Default::default()
                        },
                    )
                })
                .collect();

            scrollable_list(widget::column::with_children(track_items).spacing(2))
        };

        col.push(content).into()
    }
}
