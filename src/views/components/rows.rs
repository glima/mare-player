// SPDX-License-Identifier: MIT

//! Domain-specific row builders for Maré Player list views.
//!
//! Each method on [`AppModel`] assembles a complete, clickable list-item row
//! for a particular domain object (track, album, playlist, menu entry).  They
//! all delegate to the composable helpers in [`super::list_helpers`] for
//! styling and to [`super::icons`] for icon handles, keeping this module
//! focused purely on *what* goes into each row.

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, container, icon, text};

use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::models::{Album, Playlist, Track};

use super::constants::THUMBNAIL_SIZE;
use super::icons::RADIO_SVG;
use super::list_helpers::{TrackRowOptions, fading_text_column, list_item};

// =============================================================================
// Row Builders
// =============================================================================

impl AppModel {
    /// Get a thumbnail element - image if cached, otherwise fallback icon.
    /// Images are already circular from make_circular() processing.
    pub fn thumbnail<'a>(
        &self,
        url: Option<&str>,
        fallback_icon: &'static str,
    ) -> Element<'a, Message> {
        if let Some(url) = url
            && let Some(handle) = self.loaded_images.get(url)
        {
            return cosmic::widget::image(handle.clone())
                .width(THUMBNAIL_SIZE)
                .height(THUMBNAIL_SIZE)
                .into();
        }
        widget::icon::from_name(fallback_icon)
            .size(THUMBNAIL_SIZE)
            .into()
    }

    /// Create a track row element for use in track lists.
    ///
    /// This is the **single source of truth** for rendering a track in any list
    /// (album detail, playlist detail, favorites, search results, artist top tracks, etc.).
    ///
    /// Returns a row with: thumbnail, track info (title + artist), duration,
    /// and optionally a favorite toggle button — all wrapped in [`list_item`].
    pub fn track_row<'a>(
        &self,
        track: &Track,
        index: usize,
        opts: &TrackRowOptions<'_>,
    ) -> Element<'a, Message> {
        let thumbnail = self.thumbnail(track.cover_url.as_deref(), opts.fallback_icon);

        let track_info = fading_text_column(vec![
            text(track.title.clone())
                .size(13)
                .wrapping(Wrapping::None)
                .into(),
            text(track.artist_name.clone())
                .size(11)
                .wrapping(Wrapping::None)
                .into(),
        ]);

        let duration = container(
            text(track.duration_display())
                .size(11)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fixed(opts.duration_column_width()))
        .align_x(Alignment::End);

        // "Go to track radio" button — shows similar tracks for this track
        // Hidden inside the track radio view to prevent recursive radios.
        let trailing = if opts.show_radio_button {
            let mut radio_icon = icon::from_svg_bytes(RADIO_SVG);
            radio_icon.symbolic = true;
            let radio_btn = button::icon(radio_icon)
                .extra_small()
                .tooltip(fl!("tooltip-go-to-track-radio"))
                .on_press(Message::ShowTrackRadio(track.clone()))
                .padding(2);

            widget::row()
                .push(radio_btn)
                .push(duration)
                .spacing(4)
                .align_y(Alignment::Center)
                .width(Length::Shrink)
        } else {
            widget::row()
                .push(duration)
                .align_y(Alignment::Center)
                .width(Length::Shrink)
        };

        let row = widget::row()
            .push(thumbnail)
            .push(track_info)
            .push(trailing)
            .spacing(8)
            .padding([4, 8])
            .align_y(Alignment::Center)
            .width(Length::Fill);

        let tracks_clone: Vec<Track> = opts.tracks.to_vec();
        let context_clone = opts.context.clone();

        list_item(
            row,
            Message::PlayTrackList(tracks_clone, index, context_clone),
            0,
        )
    }

    /// Create an album list-item element (thumbnail + title + artist).
    ///
    /// Used in the albums list, search results, and anywhere an album appears
    /// as a clickable row. Wraps content via [`list_item`].
    pub fn album_row<'a>(&self, album: &Album) -> Element<'a, Message> {
        let info = fading_text_column(vec![
            text(album.title.clone())
                .size(13)
                .wrapping(Wrapping::None)
                .into(),
            text(album.artist_name.clone())
                .size(11)
                .wrapping(Wrapping::None)
                .into(),
        ]);

        let row = widget::row()
            .push(self.thumbnail(album.cover_url.as_deref(), "media-optical-symbolic"))
            .push(info)
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        list_item(row, Message::ShowAlbumDetail(album.clone()), 6)
    }

    /// Create a playlist list-item element (thumbnail + title + track count).
    ///
    /// Used in the playlists list and search results. Wraps content via
    /// [`list_item`].
    pub fn playlist_row<'a>(&self, playlist: &Playlist) -> Element<'a, Message> {
        let mut info_children: Vec<Element<'_, Message>> = vec![
            text(playlist.title.clone())
                .size(13)
                .wrapping(Wrapping::None)
                .into(),
        ];

        if playlist.num_tracks > 0 {
            info_children.push(
                text(fl!("track-count", count = playlist.num_tracks))
                    .size(11)
                    .into(),
            );
        }

        let info = fading_text_column(info_children);

        // Prefer the 2×2 album-art grid thumbnail, fall back to the
        // playlist's own cover image, then to a generic icon.
        let thumb: Element<'_, Message> =
            if let Some(handle) = self.playlist_thumbnails.get(&playlist.uuid) {
                cosmic::widget::image(handle.clone())
                    .width(THUMBNAIL_SIZE)
                    .height(THUMBNAIL_SIZE)
                    .into()
            } else {
                self.thumbnail(playlist.image_url.as_deref(), "folder-music-symbolic")
            };

        let row = widget::row()
            .push(thumb)
            .push(info)
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        list_item(
            row,
            Message::ShowPlaylistDetail(playlist.uuid.clone(), playlist.title.clone()),
            6,
        )
    }

    /// Create a main-menu navigation row (icon + label + chevron).
    ///
    /// Used on the main collection screen for Playlists / Albums / Tracks.
    /// Wraps content via [`list_item`].
    pub fn menu_row<'a>(
        icon: &'static str,
        label: String,
        on_press: Message,
    ) -> Element<'a, Message> {
        let row = widget::row()
            .push(widget::icon::from_name(icon).size(24))
            .push(text(label).size(14))
            .push(widget::space::horizontal())
            .push(widget::icon::from_name("go-next-symbolic").size(16))
            .spacing(12)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        list_item(row, on_press, 10)
    }
}
