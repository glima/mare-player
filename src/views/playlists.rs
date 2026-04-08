// SPDX-License-Identifier: MIT

//! Playlist views for Maré Player.
//!
//! This module contains the playlist list view and playlist detail view.

use std::sync::Arc;

use crate::fl;
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::{TrackRowOptions, fading_header_title, scrollable_list};

impl AppModel {
    /// Render the playlists list view.
    pub fn view_playlists(&self) -> Element<'_, Message> {
        let header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::ShowMain)
                    .padding(4),
            )
            .push(text(fl!("playlists")).size(18))
            .spacing(8)
            .align_y(Alignment::Center);

        let content: Element<'_, Message> = if self.user_playlists.is_empty() {
            if self.is_loading {
                text(fl!("loading-playlists")).size(14).into()
            } else {
                widget::Column::new()
                    .push(text(fl!("no-playlists-found")).size(14))
                    .push(button::text(fl!("refresh")).on_press(Message::LoadPlaylists))
                    .spacing(8)
                    .into()
            }
        } else {
            let playlist_items: Vec<Element<'_, Message>> = self
                .user_playlists
                .iter()
                .map(|playlist| self.playlist_row(playlist))
                .collect();

            scrollable_list(widget::Column::with_children(playlist_items).spacing(4))
        };

        widget::Column::new()
            .push(header)
            .push(content)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }

    /// Render the playlist detail view showing tracks in a playlist.
    pub fn view_playlist_detail(&self) -> Element<'_, Message> {
        let fallback_playlist = fl!("fallback-playlist");
        let title = self
            .selected_playlist_name
            .as_deref()
            .unwrap_or(&fallback_playlist);
        let tracks: Arc<[_]> = self.selected_playlist_tracks.clone().into();

        let header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::NavigateBack)
                    .padding(4),
            )
            .push(fading_header_title(title))
            .push(
                button::icon(widget::icon::from_name("media-playlist-shuffle-symbolic"))
                    .tooltip(fl!("tooltip-shuffle-play"))
                    .on_press_maybe(if tracks.is_empty() {
                        None
                    } else {
                        Some(Message::ShufflePlay(
                            Arc::clone(&tracks),
                            self.selected_playlist_name.clone(),
                        ))
                    })
                    .padding(4),
            )
            .spacing(8)
            .align_y(Alignment::Center);

        let tracks_content: Element<'_, Message> = if self.is_loading {
            text(fl!("loading-tracks")).size(14).into()
        } else if self.selected_playlist_tracks.is_empty() {
            text(fl!("no-tracks-playlist")).size(14).into()
        } else {
            let context = self.selected_playlist_name.clone();
            let track_items: Vec<Element<'_, Message>> = tracks
                .iter()
                .enumerate()
                .map(|(index, track)| {
                    self.track_row(
                        track,
                        index,
                        &TrackRowOptions {
                            tracks: Arc::clone(&tracks),
                            context: context.clone(),
                            ..Default::default()
                        },
                    )
                })
                .collect();

            scrollable_list(widget::Column::with_children(track_items).spacing(2))
        };

        widget::Column::new()
            .push(header)
            .push(tracks_content)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }
}
