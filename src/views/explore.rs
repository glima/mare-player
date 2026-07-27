// SPDX-License-Identifier: MIT

//! Explore view for Maré Player.
//!
//! Renders TIDAL's browse pages (`/v1/pages/{path}`): a Featured carousel,
//! plus clouds of links (Genres, Moods & Activities, Decades, More) and
//! content lists (albums/playlists/artists).  Link clouds drill down into
//! sub-pages recursively; an in-view back button pops the stack.
//!
//! Rows are rendered through the virtual `List` widget — only the rows
//! visible in the viewport are materialised — so long browse pages (the
//! root Explore page alone has ~70 entries) scroll smoothly.

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::messages::Message;
use crate::state::{AppModel, HandleCache};
use crate::tidal::models::{ExploreRow, ExploreTarget};
use crate::views::components::rows::build_thumbnail;
use crate::views::components::{back_button, fading_text_column, list_item, scrollable_element, virtual_list_row};

impl AppModel {
    /// Render the Explore view for the currently-loaded browse page.
    pub fn view_explore(&self) -> Element<'_, Message> {
        // Back goes up the explore stack if we drilled into a sub-page,
        // otherwise out to the main collection menu.
        let back_msg = if self.explore_stack.len() > 1 { Message::ExploreBack } else { Message::ShowMain };

        let title = self.explore_page.as_ref().map(|p| p.title.clone()).unwrap_or_else(|| fl!("explore"));

        let header = widget::Row::new()
            .push(back_button(back_msg))
            .push(text(title).size(18))
            .push(widget::space::horizontal())
            .spacing(8)
            .align_y(Alignment::Center);

        let content: Element<'_, Message> = if self.explore_rows.is_empty() {
            if self.explore_loading {
                text(fl!("loading")).size(14).into()
            } else {
                widget::Column::new()
                    .push(text(fl!("no-explore")).size(14))
                    .push(button::text(fl!("refresh")).on_press(Message::LoadExplorePage("explore".to_string())))
                    .spacing(8)
                    .into()
            }
        } else {
            let loaded_images = &self.loaded_images;
            let list = cosmic::iced::widget::list::List::new(&self.explore_rows, move |_index, row| {
                build_explore_row(loaded_images, row)
            });
            scrollable_element(list)
        };

        widget::Column::new().push(header).push(content).spacing(12).padding(12).width(Length::Fill).into()
    }
}

/// Build a single Explore row for the virtual `List` closure.
fn build_explore_row<'a>(loaded_images: &HandleCache, row: &ExploreRow) -> Element<'a, Message> {
    // The virtual `List` keeps spacing at 0 (its `spacing()` is buggy — see
    // `virtual_list_row`); the old `spacing(4)` gap is baked in below instead.
    let inner: Element<'a, Message> = match row {
        ExploreRow::SectionHeader(title) => widget::container(text(title.clone()).size(15)).padding([8, 0, 2, 0]).into(),

        ExploreRow::Featured(card) => {
            let thumb = build_thumbnail(loaded_images, card.image_url.as_deref(), "view-list-symbolic");
            let mut texts: Vec<Element<'_, Message>> = vec![text(card.title.clone()).size(13).wrapping(Wrapping::None).into()];
            if let Some(sub) = card.subtitle.as_ref().filter(|s| !s.trim().is_empty()) {
                texts.push(text(sub.clone()).size(11).wrapping(Wrapping::None).into());
            }
            let info = fading_text_column(texts);
            let r = widget::Row::new().push(thumb).push(info).spacing(8).align_y(Alignment::Center).width(Length::Fill);
            list_item(r, Message::OpenExploreTarget(card.target.clone()), 6)
        }

        ExploreRow::Link(link) => {
            let r = widget::Row::new()
                .push(text(link.text.clone()).size(13))
                .push(widget::space::horizontal())
                .push(widget::icon::from_name("go-next-symbolic").size(14))
                .align_y(Alignment::Center)
                .width(Length::Fill);
            list_item(r, Message::OpenExploreTarget(ExploreTarget::Page(link.path.clone())), 10)
        }

        ExploreRow::Album(album) => {
            let thumb = build_thumbnail(loaded_images, album.cover_url.as_deref(), "media-optical-symbolic");
            let info = fading_text_column(vec![
                text(album.title.clone()).size(13).wrapping(Wrapping::None).into(),
                text(album.artist_name.clone()).size(11).wrapping(Wrapping::None).into(),
            ]);
            let r = widget::Row::new().push(thumb).push(info).spacing(8).align_y(Alignment::Center).width(Length::Fill);
            list_item(r, Message::ShowAlbumDetail(album.clone()), 6)
        }

        ExploreRow::Playlist(playlist) => {
            let thumb = build_thumbnail(loaded_images, playlist.image_url.as_deref(), "folder-music-symbolic");
            let info = fading_text_column(vec![text(playlist.title.clone()).size(13).wrapping(Wrapping::None).into()]);
            let r = widget::Row::new().push(thumb).push(info).spacing(8).align_y(Alignment::Center).width(Length::Fill);
            list_item(r, Message::ShowPlaylistDetail(playlist.uuid.clone(), playlist.title.clone()), 6)
        }

        ExploreRow::Artist(artist) => {
            let thumb = build_thumbnail(loaded_images, artist.picture_url.as_deref(), "system-users-symbolic");
            let info = fading_text_column(vec![text(artist.name.clone()).size(13).wrapping(Wrapping::None).into()]);
            let r = widget::Row::new().push(thumb).push(info).spacing(8).align_y(Alignment::Center).width(Length::Fill);
            list_item(r, Message::ShowArtistDetail(artist.id.clone()), 6)
        }
    };

    virtual_list_row(inner, 4)
}
