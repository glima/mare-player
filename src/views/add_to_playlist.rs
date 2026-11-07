// SPDX-License-Identifier: GPL-3.0-only

//! "Add to playlist" picker for Maré Player.
//!
//! Opened from the now-playing bar or a track's detail page. A form at the
//! top creates a new playlist, private or public as the user picks, with the
//! track in it; below it, the user's playlists (in the Playlists view's
//! order, each marked private or public) add the track to that one. The
//! playlist the picker was opened from isn't offered.

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::{back_button, chip, fading_header_title, fading_text, scrollable_list};

impl AppModel {
    /// Render the "Add to playlist" picker for `self.add_to_playlist`.
    pub fn view_add_to_playlist(&self) -> Element<'_, Message> {
        let header = widget::Row::new()
            .push(back_button(Message::NavigateBack))
            .push(fading_header_title(&fl!("add-to-playlist")))
            .spacing(8)
            .align_y(Alignment::Center);
        let mut page = widget::Column::new().push(header).spacing(12).padding(12).width(Length::Fill);
        let Some(picker) = &self.add_to_playlist else {
            return page.into();
        };

        let item = format!("{} — {}", picker.track.title, picker.track.artist_name);
        page = page.push(fading_text(text(item).size(12).wrapping(Wrapping::None)));

        // New playlist: a name, private or public, and Create.
        let name = picker.new_name.trim();
        let new_playlist = widget::Column::new()
            .push(text(fl!("new-playlist")).size(14))
            .push(
                widget::text_input(fl!("playlist-name"), &picker.new_name)
                    .on_input(Message::NewPlaylistNameChanged)
                    .on_submit(|_| Message::CreatePlaylistWithTrack),
            )
            .push(
                widget::Row::new()
                    .push(chip(
                        text(fl!("playlist-private")).size(12),
                        !picker.new_public,
                        Message::NewPlaylistPublicChanged(false),
                    ))
                    .push(chip(text(fl!("playlist-public")).size(12), picker.new_public, Message::NewPlaylistPublicChanged(true)))
                    .push(widget::space::horizontal())
                    .push(
                        button::suggested(fl!("create-playlist"))
                            .on_press_maybe((!name.is_empty()).then_some(Message::CreatePlaylistWithTrack)),
                    )
                    .spacing(6)
                    .align_y(Alignment::Center),
            )
            .spacing(8);
        page = page.push(new_playlist);

        // Existing playlists, minus the one the picker was opened from.
        let offered: Vec<Element<'_, Message>> = self
            .user_playlists
            .iter()
            .filter(|p| picker.exclude.as_deref() != Some(p.uuid.as_str()))
            .map(|p| self.playlist_row(p, Message::AddToPlaylist(p.uuid.clone())))
            .collect();
        let existing: Element<'_, Message> = if !offered.is_empty() {
            scrollable_list(widget::Column::with_children(offered).spacing(4))
        } else if self.is_loading {
            text(fl!("loading-playlists")).size(14).into()
        } else {
            text(fl!("no-playlists-found")).size(14).into()
        };
        page.push(widget::divider::horizontal::default()).push(existing).into()
    }
}
