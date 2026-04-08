// SPDX-License-Identifier: MIT

//! Album views for Maré Player.
//!
//! This module contains the album list view and album detail view.
//! The detail view shows album metadata (cover, artist, release date, quality,
//! track count), a favorite heart icon, and a clickable artist name that
//! navigates to the artist detail view.

use std::sync::Arc;

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::helpers::max_description_chars;
use crate::messages::Message;
use crate::state::AppModel;
use crate::views::artist::strip_markup;
use crate::views::components::rows::build_track_row;
use crate::views::components::{
    ALBUM_COVER_SIZE, TrackRowOptions, fading_header_title, favorite_icon_handle, scrollable_list,
};

impl AppModel {
    /// Render the albums list view.
    pub fn view_albums(&self) -> Element<'_, Message> {
        let header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::ShowMain)
                    .padding(4),
            )
            .push(text(fl!("albums")).size(18))
            .spacing(8)
            .align_y(Alignment::Center);

        let content: Element<'_, Message> = if self.user_albums.is_empty() {
            if self.is_loading {
                text(fl!("loading-albums")).size(14).into()
            } else {
                widget::Column::new()
                    .push(text(fl!("no-albums-found")).size(14))
                    .push(button::text(fl!("refresh")).on_press(Message::LoadAlbums))
                    .spacing(8)
                    .into()
            }
        } else {
            let album_items: Vec<Element<'_, Message>> = self
                .user_albums
                .iter()
                .map(|album| self.album_row(album))
                .collect();

            scrollable_list(widget::Column::with_children(album_items).spacing(4))
        };

        widget::Column::new()
            .push(header)
            .push(content)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }

    /// Render the album detail view showing album info, favorite button, and tracks.
    pub fn view_album_detail(&self) -> Element<'_, Message> {
        let fallback_album = fl!("fallback-album");
        let title = self
            .selected_album
            .as_ref()
            .map(|a| a.title.as_str())
            .unwrap_or(&fallback_album);
        // Header row: back button, title, favorite heart, shuffle button
        let mut header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::NavigateBack)
                    .padding(4),
            )
            .push(fading_header_title(title));

        // Favorite heart for the album
        if let Some(album) = &self.selected_album {
            let is_favorite = self.favorite_album_ids.contains(&album.id);
            let tooltip = if is_favorite {
                fl!("tooltip-remove-from-favorites")
            } else {
                fl!("tooltip-add-to-favorites")
            };
            header = header.push(
                button::icon(favorite_icon_handle(is_favorite))
                    .tooltip(tooltip)
                    .on_press(Message::ToggleFavoriteAlbum(album.clone()))
                    .padding(4),
            );
        }

        // Shuffle button
        header = header.push(
            button::icon(widget::icon::from_name("media-playlist-shuffle-symbolic"))
                .tooltip(fl!("tooltip-shuffle-play"))
                .on_press_maybe(if self.track_list_content.is_empty() {
                    None
                } else {
                    Some(Message::ShufflePlay(
                        Arc::clone(&self.track_list_arc),
                        self.selected_album.as_ref().map(|a| a.title.clone()),
                    ))
                })
                .padding(4),
        );

        let header = header.spacing(8).align_y(Alignment::Center);

        // Build the scrollable body: album info section + tracks
        let mut body = widget::Column::new().spacing(12).width(Length::Fill);

        // Album info section (cover + metadata)
        if let Some(album) = &self.selected_album {
            body = body.push(self.view_album_info_section(album));
        }

        // Tracks section
        let tracks_content: Element<'_, Message> =
            if self.is_loading && self.selected_album_tracks.is_empty() {
                text(fl!("loading-tracks")).size(14).into()
            } else if self.selected_album_tracks.is_empty() {
                text(fl!("no-tracks-album")).size(14).into()
            } else {
                let context = self.selected_album.as_ref().map(|a| a.title.clone());
                let loaded_images = &self.loaded_images;
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

                track_list.into()
            };

        body = body.push(tracks_content);

        let scrollable_body = scrollable_list(body);

        widget::Column::new()
            .push(header)
            .push(scrollable_body)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }

    /// Render the album info section: large cover, artist (clickable), release date,
    /// quality, track count, and duration.
    fn view_album_info_section(&self, album: &crate::tidal::models::Album) -> Element<'_, Message> {
        // Large album cover
        let cover: Element<'_, Message> = if let Some(url) = &album.cover_url
            && let Some(handle) = self.loaded_images.get(url)
        {
            cosmic::widget::image(handle.clone())
                .width(ALBUM_COVER_SIZE)
                .height(ALBUM_COVER_SIZE)
                .into()
        } else {
            widget::icon::from_name("media-optical-symbolic")
                .size(ALBUM_COVER_SIZE)
                .into()
        };

        // Metadata column
        let mut details = widget::Column::new()
            .spacing(3)
            .width(Length::Fill)
            .clip(true);

        // Artist name — clickable to navigate to artist detail
        let artist_element: Element<'_, Message> = if let Some(ref artist_id) = album.artist_id {
            button::custom(
                text(album.artist_name.clone())
                    .size(13)
                    .wrapping(Wrapping::None),
            )
            .on_press(Message::ShowArtistDetail(artist_id.clone()))
            .width(Length::Shrink)
            .padding(0)
            .class(cosmic::theme::Button::MenuItem)
            .into()
        } else {
            text(album.artist_name.clone())
                .size(13)
                .wrapping(Wrapping::None)
                .into()
        };
        details = details.push(artist_element);

        // Release date
        if let Some(ref date) = album.release_date {
            let year = date.split('-').next().unwrap_or(date);
            details = details.push(text(fl!("released", year = year)).size(11));
        }

        // Track count and total duration
        let mut meta_parts: Vec<String> = Vec::new();
        if album.num_tracks > 0 {
            meta_parts.push(fl!("track-count", count = album.num_tracks));
        }
        if album.duration > 0 {
            let minutes = album.duration / 60;
            let seconds = album.duration % 60;
            if minutes >= 60 {
                let hours = minutes / 60;
                let remaining_mins = minutes % 60;
                meta_parts.push(format!("{}:{:02}:{:02}", hours, remaining_mins, seconds));
            } else {
                meta_parts.push(format!("{}:{:02}", minutes, seconds));
            }
        }
        if !meta_parts.is_empty() {
            details = details.push(text(meta_parts.join(" • ")).size(11));
        }

        // Audio quality
        if let Some(ref quality) = album.audio_quality {
            details = details.push(text(fl!("quality-label", quality = quality.clone())).size(10));
        }

        // Explicit badge
        if album.explicit {
            details = details.push(text(fl!("explicit")).size(10));
        }

        // Row: cover + details
        let info_row = widget::Row::new()
            .push(cover)
            .push(details)
            .spacing(12)
            .align_y(Alignment::Center);

        let mut section = widget::Column::new().spacing(8).push(info_row);

        // Album review text below the cover row (same truncation as artist bio)
        if let Some(review) = &album.review
            && !review.is_empty()
        {
            let clean_review = strip_markup(review);
            let max_chars = max_description_chars(self.window_width);
            let char_count = clean_review.chars().count();
            let display_review = if char_count > max_chars {
                let truncated: String = clean_review.chars().take(max_chars).collect();
                if let Some(last_dot) = truncated
                    .rfind(". ")
                    .or_else(|| truncated.strip_suffix('.').map(|s| s.len()))
                {
                    let sentence_end = last_dot + 1;
                    if sentence_end >= max_chars / 3 {
                        truncated[..sentence_end].to_string()
                    } else {
                        format!("{}…", truncated)
                    }
                } else {
                    format!("{}…", truncated)
                }
            } else {
                clean_review
            };
            section = section.push(
                text(display_review)
                    .size(12)
                    .wrapping(Wrapping::WordOrGlyph),
            );
        }

        section.into()
    }
}
