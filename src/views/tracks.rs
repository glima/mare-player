// SPDX-License-Identifier: MIT

//! Favorite tracks view for Maré Player.
//!
//! This module renders the favorite tracks list view using iced's virtual
//! [`List`] widget, which only materialises the rows currently visible in
//! the viewport.  The underlying data lives in
//! [`AppModel::track_list_content`] and is kept in sync by the handler
//! helper `rebuild_favorites_track_list`.
//!
//! A toggleable search bar filters the tracks client-side by matching
//! the query against track title, artist name, and album name.

use std::sync::Arc;

use crate::fl;
use cosmic::Element;
use cosmic::iced::widget::text_input;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::rows::build_track_row;
use crate::views::components::{TrackRowOptions, scrollable_element};

impl AppModel {
    /// Render the favorite tracks list view.
    ///
    /// Track rows are rendered through a virtual `List` — only the rows
    /// visible in the scroll viewport are materialised, eliminating the
    /// O(N) widget-tree and layout cost that the old `Column`-based
    /// approach incurred for every frame.
    pub fn view_favorite_tracks(&self) -> Element<'_, Message> {
        // --- header row ---
        let header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::ShowMain)
                    .padding(4),
            )
            .push(text(fl!("favorite-tracks")).size(18))
            .push(widget::space::horizontal())
            .push(
                button::icon(widget::icon::from_name("system-search-symbolic"))
                    .tooltip(fl!("tooltip-search"))
                    .on_press(Message::ToggleFavoriteTracksFilter)
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
                            Some(fl!("context-favorites")),
                        ))
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

        if self.favorite_tracks_filter_visible {
            let filter_bar = text_input(
                &fl!("favorite-tracks-filter-placeholder"),
                &self.favorite_tracks_filter_query,
            )
            .id("favorite-tracks-filter-input")
            .on_input(Message::FavoriteTracksFilterChanged)
            .width(Length::Fill);
            col = col.push(filter_bar);
        }

        // --- track list (virtual — only visible rows are built) ---
        let content: Element<'_, Message> = if self.track_list_content.is_empty() {
            if self.is_loading {
                text(fl!("loading-tracks")).size(14).into()
            } else if !self.favorite_tracks_filter_query.is_empty()
                && self.favorite_tracks_filter_visible
            {
                // Tracks exist but filter matched nothing.
                text(fl!("no-results")).size(14).into()
            } else {
                widget::Column::new()
                    .push(text(fl!("no-favorite-tracks")).size(14))
                    .push(button::text(fl!("refresh")).on_press(Message::LoadFavoriteTracks))
                    .spacing(8)
                    .into()
            }
        } else {
            let loaded_images = &self.loaded_images;
            let opts = TrackRowOptions {
                tracks: Arc::clone(&self.track_list_arc),
                context: Some(fl!("context-favorites")),
                fallback_icon: "emblem-favorite-symbolic",
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
