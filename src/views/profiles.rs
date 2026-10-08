// SPDX-License-Identifier: GPL-3.0-only

//! Profiles (followed artists) view for Maré Player.
//!
//! This module renders the user's followed artists as a browsable list.
//! Tapping an artist navigates to the existing artist detail view.

use crate::fl;
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::rows::build_profile_artist_row;
use crate::views::components::{back_button, scrollable_element, virtual_list_row};

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
            let count_label = widget::Row::new().push(text(fl!("artist-count", count = count)).size(12)).padding([0, 0, 4, 0]);

            let loaded_images = &self.loaded_images;
            let list = cosmic::iced::widget::list::List::new(&self.profiles_content, move |_index, artist| {
                virtual_list_row(build_profile_artist_row(loaded_images, artist), 2)
            });

            widget::Column::new().push(count_label).push(scrollable_element(list)).spacing(4).into()
        };

        widget::Column::new().push(header).push(content).spacing(12).padding(12).width(Length::Fill).into()
    }
}
