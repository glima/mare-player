// SPDX-License-Identifier: MIT

//! Share prompt view for Maré Player.
//!
//! This module contains the share dialog for generating song.link URLs
//! to share tracks and albums across platforms.

use cosmic::Element;
use cosmic::iced::alignment::Horizontal;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::{fading_standard_text, fading_suggested_text};

impl AppModel {
    /// Render the share prompt dialog.
    ///
    /// Shows options to share the current track or album via song.link.
    pub fn view_share_prompt(
        &self,
        track_id: String,
        track_title: String,
        album_id: Option<String>,
        album_title: Option<String>,
    ) -> Element<'_, Message> {
        let header = widget::row()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::CancelShare)
                    .padding(4),
            )
            .push(text(fl!("share")).size(18))
            .spacing(8)
            .align_y(Alignment::Center);

        let description = text(fl!("share-description"))
            .size(12)
            .align_x(Horizontal::Center)
            .width(Length::Fill);

        let track_label = text(fl!("share-track", title = track_title.clone()))
            .size(14)
            .wrapping(Wrapping::None);
        let track_btn = button::custom(fading_suggested_text(track_label))
            .on_press(Message::ShareTrack(track_id, track_title))
            .width(Length::Fill)
            .class(cosmic::theme::Button::Suggested);

        let album_btn: Option<Element<'_, Message>> =
            if let (Some(id), Some(title)) = (album_id, album_title) {
                let album_label = text(fl!("share-album", title = title.clone()))
                    .size(14)
                    .wrapping(Wrapping::None);
                Some(
                    button::custom(fading_standard_text(album_label))
                        .on_press(Message::ShareAlbum(id, title))
                        .width(Length::Fill)
                        .class(cosmic::theme::Button::Standard)
                        .into(),
                )
            } else {
                None
            };

        let mut content = widget::column()
            .push(header)
            .push(widget::space::vertical().height(12))
            .push(description)
            .push(widget::space::vertical().height(16))
            .push(track_btn);

        if let Some(album) = album_btn {
            content = content.push(widget::space::vertical().height(8)).push(album);
        }

        content.spacing(4).padding(12).width(Length::Fill).into()
    }
}
