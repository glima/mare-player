// SPDX-License-Identifier: MIT

//! Artist detail view for Maré Player.
//!
//! Shows artist picture, bio, popularity, top tracks, and discography (albums).
//! Navigable from the now-playing bar or search results.

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::helpers::max_description_chars;
use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::{
    ARTIST_PICTURE_SIZE, TrackRowOptions, fading_header_title, fading_text_column,
    favorite_icon_handle, list_item, scrollable_list,
};

impl AppModel {
    /// Render the artist detail view showing picture, bio, top tracks, and albums.
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

        // Build scrollable content
        let mut content_col = widget::Column::new().spacing(12).width(Length::Fill);

        if self.is_loading && self.selected_artist.is_none() {
            content_col = content_col.push(text(fl!("loading-artist")).size(14));
        }

        // Artist info section (picture + details)
        if let Some(artist) = &self.selected_artist {
            content_col = content_col.push(self.view_artist_info_section(artist));
        }

        // Top tracks section
        if !self.selected_artist_top_tracks.is_empty() {
            content_col = content_col.push(self.view_artist_top_tracks_section());
        }

        // Discography (albums) section
        if !self.selected_artist_albums.is_empty() {
            content_col = content_col.push(self.view_artist_albums_section());
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

    /// Render the artist info section: picture, name, popularity, roles, bio.
    fn view_artist_info_section(
        &self,
        artist: &crate::tidal::models::Artist,
    ) -> Element<'_, Message> {
        // Artist picture (large)
        let picture: Element<'_, Message> = if let Some(url) = &artist.picture_url
            && let Some(handle) = self.loaded_images.get(url)
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

        // Roles (e.g., "Artist, Producer")
        if !artist.roles.is_empty() {
            // Deduplicate roles
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

        // Popularity bar
        if let Some(popularity) = artist.popularity {
            details =
                details.push(text(fl!("popularity", value = popularity.to_string())).size(11));
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
            // Strip any HTML tags from the bio (TIDAL sometimes includes them)
            let clean_bio = strip_markup(bio);
            let max_chars = max_description_chars(self.window_width);
            let char_count = clean_bio.chars().count();
            let display_bio = if char_count > max_chars {
                // Try to break at the last sentence-ending full stop within the limit
                let truncated: String = clean_bio.chars().take(max_chars).collect();
                if let Some(last_dot) = truncated.rfind(". ").or_else(|| {
                    // Also accept a period right at the end of the truncated region
                    truncated.strip_suffix('.').map(|s| s.len())
                }) {
                    // Only use the sentence break if it keeps a reasonable amount of text
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

    /// Render the top tracks section for the artist detail view.
    fn view_artist_top_tracks_section(&self) -> Element<'_, Message> {
        let section_header = text(fl!("top-tracks")).size(15);

        let all_tracks = self.selected_artist_top_tracks.clone();
        let artist_name = self
            .selected_artist
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let context = Some(fl!("artist-top-tracks-context", artist = artist_name));

        let track_items: Vec<Element<'_, Message>> = self
            .selected_artist_top_tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                self.track_row(
                    track,
                    index,
                    &TrackRowOptions {
                        tracks: &all_tracks,
                        context: context.clone(),
                        ..Default::default()
                    },
                )
            })
            .collect();

        widget::Column::new()
            .push(section_header)
            .push(
                widget::Column::with_children(track_items)
                    .spacing(2)
                    .width(Length::Fill),
            )
            .spacing(6)
            .into()
    }

    /// Render the discography (albums) section for the artist detail view.
    fn view_artist_albums_section(&self) -> Element<'_, Message> {
        let section_header = text(fl!("discography")).size(15);

        let album_items: Vec<Element<'_, Message>> = self
            .selected_artist_albums
            .iter()
            .map(|album| {
                let album_clone = album.clone();

                let mut info_children: Vec<Element<'_, Message>> = vec![
                    text(album.title.clone())
                        .size(13)
                        .wrapping(Wrapping::None)
                        .into(),
                ];

                // Release date + track count
                let mut meta_parts: Vec<String> = Vec::new();
                if let Some(ref date) = album.release_date {
                    // Show just the year if it looks like a full date
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

                let info_parts = fading_text_column(info_children);

                let row_content = widget::Row::new()
                    .push(self.thumbnail(album.cover_url.as_deref(), "media-optical-symbolic"))
                    .push(info_parts)
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .width(Length::Fill);

                list_item(row_content, Message::ShowAlbumDetail(album_clone), 6)
            })
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
