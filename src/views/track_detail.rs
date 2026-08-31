// SPDX-License-Identifier: GPL-3.0-only

//! Track detail view for Maré Player.
//!
//! Shows recommendations seeded from a specific track, mirroring the
//! "track page" found in the TIDAL web/desktop client. The track header plus
//! three recommendation sections are flattened into a single virtual `List`
//! (`track_detail_rows`) so only the rows visible in the viewport materialise
//! and their covers load lazily:
//!
//! 1. **More Albums by {Artist}** — the track artist's discography
//! 2. **Related Albums** — one album per similar artist
//! 3. **Related Artists** — artists similar to the track's artist

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, icon, text};

use crate::fl;
use crate::messages::Message;
use crate::state::{AppModel, HandleCache};
use crate::tidal::models::{Album, Artist, Track, TrackDetailRow};
use crate::views::components::rows::build_thumbnail;
use crate::views::components::{
    ARTIST_PICTURE_SIZE, CREDITS_SVG, back_button, fading_header_title, fading_text, fading_text_column, list_item,
    scrollable_element, virtual_list_row,
};

impl AppModel {
    /// Render the track detail view showing recommendations for the selected track.
    pub fn view_track_detail(&self) -> Element<'_, Message> {
        let fallback_track = fl!("fallback-track");
        let track_title = self.selected_detail_track.as_ref().map(|t| t.title.as_str()).unwrap_or(&fallback_track);

        // Header bar: back button + track title + credits action.
        //
        // Lyrics deliberately don't have a button here: the now-playing bar is
        // always on screen and surfaces its own lyrics icon whenever the
        // playing track has them, so a second entry point would be redundant.
        let header = widget::Row::new()
            .push(back_button(Message::NavigateBack))
            .push(fading_header_title(track_title))
            .push({
                // Credits action: who played on / produced this recording.
                // Only enabled when a track is selected.
                let track_for_credits = self.selected_detail_track.clone();
                let mut ci = icon::from_svg_bytes(CREDITS_SVG);
                ci.symbolic = true;
                let btn = button::icon(ci).tooltip(fl!("tooltip-show-credits")).padding(4);
                if let Some(track) = track_for_credits { btn.on_press(Message::ShowCredits(track)) } else { btn }
            })
            .spacing(8)
            .align_y(Alignment::Center);

        let content: Element<'_, Message> = if self.track_detail_rows.is_empty() {
            text(fl!("loading-recommendations")).size(14).into()
        } else {
            let loaded_images = &self.loaded_images;
            let list = cosmic::iced::widget::list::List::new(&self.track_detail_rows, move |_index, row| {
                build_track_detail_row(loaded_images, row)
            });
            scrollable_element(list)
        };

        widget::Column::new().push(header).push(content).spacing(12).padding(12).width(Length::Fill).into()
    }
}

/// Build a single track-detail row for the virtual `List` closure.
fn build_track_detail_row<'a>(loaded_images: &HandleCache, row: &TrackDetailRow) -> Element<'a, Message> {
    let inner: Element<'a, Message> = match row {
        TrackDetailRow::Header(track) => build_track_detail_header_row(loaded_images, track),
        TrackDetailRow::SectionHeader(title) => widget::container(text(title.clone()).size(15)).padding([8, 0, 2, 0]).into(),
        TrackDetailRow::Loading => text(fl!("loading-recommendations")).size(12).into(),
        TrackDetailRow::ArtistAlbum(album) => build_compact_album_row(loaded_images, album, false),
        TrackDetailRow::RelatedAlbum(album) => build_compact_album_row(loaded_images, album, true),
        TrackDetailRow::RelatedArtist(artist) => build_related_artist_row(loaded_images, artist),
    };
    virtual_list_row(inner, 4)
}

