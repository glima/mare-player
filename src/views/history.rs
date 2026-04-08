// SPDX-License-Identifier: MIT

//! Play history view for Maré Player.
//!
//! This module renders the locally-tracked "Recently Played" track list
//! using iced's virtual [`List`] widget, which only materialises the rows
//! currently visible in the viewport.  The underlying data lives in
//! [`AppModel::track_list_content`] and is kept in sync by the handler
//! helpers (`rebuild_history_track_list`, `set_track_list`).
//!
//! A toggleable search bar filters the history client-side by matching
//! the query against track title, artist name, and album name.

use std::sync::Arc;

use cosmic::Element;
use cosmic::iced::widget::text_input;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::rows::build_track_row;
use crate::views::components::{TrackRowOptions, scrollable_element};

impl AppModel {
    /// Render the play history list view.
    ///
    /// Track rows are rendered through a virtual `List` — only the rows
    /// visible in the scroll viewport are materialised, eliminating the
    /// O(N) widget-tree and layout cost that the old `Column`-based
    /// approach incurred for every frame.
    pub fn view_history(&self) -> Element<'_, Message> {
        // --- header row ---
        let header = widget::Row::new()
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
                    .on_press_maybe(if self.track_list_content.is_empty() {
                        None
                    } else {
                        Some(Message::ShufflePlay(
                            Arc::clone(&self.track_list_arc),
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
        let mut col = widget::Column::new()
            .spacing(12)
            .padding(12)
            .width(Length::Fill);
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

        // --- track list (virtual — only visible rows are built) ---
        let content: Element<'_, Message> = if self.track_list_content.is_empty() {
            if self.play_history.is_empty() {
                text(fl!("no-history")).size(14).into()
            } else {
                // History has entries but filter matched nothing.
                text(fl!("no-results")).size(14).into()
            }
        } else {
            let loaded_images = &self.loaded_images;
            let opts = TrackRowOptions {
                tracks: Arc::clone(&self.track_list_arc),
                context: Some(fl!("context-history")),
                fallback_icon: "document-open-recent-symbolic",
                ..Default::default()
            };

            let track_list = cosmic::iced::widget::list::List::new(
                &self.track_list_content,
                move |index, track| build_track_row(loaded_images, track, index, &opts),
            )
            .spacing(2);

            scrollable_element(track_list)
        };

        col.push(content).into()
    }
}
