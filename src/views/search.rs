// SPDX-License-Identifier: MIT

//! Search view for Maré Player.
//!
//! This module contains the search interface with results for
//! tracks, albums, and playlists.

use crate::fl;
use cosmic::Element;
use cosmic::iced::widget::text_input;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::{TrackRowOptions, scrollable_list};

impl AppModel {
    /// Render the search view with search bar and results.
    pub fn view_search(&self) -> Element<'_, Message> {
        let header = widget::row()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::ShowMain)
                    .padding(4),
            )
            .push(text(fl!("search")).size(18))
            .spacing(8)
            .align_y(Alignment::Center);

        let search_bar = widget::row()
            .push(
                text_input(&fl!("search-placeholder"), &self.search_query)
                    .id("search-input")
                    .on_input(Message::SearchQueryChanged)
                    .on_submit(Message::PerformSearch)
                    .width(Length::Fill),
            )
            .push(
                button::icon(widget::icon::from_name("system-search-symbolic"))
                    .on_press(Message::PerformSearch)
                    .padding(4),
            )
            .spacing(8)
            .align_y(Alignment::Center);

        let results_content: Element<'_, Message> = if self.is_loading {
            text(fl!("searching")).size(14).into()
        } else if let Some(results) = &self.search_results {
            if results.is_empty() {
                text(fl!("no-results")).size(14).into()
            } else {
                let mut items_col = widget::column().spacing(4);

                // Tracks section
                if !results.tracks.is_empty() {
                    items_col = items_col.push(text(fl!("tracks")).size(12));
                    let search_tracks: Vec<_> = results.tracks.iter().take(5).cloned().collect();
                    for (index, track) in search_tracks.iter().enumerate() {
                        items_col = items_col.push(self.track_row(
                            track,
                            index,
                            &TrackRowOptions {
                                tracks: &search_tracks,
                                context: Some(fl!("context-search")),
                                ..Default::default()
                            },
                        ));
                    }
                }

                // Albums section
                if !results.albums.is_empty() {
                    items_col = items_col.push(widget::space::vertical().height(8));
                    items_col = items_col.push(text(fl!("albums")).size(12));
                    for album in results.albums.iter().take(3) {
                        items_col = items_col.push(self.album_row(album));
                    }
                }

                // Playlists section
                if !results.playlists.is_empty() {
                    items_col = items_col.push(widget::space::vertical().height(8));
                    items_col = items_col.push(text(fl!("playlists")).size(12));
                    for playlist in results.playlists.iter().take(3) {
                        items_col = items_col.push(self.playlist_row(playlist));
                    }
                }

                scrollable_list(items_col)
            }
        } else {
            text(fl!("enter-search-term")).size(14).into()
        };

        widget::column()
            .push(header)
            .push(search_bar)
            .push(results_content)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }
}
