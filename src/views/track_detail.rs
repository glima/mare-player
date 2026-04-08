// SPDX-License-Identifier: MIT

//! Track detail view for Maré Player.
//!
//! Shows recommendations seeded from a specific track, mirroring the
//! "track page" found in the TIDAL web/desktop client.  Three scrollable
//! list sections are rendered inside a single scroll container (just like
//! the artist detail view):
//!
//! 1. **More Albums by {Artist}** — the track artist's discography
//! 2. **Related Albums** — one album per similar artist
//! 3. **Related Artists** — artists similar to the track's artist

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::{
    ARTIST_PICTURE_SIZE, fading_header_title, fading_text_column, list_item, scrollable_list,
};

impl AppModel {
    /// Render the track detail view showing recommendations for the selected track.
    pub fn view_track_detail(&self) -> Element<'_, Message> {
        let fallback_track = fl!("fallback-track");
        let track_title = self
            .selected_detail_track
            .as_ref()
            .map(|t| t.title.as_str())
            .unwrap_or(&fallback_track);

        let fallback_artist = fl!("fallback-artist");
        let artist_name = self
            .selected_detail_track
            .as_ref()
            .map(|t| t.artist_name.as_str())
            .unwrap_or(&fallback_artist);

        // Header row: back button + track title
        let header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::NavigateBack)
                    .padding(4),
            )
            .push(fading_header_title(track_title))
            .spacing(8)
            .align_y(Alignment::Center);

        // Build scrollable content with all sections in one column
        let mut content_col = widget::Column::new().spacing(16).width(Length::Fill);

        // Track info summary (cover + title + artist + album)
        if let Some(track) = &self.selected_detail_track {
            content_col = content_col.push(self.view_track_detail_header(track));
        }

        // Section 1: More Albums by {Artist}
        if !self.track_detail_artist_albums.is_empty() {
            content_col =
                content_col.push(self.view_track_detail_artist_albums_section(artist_name));
        } else if self.is_loading {
            content_col = content_col.push(
                widget::Column::new()
                    .push(text(fl!("more-albums-by", artist = artist_name)).size(15))
                    .push(text(fl!("loading-recommendations")).size(12))
                    .spacing(6),
            );
        }

        // Section 2: Related Albums
        if !self.track_detail_related_albums.is_empty() {
            content_col = content_col.push(self.view_track_detail_related_albums_section());
        } else if !self.track_detail_related_artists.is_empty() {
            // Related artists arrived but albums are still loading
            content_col = content_col.push(
                widget::Column::new()
                    .push(text(fl!("related-albums")).size(15))
                    .push(text(fl!("loading-recommendations")).size(12))
                    .spacing(6),
            );
        }

        // Section 3: Related Artists
        if !self.track_detail_related_artists.is_empty() {
            content_col = content_col.push(self.view_track_detail_related_artists_section());
        } else if self.is_loading {
            content_col = content_col.push(
                widget::Column::new()
                    .push(text(fl!("related-artists")).size(15))
                    .push(text(fl!("loading-recommendations")).size(12))
                    .spacing(6),
            );
        }

        let scrollable_content = scrollable_list(content_col);

        widget::Column::new()
            .push(header)
            .push(scrollable_content)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }

    /// Track info header: cover art + title + artist (clickable) + album (clickable).
    fn view_track_detail_header(
        &self,
        track: &crate::tidal::models::Track,
    ) -> Element<'_, Message> {
        let cover: Element<'_, Message> = if let Some(url) = &track.cover_url
            && let Some(handle) = self.loaded_images.get(url)
        {
            cosmic::widget::image(handle.clone())
                .width(ARTIST_PICTURE_SIZE)
                .height(ARTIST_PICTURE_SIZE)
                .into()
        } else {
            widget::icon::from_name("media-optical-symbolic")
                .size(ARTIST_PICTURE_SIZE)
                .into()
        };

        let mut details = widget::Column::new().spacing(4);

        details = details.push(
            text(track.title.clone())
                .size(16)
                .wrapping(Wrapping::WordOrGlyph),
        );

        // Clickable artist name
        if let Some(artist_id) = &track.artist_id {
            details = details.push(
                button::custom(
                    text(track.artist_name.clone())
                        .size(13)
                        .wrapping(Wrapping::None),
                )
                .on_press(Message::ShowArtistDetail(artist_id.clone()))
                .width(Length::Shrink)
                .padding(0)
                .class(cosmic::theme::Button::MenuItem),
            );
        } else {
            details = details.push(text(track.artist_name.clone()).size(13));
        }

        // Clickable album name
        if let Some(album_name) = &track.album_name {
            if let Some(album_id) = &track.album_id {
                details = details.push(
                    button::custom(text(album_name.clone()).size(12).wrapping(Wrapping::None))
                        .on_press(Message::ShowAlbumDetailById(album_id.clone()))
                        .width(Length::Shrink)
                        .padding(0)
                        .class(cosmic::theme::Button::MenuItem),
                );
            } else {
                details = details.push(text(album_name.clone()).size(12));
            }
        }

        // Duration + quality badge
        let mut meta_parts: Vec<String> = vec![track.duration_display()];
        if let Some(ref quality) = track.audio_quality {
            meta_parts.push(quality.clone());
        }
        details = details.push(text(meta_parts.join(" • ")).size(11));

        widget::Row::new()
            .push(cover)
            .push(details)
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
    }

    /// Section: "More Albums by {Artist}" — album list with cover, title, year.
    fn view_track_detail_artist_albums_section(&self, artist_name: &str) -> Element<'_, Message> {
        let section_header = text(fl!("more-albums-by", artist = artist_name)).size(15);

        let album_items: Vec<Element<'_, Message>> = self
            .track_detail_artist_albums
            .iter()
            .map(|album| self.compact_album_row(album))
            .collect();

        widget::Column::new()
            .push(section_header)
            .push(
                widget::Column::with_children(album_items)
                    .spacing(2)
                    .width(Length::Fill),
            )
            .spacing(6)
            .into()
    }

    /// Section: "Related Albums" — one album per similar artist.
    fn view_track_detail_related_albums_section(&self) -> Element<'_, Message> {
        let section_header = text(fl!("related-albums")).size(15);

        let album_items: Vec<Element<'_, Message>> = self
            .track_detail_related_albums
            .iter()
            .map(|album| self.compact_album_row_with_artist(album))
            .collect();

        widget::Column::new()
            .push(section_header)
            .push(
                widget::Column::with_children(album_items)
                    .spacing(2)
                    .width(Length::Fill),
            )
            .spacing(6)
            .into()
    }

    /// Section: "Related Artists" — artist picture + name, clickable.
    fn view_track_detail_related_artists_section(&self) -> Element<'_, Message> {
        let section_header = text(fl!("related-artists")).size(15);

        let artist_items: Vec<Element<'_, Message>> = self
            .track_detail_related_artists
            .iter()
            .map(|artist| {
                let picture: Element<'_, Message> = if let Some(url) = &artist.picture_url
                    && let Some(handle) = self.loaded_images.get(url)
                {
                    cosmic::widget::image(handle.clone())
                        .width(40)
                        .height(40)
                        .into()
                } else {
                    widget::icon::from_name("avatar-default-symbolic")
                        .size(40)
                        .into()
                };

                let info_parts = fading_text_column(vec![
                    text(artist.name.clone())
                        .size(13)
                        .wrapping(Wrapping::None)
                        .into(),
                ]);

                let row_content = widget::Row::new()
                    .push(picture)
                    .push(info_parts)
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .width(Length::Fill);

                list_item(row_content, Message::ShowArtistDetail(artist.id.clone()), 6)
            })
            .collect();

        widget::Column::new()
            .push(section_header)
            .push(
                widget::Column::with_children(artist_items)
                    .spacing(2)
                    .width(Length::Fill),
            )
            .spacing(6)
            .into()
    }

    /// Compact album row for the "More Albums by {Artist}" section.
    ///
    /// Shows thumbnail, title, and release year + track count.
    /// Omits artist name (redundant — the section header already says it)
    /// and quality badge (too noisy for a recommendation list).
    fn compact_album_row(&self, album: &crate::tidal::models::Album) -> Element<'_, Message> {
        let album_clone = album.clone();

        let mut info_children: Vec<Element<'_, Message>> = vec![
            text(album.title.clone())
                .size(13)
                .wrapping(Wrapping::None)
                .into(),
        ];

        // Release year + track count
        let mut meta_parts: Vec<String> = Vec::new();
        if let Some(ref date) = album.release_date {
            let year = date.split('-').next().unwrap_or(date);
            meta_parts.push(year.to_string());
        }
        if album.num_tracks > 0 {
            meta_parts.push(fl!("track-count", count = album.num_tracks));
        }
        if !meta_parts.is_empty() {
            info_children.push(
                text(meta_parts.join(" • "))
                    .size(11)
                    .wrapping(Wrapping::None)
                    .into(),
            );
        }

        let info_parts = fading_text_column(info_children);

        let row_content = widget::Row::new()
            .push(self.thumbnail(album.cover_url.as_deref(), "media-optical-symbolic"))
            .push(info_parts)
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        list_item(row_content, Message::ShowAlbumDetail(album_clone), 6)
    }

    /// Album row for the "Related Albums" section.
    ///
    /// Like [`Self::compact_album_row`] but includes the artist name since
    /// related albums come from different artists.
    fn compact_album_row_with_artist(
        &self,
        album: &crate::tidal::models::Album,
    ) -> Element<'_, Message> {
        let album_clone = album.clone();

        let mut info_children: Vec<Element<'_, Message>> = vec![
            text(album.title.clone())
                .size(13)
                .wrapping(Wrapping::None)
                .into(),
        ];

        // Artist name (these are from different artists, so it's needed)
        info_children.push(
            text(album.artist_name.clone())
                .size(11)
                .wrapping(Wrapping::None)
                .into(),
        );

        // Release year + track count
        let mut meta_parts: Vec<String> = Vec::new();
        if let Some(ref date) = album.release_date {
            let year = date.split('-').next().unwrap_or(date);
            meta_parts.push(year.to_string());
        }
        if album.num_tracks > 0 {
            meta_parts.push(fl!("track-count", count = album.num_tracks));
        }
        if !meta_parts.is_empty() {
            info_children.push(
                text(meta_parts.join(" • "))
                    .size(11)
                    .wrapping(Wrapping::None)
                    .into(),
            );
        }

        let info_parts = fading_text_column(info_children);

        let row_content = widget::Row::new()
            .push(self.thumbnail(album.cover_url.as_deref(), "media-optical-symbolic"))
            .push(info_parts)
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        list_item(row_content, Message::ShowAlbumDetail(album_clone), 6)
    }
}
