// SPDX-License-Identifier: GPL-3.0-only

//! Domain-specific row builders for Maré Player list views.
//!
//! Each method on [`AppModel`] assembles a complete, clickable list-item row
//! for a particular domain object (track, album, playlist, menu entry).  They
//! all delegate to the composable helpers in [`super::list_helpers`] for
//! styling and to [`super::icons`] for icon handles, keeping this module
//! focused purely on *what* goes into each row.
//!
//! The free functions [`build_thumbnail`] and [`build_track_row`] contain the
//! core logic and can be called from virtual `List` closures that don't have
//! access to `&self`.  The corresponding [`AppModel`] methods delegate to them.

use std::collections::HashSet;
use std::sync::Arc;

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, container, icon, text};

use crate::fl;
use crate::messages::Message;
use crate::state::{AppModel, HandleCache};
use crate::tidal::models::{Album, Artist, Playlist, Track};

use super::constants::THUMBNAIL_SIZE;
use super::icons::RADIO_SVG;
use super::list_helpers::{TrackRowOptions, fading_text_column, list_item};

// =============================================================================
// Free-standing builders (usable without `&self`)
// =============================================================================

/// Standalone thumbnail builder for virtual `List` closures.
///
/// Returns an image element when the URL is present and already cached in
/// `loaded_images`, otherwise falls back to a named icon.  Each cache hit
/// also touches the LRU entry so visible items stay loaded.
pub(crate) fn build_thumbnail<'a>(
    loaded_images: &HandleCache,
    url: Option<&str>,
    fallback_icon: &'static str,
) -> Element<'a, Message> {
    if let Some(url) = url
        && let Some(handle) = loaded_images.get_or_request(url)
    {
        return cosmic::widget::image(handle.clone()).width(THUMBNAIL_SIZE).height(THUMBNAIL_SIZE).into();
    }
    widget::icon::from_name(fallback_icon).size(THUMBNAIL_SIZE).into()
}

/// Radio control shared by list rows and the now-playing bar.
///
/// A disabled button normally ignores mouse events, which would activate the
/// enclosing playback row. Capture both press and release without giving the
/// button an action, preserving its disabled styling and tooltip.
/// `compact` keeps the existing smaller padding in list rows.
pub(crate) fn build_track_radio_button<'a>(
    track: &Track,
    unavailable_radio_tracks: &HashSet<String>,
    compact: bool,
) -> Element<'a, Message> {
    let available = !unavailable_radio_tracks.contains(&track.id);
    let mut radio_icon = icon::from_svg_bytes(RADIO_SVG);
    radio_icon.symbolic = true;
    let radio_btn = button::icon(radio_icon)
        .tooltip(if available { fl!("tooltip-go-to-track-radio") } else { fl!("no-radio-tracks") })
        .on_press_maybe(available.then(|| Message::ShowTrackRadio(track.clone())));
    let radio_btn = if compact { radio_btn.extra_small().padding(2) } else { radio_btn.padding(4) };

    // Keep the widget structure stable when availability changes during a redraw.
    let area = widget::mouse_area(radio_btn);
    if available { area.into() } else { area.on_press(Message::Noop).on_release(Message::Noop).into() }
}

