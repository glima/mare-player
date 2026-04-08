// SPDX-License-Identifier: MIT

//! Profiles (followed artists) view for Maré Player.
//!
//! This module renders the user's followed artists as a browsable list.
//! Tapping an artist navigates to the existing artist detail view.

use crate::fl;
use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::models::Artist;
use crate::views::components::{THUMBNAIL_SIZE, fading_text_column, list_item, scrollable_list};

impl AppModel {
    /// Render the followed artists (profiles) list view.
    pub fn view_profiles(&self) -> Element<'_, Message> {
        let header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::ShowMain)
                    .padding(4),
            )
            .push(text(fl!("profiles")).size(18))
            .push(widget::space::horizontal())
            .push(
                button::icon(widget::icon::from_name("view-refresh-symbolic"))
                    .tooltip(fl!("tooltip-refresh"))
                    .on_press(Message::LoadProfiles)
                    .padding(4),
            )
            .spacing(8)
            .align_y(Alignment::Center);

        let content: Element<'_, Message> = if self.user_followed_artists.is_empty() {
            if self.is_loading {
                text(fl!("loading-followed-artists")).size(14).into()
            } else {
                widget::Column::new()
                    .push(text(fl!("no-followed-artists")).size(14))
                    .push(button::text(fl!("refresh")).on_press(Message::LoadProfiles))
                    .spacing(8)
                    .into()
            }
        } else {
            let count = self.user_followed_artists.len();
            let count_label = widget::Row::new()
                .push(text(fl!("artist-count", count = count)).size(12))
                .padding([0, 0, 4, 0]);

            let artist_items: Vec<Element<'_, Message>> = self
                .user_followed_artists
                .iter()
                .map(|artist| self.profile_artist_row(artist))
                .collect();

            let list = scrollable_list(widget::Column::with_children(artist_items).spacing(4));

            widget::Column::new()
                .push(count_label)
                .push(list)
                .spacing(4)
                .into()
        };

        widget::Column::new()
            .push(header)
            .push(content)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }

    /// Create an artist list-item for the profiles view (picture + name + role).
    ///
    /// Navigates to the existing artist detail view on click.
    fn profile_artist_row<'a>(&self, artist: &Artist) -> Element<'a, Message> {
        let mut info_children: Vec<Element<'_, Message>> = vec![
            text(artist.name.clone())
                .size(13)
                .wrapping(Wrapping::None)
                .into(),
        ];

        // Show primary role if available (e.g. "Artist", "Producer", "DJ")
        if let Some(role) = artist.roles.first() {
            info_children.push(text(role.clone()).size(11).wrapping(Wrapping::None).into());
        }

        let info = fading_text_column(info_children);

        // Use artist picture or a fallback icon
        let thumb: Element<'_, Message> = if let Some(url) = &artist.picture_url {
            if let Some(handle) = self.loaded_images.get(url) {
                cosmic::widget::image(handle.clone())
                    .width(THUMBNAIL_SIZE)
                    .height(THUMBNAIL_SIZE)
                    .into()
            } else {
                widget::icon::from_name("system-users-symbolic")
                    .size(THUMBNAIL_SIZE)
                    .into()
            }
        } else {
            widget::icon::from_name("system-users-symbolic")
                .size(THUMBNAIL_SIZE)
                .into()
        };

        let row = widget::Row::new()
            .push(thumb)
            .push(info)
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        list_item(row, Message::ShowArtistDetail(artist.id.clone()), 6)
    }
}