/// Track info header: cover art + title + clickable artist + clickable album + metadata.
fn build_track_detail_header_row<'a>(loaded_images: &HandleCache, track: &Track) -> Element<'a, Message> {
    let cover: Element<'a, Message> = if let Some(url) = &track.cover_url
        && let Some(handle) = loaded_images.get_or_request(url)
    {
        cosmic::widget::image(handle.clone()).width(ARTIST_PICTURE_SIZE).height(ARTIST_PICTURE_SIZE).into()
    } else {
        widget::icon::from_name("media-optical-symbolic").size(ARTIST_PICTURE_SIZE).into()
    };

    // Fill width so every child has the pane's right edge to fade against —
    // a Shrink column would size to its longest label and let it overflow the
    // popup with nothing to clip it.
    let mut details = widget::Column::new().spacing(4).width(Length::Fill);

    // The title is the one item that wraps (WordOrGlyph breaks even a single
    // unbroken word), so it can't overflow horizontally and needs no fade.
    details = details.push(text(track.title.clone()).size(16).wrapping(Wrapping::WordOrGlyph));

    // Clickable artist name. The fade goes *inside* the button: a button sets
    // its own text_color, which would override an outer FadingClip's alpha
    // ramp and leave a hard clip. Shrink width keeps the hover highlight
    // hugging short names.
    if let Some(artist_id) = &track.artist_id {
        details = details.push(
            button::custom(fading_text(text(track.artist_name.clone()).size(13).wrapping(Wrapping::None)))
                .on_press(Message::ShowArtistDetail(artist_id.clone()))
                .width(Length::Shrink)
                .padding(0)
                .class(cosmic::theme::Button::MenuItem),
        );
    } else {
        details =
            details.push(fading_text_column(vec![text(track.artist_name.clone()).size(13).wrapping(Wrapping::None).into()]));
    }

    // Clickable album name
    if let Some(album_name) = &track.album_name {
        if let Some(album_id) = &track.album_id {
            details = details.push(
                button::custom(fading_text(text(album_name.clone()).size(12).wrapping(Wrapping::None)))
                    .on_press(Message::ShowAlbumDetailById(album_id.clone()))
                    .width(Length::Shrink)
                    .padding(0)
                    .class(cosmic::theme::Button::MenuItem),
            );
        } else {
            details = details.push(fading_text_column(vec![text(album_name.clone()).size(12).wrapping(Wrapping::None).into()]));
        }
    }

    // Duration + quality badge
    let mut meta_parts: Vec<String> = vec![track.duration_display()];
    if let Some(ref quality) = track.audio_quality {
        meta_parts.push(quality.clone());
    }
    details = details.push(fading_text_column(vec![text(meta_parts.join(" • ")).size(11).wrapping(Wrapping::None).into()]));

    widget::Row::new().push(cover).push(details).spacing(12).align_y(Alignment::Center).into()
}

/// Compact album card for the recommendation sections. `with_artist` includes
/// the artist name (needed for "Related Albums", which span different artists;
/// omitted for "More Albums by {Artist}" where it's redundant).
fn build_compact_album_row<'a>(loaded_images: &HandleCache, album: &Album, with_artist: bool) -> Element<'a, Message> {
    let mut info_children: Vec<Element<'a, Message>> = vec![text(album.title.clone()).size(13).wrapping(Wrapping::None).into()];

    if with_artist {
        info_children.push(text(album.artist_name.clone()).size(11).wrapping(Wrapping::None).into());
    }

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
        info_children.push(text(meta_parts.join(" • ")).size(11).wrapping(Wrapping::None).into());
    }

    let row_content = widget::Row::new()
        .push(build_thumbnail(loaded_images, album.cover_url.as_deref(), "media-optical-symbolic"))
        .push(fading_text_column(info_children))
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    list_item(row_content, Message::ShowAlbumDetail(album.clone()), 6)
}

/// Related-artist card: picture + name, navigating to the artist detail view.
fn build_related_artist_row<'a>(loaded_images: &HandleCache, artist: &Artist) -> Element<'a, Message> {
    let picture: Element<'a, Message> = if let Some(url) = &artist.picture_url
        && let Some(handle) = loaded_images.get_or_request(url)
    {
        cosmic::widget::image(handle.clone()).width(40).height(40).into()
    } else {
        widget::icon::from_name("avatar-default-symbolic").size(40).into()
    };

    let info = fading_text_column(vec![text(artist.name.clone()).size(13).wrapping(Wrapping::None).into()]);

    let row_content = widget::Row::new().push(picture).push(info).spacing(8).align_y(Alignment::Center).width(Length::Fill);

    list_item(row_content, Message::ShowArtistDetail(artist.id.clone()), 6)
}