/// Standalone track row builder for virtual `List` closures.
///
/// This contains the full row-building logic. The [`AppModel::track_row`]
/// method delegates here.
pub(crate) fn build_track_row<'a>(
    loaded_images: &HandleCache,
    track: &Track,
    index: usize,
    opts: &TrackRowOptions,
) -> Element<'a, Message> {
    let base_thumbnail = build_thumbnail(loaded_images, track.cover_url.as_deref(), opts.fallback_icon);
    // Video entries get a small video emblem in the thumbnail's corner.
    let thumbnail: Element<'a, Message> = if track.is_video {
        let badge = container(widget::icon::from_name("emblem-videos-symbolic").size(12)).padding(2).class(
            cosmic::theme::Container::custom(|_theme| cosmic::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(cosmic::iced::Color::from_rgba(0.0, 0.0, 0.0, 0.6))),
                text_color: Some(cosmic::iced::Color::WHITE),
                border: cosmic::iced::Border { radius: [7.0; 4].into(), ..Default::default() },
                ..Default::default()
            }),
        );
        cosmic::iced::widget::Stack::new()
            .push(base_thumbnail)
            .push(
                container(badge)
                    .width(Length::Fixed(THUMBNAIL_SIZE as f32))
                    .height(Length::Fixed(THUMBNAIL_SIZE as f32))
                    .align_x(Alignment::End)
                    .align_y(Alignment::End),
            )
            .into()
    } else {
        base_thumbnail
    };

    let track_info = fading_text_column(vec![
        text(track.title.clone()).size(13).wrapping(Wrapping::None).into(),
        text(track.artist_name.clone()).size(11).wrapping(Wrapping::None).into(),
    ]);

    let duration = container(text(track.duration_display()).size(11).wrapping(Wrapping::None))
        .width(Length::Fixed(opts.duration_column_width()))
        .align_x(Alignment::End);

    // "Go to track radio" button — shows similar tracks for this track.
    // Hidden inside the track radio view (to prevent recursive radios) and for
    // videos (TIDAL has no track radio for them — the /tracks/{id}/mix
    // endpoint 404s on a video id).
    let trailing = if opts.show_radio_button && !track.is_video {
        let radio_btn = build_track_radio_button(track, &opts.unavailable_radio_tracks, true);

        widget::Row::new().push(radio_btn).push(duration).spacing(4).align_y(Alignment::Center).width(Length::Shrink)
    } else {
        widget::Row::new().push(duration).align_y(Alignment::Center).width(Length::Shrink)
    };

    let row = widget::Row::new()
        .push(thumbnail)
        .push(track_info)
        .push(trailing)
        .spacing(8)
        .padding([4, 8])
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let tracks_arc = Arc::clone(&opts.tracks);
    let source_clone = opts.source.clone();

    list_item(row, Message::PlayTrackList(tracks_arc, index, source_clone), 0)
}

/// Standalone album row builder for virtual `List` closures. The
/// [`AppModel::album_row`] method delegates here.
pub(crate) fn build_album_row<'a>(loaded_images: &HandleCache, album: &Album) -> Element<'a, Message> {
    let info = fading_text_column(vec![
        text(album.title.clone()).size(13).wrapping(Wrapping::None).into(),
        text(album.artist_name.clone()).size(11).wrapping(Wrapping::None).into(),
    ]);

    let row = widget::Row::new()
        .push(build_thumbnail(loaded_images, album.cover_url.as_deref(), "media-optical-symbolic"))
        .push(info)
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    list_item(row, Message::ShowAlbumDetail(album.clone()), 6)
}

/// Standalone artist row builder (picture + name + primary role) for virtual
/// `List` closures. Used by the Profiles view and search results; navigates
/// to the artist detail view on click.
pub(crate) fn build_profile_artist_row<'a>(loaded_images: &HandleCache, artist: &Artist) -> Element<'a, Message> {
    let mut info_children: Vec<Element<'_, Message>> = vec![text(artist.name.clone()).size(13).wrapping(Wrapping::None).into()];

    // Show primary role if available (e.g. "Artist", "Producer", "DJ")
    if let Some(role) = artist.roles.first() {
        info_children.push(text(role.clone()).size(11).wrapping(Wrapping::None).into());
    }

    let row = widget::Row::new()
        .push(build_thumbnail(loaded_images, artist.picture_url.as_deref(), "system-users-symbolic"))
        .push(fading_text_column(info_children))
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    list_item(row, Message::ShowArtistDetail(artist.id.clone()), 6)
}

// =============================================================================
// Row Builders (methods delegating to the free functions above)
// =============================================================================

