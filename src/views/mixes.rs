// SPDX-License-Identifier: MIT

//! Mixes & Radio views for Maré Player.
//!
//! This module contains the mixes list view (personalized mixes from the
//! TIDAL home feed) and the mix detail view showing tracks in a selected mix.
//!
//! The mix detail track list uses iced's virtual [`List`] widget so that only
//! the rows visible in the viewport are materialised.

use std::sync::Arc;

use crate::fl;
use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::models::Mix;
use crate::views::components::rows::build_track_row;
use crate::views::components::{
    THUMBNAIL_SIZE, TrackRowOptions, fading_header_title, fading_text_column, list_item,
    scrollable_element, scrollable_list,
};

impl AppModel {
    /// Render the mixes & radio list view.
    pub fn view_mixes(&self) -> Element<'_, Message> {
        let header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::ShowMain)
                    .padding(4),
            )
            .push(text(fl!("mixes-and-radio")).size(18))
            .push(widget::space::horizontal())
            .push(
                button::icon(widget::icon::from_name("view-refresh-symbolic"))
                    .tooltip(fl!("tooltip-refresh"))
                    .on_press(Message::LoadMixes)
                    .padding(4),
            )
            .spacing(8)
            .align_y(Alignment::Center);

        let content: Element<'_, Message> = if self.user_mixes.is_empty() {
            if self.is_loading {
                text(fl!("loading-mixes")).size(14).into()
            } else {
                widget::Column::new()
                    .push(text(fl!("no-mixes-found")).size(14))
                    .push(button::text(fl!("refresh")).on_press(Message::LoadMixes))
                    .spacing(8)
                    .into()
            }
        } else {
            let mix_items: Vec<Element<'_, Message>> = self
                .user_mixes
                .iter()
                .map(|mix| self.mix_row(mix))
                .collect();

            scrollable_list(widget::Column::with_children(mix_items).spacing(4))
        };

        widget::Column::new()
            .push(header)
            .push(content)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }

    /// Render the mix detail view showing tracks in a mix.
    pub fn view_mix_detail(&self) -> Element<'_, Message> {
        let fallback_mix = fl!("fallback-mix");
        let title = self.selected_mix_name.as_deref().unwrap_or(&fallback_mix);

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
                    .on_press_maybe(if self.track_list_content.is_empty() {
                        None
                    } else {
                        Some(Message::ShufflePlay(
                            Arc::clone(&self.track_list_arc),
                            self.selected_mix_name.clone(),
                        ))
                    })
                    .padding(4),
            )
            .spacing(8)
            .align_y(Alignment::Center);

        let tracks_content: Element<'_, Message> = if self.is_loading {
            text(fl!("loading-tracks")).size(14).into()
        } else if self.selected_mix_tracks.is_empty() {
            text(fl!("no-tracks-mix")).size(14).into()
        } else {
            let loaded_images = &self.loaded_images;
            let context = self.selected_mix_name.clone();
            let opts = TrackRowOptions {
                tracks: Arc::clone(&self.track_list_arc),
                context: context.clone(),
                ..Default::default()
            };

            let track_list = cosmic::iced::widget::list::List::new(
                &self.track_list_content,
                move |index, track| build_track_row(loaded_images, track, index, &opts),
            )
            .spacing(2);

            scrollable_element(track_list)
        };

        widget::Column::new()
            .push(header)
            .push(tracks_content)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }

    /// Create a mix list-item element (thumbnail + title + subtitle).
    fn mix_row<'a>(&self, mix: &Mix) -> Element<'a, Message> {
        let info = fading_text_column(vec![
            text(mix.title.clone())
                .size(13)
                .wrapping(Wrapping::None)
                .into(),
            text(mix.subtitle.clone())
                .size(11)
                .wrapping(Wrapping::None)
                .into(),
        ]);

        // Use mix cover image or a fallback icon
        let thumb: Element<'_, Message> = if let Some(url) = &mix.image_url {
            if let Some(handle) = self.loaded_images.get(url) {
                cosmic::widget::image(handle.clone())
                    .width(THUMBNAIL_SIZE)
                    .height(THUMBNAIL_SIZE)
                    .into()
            } else {
                widget::icon::from_name("media-playlist-shuffle-symbolic")
                    .size(THUMBNAIL_SIZE)
                    .into()
            }
        } else {
            widget::icon::from_name("media-playlist-shuffle-symbolic")
                .size(THUMBNAIL_SIZE)
                .into()
        };

        let row = widget::Row::new()
            .push(thumb)
            .push(info)
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        list_item(
            row,
            Message::ShowMixDetail(mix.id.clone(), mix.title.clone()),
            6,
        )
    }
}
