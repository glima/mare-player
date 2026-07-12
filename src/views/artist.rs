// SPDX-License-Identifier: MIT

//! Artist detail view for Maré Player.
//!
//! Shows artist picture, bio, popularity, top tracks, music videos, and
//! discography — flattened into a single virtual `List` (`artist_rows`) so only
//! the rows visible in the viewport materialise and their covers load lazily.
//! Navigable from the now-playing bar or search results.

use std::sync::Arc;

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::helpers::max_description_chars;
use crate::messages::Message;
use crate::state::{AppModel, HandleCache};
use crate::tidal::models::{Album, Artist, ArtistRow, PlaybackSource, Track};
use crate::views::components::rows::{build_thumbnail, build_track_row};
use crate::views::components::{
    ARTIST_PICTURE_SIZE, TrackRowOptions, fading_header_title, fading_text_column,
    favorite_icon_handle, list_item, scrollable_element, virtual_list_row,
};

impl AppModel {
    /// Render the artist detail view as a single virtual list of heterogeneous
    /// rows (info block, section headers, tracks, videos, album cards).
    pub fn view_artist_detail(&self) -> Element<'_, Message> {
        let fallback_artist = fl!("fallback-artist");
        let artist_name = self
            .selected_artist
            .as_ref()
            .map(|a| a.name.as_str())
            .unwrap_or(&fallback_artist);

        // Header row: back button, title, follow heart
        let mut header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::NavigateBack)
                    .padding(4),
            )
            .push(fading_header_title(artist_name));

        // Follow/unfollow heart for the artist (pushed to far right)
        if let Some(artist) = &self.selected_artist {
            let is_followed = self.followed_artist_ids.contains(&artist.id);
            let tooltip = if is_followed {
                fl!("tooltip-unfollow-artist")
            } else {
                fl!("tooltip-follow-artist")
            };
            header = header.push(
                button::icon(favorite_icon_handle(is_followed))
                    .tooltip(tooltip)
                    .on_press(Message::ToggleFollowArtist(artist.clone()))
                    .padding(4),
            );
        }

        let header = header.spacing(8).align_y(Alignment::Center);

        let content: Element<'_, Message> = if self.artist_rows.is_empty() {
            text(fl!("loading-artist")).size(14).into()
        } else {
            let loaded_images = &self.loaded_images;
            let window_width = self.window_width;

            // Per-section playback context: clicking a top track / video plays
            // that section's list starting at the clicked index.
            let source = self.selected_artist.as_ref().map(|a| {
                PlaybackSource::artist(
                    a.id.clone(),
                    fl!("artist-top-tracks-context", artist = a.name.clone()),
                )
            });
            let top_tracks: Arc<[Track]> = self.selected_artist_top_tracks.clone().into();
            let videos: Arc<[Track]> = self.selected_artist_videos.clone().into();
            let top_opts = TrackRowOptions {
                tracks: Arc::clone(&top_tracks),
                source: source.clone(),
                ..Default::default()
            };
            let video_opts = TrackRowOptions {
                tracks: Arc::clone(&videos),
                source,
                ..Default::default()
            };

            let list =
                cosmic::iced::widget::list::List::new(&self.artist_rows, move |_index, row| {
                    build_artist_row(
                        loaded_images,
                        window_width,
                        &top_tracks,
                        &top_opts,
                        &videos,
                        &video_opts,
                        row,
                    )
                });
            scrollable_element(list)
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

/// Build a single artist-detail row for the virtual `List` closure.
#[allow(clippy::too_many_arguments)]
fn build_artist_row<'a>(
    loaded_images: &HandleCache,
    window_width: f32,
    top_tracks: &[Track],
    top_opts: &TrackRowOptions,
    videos: &[Track],
    video_opts: &TrackRowOptions,
    row: &ArtistRow,
) -> Element<'a, Message> {
    let inner: Element<'a, Message> = match row {
        ArtistRow::Info(artist) => build_artist_info_row(loaded_images, artist, window_width),
        ArtistRow::SectionHeader(title) => widget::container(text(title.clone()).size(15))
            .padding([8, 0, 2, 0])
            .into(),
        // `.get()` guards against a transiently stale index (rows and the
        // backing vecs are rebuilt together, so this normally always hits).
        ArtistRow::TopTrack(i) => match top_tracks.get(*i) {
            Some(track) => build_track_row(loaded_images, track, *i, top_opts),
            None => widget::space::horizontal().into(),
        },
        ArtistRow::Video(i) => match videos.get(*i) {
            Some(video) => build_track_row(loaded_images, video, *i, video_opts),
            None => widget::space::horizontal().into(),
        },
        ArtistRow::Album(album) => build_artist_album_row(loaded_images, album),
    };
    virtual_list_row(inner, 4)
}