impl AppModel {
    /// Get a thumbnail element - image if cached, otherwise fallback icon.
    /// Images are already circular from make_circular() processing.
    pub fn thumbnail<'a>(&self, url: Option<&str>, fallback_icon: &'static str) -> Element<'a, Message> {
        build_thumbnail(&self.loaded_images, url, fallback_icon)
    }

    /// Create a track row element for use in track lists.
    ///
    /// This is the **single source of truth** for rendering a track in any list
    /// (album detail, playlist detail, favorites, search results, artist top tracks, etc.).
    ///
    /// Returns a row with: thumbnail, track info (title + artist), duration,
    /// and optionally a radio button — all wrapped in [`list_item`].
    pub fn track_row<'a>(&self, track: &Track, index: usize, opts: &TrackRowOptions) -> Element<'a, Message> {
        build_track_row(&self.loaded_images, track, index, opts)
    }

    /// Create an album list-item element (thumbnail + title + artist).
    ///
    /// Used in the albums list, search results, and anywhere an album appears
    /// as a clickable row. Wraps content via [`list_item`].
    pub fn album_row<'a>(&self, album: &Album) -> Element<'a, Message> {
        build_album_row(&self.loaded_images, album)
    }

    /// Create a playlist list-item element (thumbnail + title + track count).
    ///
    /// Used in the playlists list and search results. Wraps content via
    /// [`list_item`].
    pub fn playlist_row<'a>(&self, playlist: &Playlist) -> Element<'a, Message> {
        let mut info_children: Vec<Element<'_, Message>> =
            vec![text(playlist.title.clone()).size(13).wrapping(Wrapping::None).into()];

        if playlist.num_tracks > 0 {
            info_children.push(text(fl!("track-count", count = playlist.num_tracks)).size(11).into());
        }

        let info = fading_text_column(info_children);

        // Prefer the 2×2 album-art grid thumbnail, fall back to the
        // playlist's own cover image, then to a generic icon.
        let thumb: Element<'_, Message> = if let Some(handle) = self.playlist_thumbnails.get(&playlist.uuid) {
            cosmic::widget::image(handle.clone()).width(THUMBNAIL_SIZE).height(THUMBNAIL_SIZE).into()
        } else {
            self.thumbnail(playlist.image_url.as_deref(), "folder-music-symbolic")
        };

        let row = widget::Row::new().push(thumb).push(info).spacing(8).align_y(Alignment::Center).width(Length::Fill);

        list_item(row, Message::ShowPlaylistDetail(playlist.uuid.clone(), playlist.title.clone()), 6)
    }

    /// Create a main-menu navigation row (icon + label + chevron).
    ///
    /// Used on the main collection screen for Playlists / Albums / Tracks.
    /// Wraps content via [`list_item`].
    pub fn menu_row<'a>(icon: &'static str, label: String, on_press: Message) -> Element<'a, Message> {
        let row = widget::Row::new()
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

// Exercise the real widgets with the CPU renderer, without a window or GPU.
// Both the default applet and --no-default-features standalone use this renderer.
#[cfg(all(test, not(feature = "wgpu")))]
mod tests {
    use super::{TrackRowOptions, build_track_radio_button, build_track_row};
    use crate::messages::Message;
    use crate::state::HandleCache;
    use crate::tidal::models::Track;
    use cosmic::Element;
    use cosmic::iced::core::{Event, Layout, Point, Rectangle, Shell, Size, clipboard, layout, mouse, widget::Tree};
    use std::collections::HashSet;
    use std::sync::Arc;

    fn track() -> Track {
        Track { id: "seed".into(), title: "Seed track".into(), ..Default::default() }
    }

