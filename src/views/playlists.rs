// SPDX-License-Identifier: GPL-3.0-only

//! Playlist views for Maré Player.
//!
//! This module contains the playlist list view and playlist detail view.
//! The detail view of a playlist the user created can delete it, once they
//! confirm.
//! The list view orders the user's playlists A–Z or by date created, with
//! the same sort buttons as the favourites.

use std::sync::Arc;

use crate::fl;
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::{
    TrackRowOptions, back_button, fading_header_title, scrollable_element, scrollable_list, sort_buttons,
};

impl AppModel {
    /// Render the playlists list view.
    pub fn view_playlists(&self) -> Element<'_, Message> {
        let header = widget::Row::new()
            .push(back_button(Message::ShowMain))
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
                .map(|p| self.playlist_row(p, Message::ShowPlaylistDetail(p.uuid.clone(), p.title.clone())))
                .collect();

            scrollable_list(widget::Column::with_children(playlist_items).spacing(4))
        };

        let mut col = widget::Column::new().push(header).spacing(12).padding(12).width(Length::Fill);
        if !self.user_playlists.is_empty() {
            col = col.push(sort_buttons(self.config.playlists_sort, fl!("sort-date-added"), Message::SortPlaylists));
        }
        col.push(content).into()
    }

    /// Render the playlist detail view showing tracks in a playlist.
    pub fn view_playlist_detail(&self) -> Element<'_, Message> {
        let fallback_playlist = fl!("fallback-playlist");
        let title = self.selected_playlist_name.as_deref().unwrap_or(&fallback_playlist);

        let header = widget::Row::new()
            .push(back_button(Message::NavigateBack))
            .push(fading_header_title(title))
            .push(
                button::icon(widget::icon::from_name("media-playlist-shuffle-symbolic"))
                    .tooltip(fl!("tooltip-shuffle-play"))
                    .on_press_maybe(if self.track_list_arc.is_empty() {
                        None
                    } else {
                        Some(Message::ShufflePlay(
                            Arc::clone(&self.track_list_arc),
                            match (&self.selected_playlist_uuid, &self.selected_playlist_name) {
                                (Some(uuid), Some(name)) => {
                                    Some(crate::tidal::models::PlaybackSource::playlist(uuid.clone(), name.clone()))
                                }
                                _ => None,
                            },
                        ))
                    })
                    .padding(4),
            )
            .push_maybe(self.selected_playlist_uuid.as_deref().filter(|uuid| self.owns_playlist(uuid)).map(|_| {
                button::icon(widget::icon::from_name("edit-delete-symbolic"))
                    .tooltip(fl!("delete-playlist"))
                    .on_press(Message::AskDeletePlaylist)
                    .padding(4)
            }))
            .spacing(8)
            .align_y(Alignment::Center);

        // Deleting can't be undone, so it asks first.
        let confirm_delete =
            self.confirm_delete_playlist.as_ref().filter(|uuid| self.selected_playlist_uuid.as_ref() == Some(*uuid)).map(
                |_| -> Element<'_, Message> {
                    widget::container(
                        widget::Column::new()
                            .push(text(fl!("delete-playlist-confirm", playlist = title.to_string())).size(13))
                            .push(
                                widget::Row::new()
                                    .push(widget::space::horizontal())
                                    .push(button::standard(fl!("cancel")).on_press(Message::CancelDeletePlaylist))
                                    .push(button::destructive(fl!("delete")).on_press(Message::DeletePlaylist))
                                    .spacing(8),
                            )
                            .spacing(8),
                    )
                    .padding(12)
                    .class(cosmic::theme::Container::Card)
                    .into()
                },
            );

        let tracks_content: Element<'_, Message> = if self.is_loading {
            text(fl!("loading-tracks")).size(14).into()
        } else if self.selected_playlist_tracks.is_empty() {
            text(fl!("no-tracks-playlist")).size(14).into()
        } else {
            let source = match (&self.selected_playlist_uuid, &self.selected_playlist_name) {
                (Some(uuid), Some(name)) => Some(crate::tidal::models::PlaybackSource::playlist(uuid.clone(), name.clone())),
                _ => None,
            };
            let opts = TrackRowOptions { tracks: Arc::clone(&self.track_list_arc), source, ..self.track_row_options() };

            let track_list = self.track_list(opts);

            scrollable_element(track_list)
        };

        widget::Column::new()
            .push(header)
            .push_maybe(confirm_delete)
            .push(tracks_content)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }
}