/// Build the artist info block: picture, roles, popularity, and bio.
fn build_artist_info_row<'a>(
    loaded_images: &HandleCache,
    artist: &Artist,
    window_width: f32,
) -> Element<'a, Message> {
    // Artist picture (large)
    let picture: Element<'a, Message> = if let Some(url) = &artist.picture_url
        && let Some(handle) = loaded_images.get_or_request(url)
    {
        cosmic::widget::image(handle.clone())
            .width(ARTIST_PICTURE_SIZE)
            .height(ARTIST_PICTURE_SIZE)
            .into()
    } else {
        widget::icon::from_name("avatar-default-symbolic")
            .size(ARTIST_PICTURE_SIZE)
            .into()
    };

    // Details column next to the picture
    let mut details = widget::Column::new().spacing(4);

    // Roles (e.g., "Artist, Producer"), deduplicated
    if !artist.roles.is_empty() {
        let mut seen = std::collections::HashSet::new();
        let unique_roles: Vec<&str> = artist
            .roles
            .iter()
            .filter(|r| seen.insert(r.as_str()))
            .map(|r| r.as_str())
            .collect();
        let roles_text = unique_roles.join(", ");
        details = details.push(text(roles_text).size(12).wrapping(Wrapping::WordOrGlyph));
    }

    // Popularity
    if let Some(popularity) = artist.popularity {
        details = details.push(text(fl!("popularity", value = popularity.to_string())).size(11));
    }

    // Top row: picture + details side by side
    let info_row = widget::Row::new()
        .push(picture)
        .push(details)
        .spacing(12)
        .align_y(Alignment::Center);

    let mut section = widget::Column::new().spacing(8).push(info_row);

    // Bio text below the picture row
    if let Some(bio) = &artist.bio
        && !bio.is_empty()
    {
        let clean_bio = strip_markup(bio);
        let max_chars = max_description_chars(window_width);
        let char_count = clean_bio.chars().count();
        let display_bio = if char_count > max_chars {
            let truncated: String = clean_bio.chars().take(max_chars).collect();
            if let Some(last_dot) = truncated
                .rfind(". ")
                .or_else(|| truncated.strip_suffix('.').map(|s| s.len()))
            {
                let sentence_end = last_dot + 1; // include the '.'
                if sentence_end >= max_chars / 3 {
                    truncated[..sentence_end].to_string()
                } else {
                    format!("{}…", truncated)
                }
            } else {
                format!("{}…", truncated)
            }
        } else {
            clean_bio
        };
        section = section.push(text(display_bio).size(12).wrapping(Wrapping::WordOrGlyph));
    }

    section.into()
}

/// Build a single discography album card (thumbnail + title + meta + quality).
fn build_artist_album_row<'a>(loaded_images: &HandleCache, album: &Album) -> Element<'a, Message> {
    let mut info_children: Vec<Element<'a, Message>> = vec![
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

    // Quality badge
    if let Some(ref quality) = album.audio_quality {
        info_children.push(
            text(quality.clone())
                .size(10)
                .wrapping(Wrapping::None)
                .into(),
        );
    }

    let row_content = widget::Row::new()
        .push(build_thumbnail(
            loaded_images,
            album.cover_url.as_deref(),
            "media-optical-symbolic",
        ))
        .push(fading_text_column(info_children))
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    list_item(row_content, Message::ShowAlbumDetail(album.clone()), 6)
}

/// Strip HTML tags and TIDAL's custom `[wimpLink ...]...[/wimpLink]` markup
/// from bio text, keeping only the visible content between link tags.
pub(crate) fn strip_markup(input: &str) -> String {
    // First strip [wimpLink ...] and [/wimpLink] bracket tags
    let mut s = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '[' {
            // Consume everything up to and including the closing ']'
            let mut tag = String::new();
            for inner in chars.by_ref() {
                if inner == ']' {
                    break;
                }
                tag.push(inner);
            }
            // If it's NOT a wimpLink open/close tag, preserve it literally
            if !tag.starts_with("wimpLink") && !tag.starts_with("/wimpLink") {
                s.push('[');
                s.push_str(&tag);
                s.push(']');
            }
        } else {
            s.push(ch);
        }
    }

    // Then strip HTML tags (<...>)
    let mut result = String::with_capacity(s.len());
    let mut inside_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => result.push(ch),
            _ => {}
        }
    }
    result
}
