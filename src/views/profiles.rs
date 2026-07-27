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
use crate::state::{AppModel, HandleCache};
use crate::tidal::models::Artist;
use crate::views::components::rows::build_thumbnail;
use crate::views::components::{
    back_button, fading_text_column, list_item, scrollable_element, virtual_list_row,
};

impl AppModel {
    /// Render the followed artists (profiles) list view.
    pub fn view_profiles(&self) -> Element<'_, Message> {
        let header = widget::Row::new()
            .push(back_button(Message::ShowMain))
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

            let loaded_images = &self.loaded_images;
            let list = cosmic::iced::widget::list::List::new(
                &self.profiles_content,
                move |_index, artist| {
                    virtual_list_row(build_profile_artist_row(loaded_images, artist), 2)
                },
            );

            widget::Column::new()
                .push(count_label)
                .push(scrollable_element(list))
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
}

/// Build a followed-artist list-item (picture + name + role) for the virtual
/// `List`. Navigates to the artist detail view on click.
fn build_profile_artist_row<'a>(
    loaded_images: &HandleCache,
    artist: &Artist,
) -> Element<'a, Message> {
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

    let row = widget::Row::new()
        .push(build_thumbnail(
            loaded_images,
            artist.picture_url.as_deref(),
            "system-users-symbolic",
        ))
        .push(fading_text_column(info_children))
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    list_item(row, Message::ShowArtistDetail(artist.id.clone()), 6)
}