    fn click(mut element: Element<'_, Message>, point: Point) -> (Vec<Message>, Vec<bool>) {
        let renderer = cosmic::Renderer::new(cosmic::iced::Font::default(), 14.0.into());
        let mut tree = Tree::new(element.as_widget());
        let limits = layout::Limits::new(Size::ZERO, Size::new(300.0, 100.0));
        let node = element.as_widget_mut().layout(&mut tree, &renderer, &limits);
        let viewport = Rectangle::new(Point::ORIGIN, Size::new(300.0, 100.0));
        let mut messages = Vec::new();
        let mut captured = Vec::new();
        for event in [mouse::Event::ButtonPressed(mouse::Button::Left), mouse::Event::ButtonReleased(mouse::Button::Left)] {
            let mut shell = Shell::new(&mut messages);
            element.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(event),
                Layout::new(&node),
                mouse::Cursor::Available(point),
                &renderer,
                &mut clipboard::Null,
                &mut shell,
                &viewport,
            );
            captured.push(shell.is_event_captured());
        }
        (messages, captured)
    }

    fn radio_in_playback_row(unavailable: HashSet<String>) -> (Element<'static, Message>, Point) {
        let track = track();
        let opts = TrackRowOptions {
            tracks: Arc::from([track.clone()]),
            unavailable_radio_tracks: Arc::new(unavailable),
            ..Default::default()
        };
        // The radio is immediately left of duration: right padding, duration
        // width, inter-control gap, then half the 20px compact radio button.
        let radio_center = Point::new(300.0 - 8.0 - opts.duration_column_width() - 4.0 - 10.0, 24.0);
        (build_track_row(&HandleCache::new(0), &track, 0, &opts), radio_center)
    }

    #[test]
    fn unknown_radio_remains_clickable_in_both_sizes() {
        // Learning another track's absence must not affect this track.
        let unavailable = HashSet::from(["other-track".to_string()]);
        for compact in [true, false] {
            let (messages, captured) = click(build_track_radio_button(&track(), &unavailable, compact), Point::new(10.0, 10.0));
            assert!(matches!(messages.as_slice(), [Message::ShowTrackRadio(track)] if track.id == "seed"));
            assert_eq!(captured, [true, true]);
        }
    }

    #[test]
    fn known_unavailable_radio_has_no_action_in_both_sizes() {
        let unavailable = HashSet::from(["seed".to_string()]);
        for compact in [true, false] {
            let (messages, captured) = click(build_track_radio_button(&track(), &unavailable, compact), Point::new(10.0, 10.0));
            assert!(matches!(messages.as_slice(), [Message::Noop, Message::Noop]));
            assert_eq!(captured, [true, true]);
        }
    }

    #[test]
    fn disabled_radio_captures_press_and_release_instead_of_playing_parent_row() {
        let unavailable = HashSet::from(["seed".to_string()]);
        let (row, radio_center) = radio_in_playback_row(unavailable);
        let (messages, captured) = click(row, radio_center);
        assert!(matches!(messages.as_slice(), [Message::Noop, Message::Noop]));
        assert_eq!(captured, [true, true]);
    }

    #[test]
    fn enabled_nested_radio_opens_radio_instead_of_playing_parent_row() {
        let (row, radio_center) = radio_in_playback_row(HashSet::new());
        let (messages, captured) = click(row, radio_center);
        assert!(matches!(messages.as_slice(), [Message::ShowTrackRadio(track)] if track.id == "seed"));
        assert_eq!(captured, [true, true]);
    }

    #[test]
    fn known_unavailable_track_still_plays_when_its_row_is_clicked() {
        let track = track();
        let opts = TrackRowOptions {
            tracks: Arc::from([track.clone()]),
            unavailable_radio_tracks: Arc::new(HashSet::from([track.id.clone()])),
            ..Default::default()
        };
        let row = build_track_row(&HandleCache::new(0), &track, 0, &opts);
        let (messages, captured) = click(row, Point::new(60.0, 20.0));
        assert!(
            matches!(messages.as_slice(), [Message::PlayTrackList(tracks, 0, None)] if tracks.first().is_some_and(|t| t.id == "seed"))
        );
        assert_eq!(captured, [true, true]);
    }
}
