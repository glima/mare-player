// SPDX-License-Identifier: MIT

//! Feed view for Maré Player.
//!
//! Shows new releases from followed artists, grouped by time period and
//! flattened into a single virtual `List` (`feed_content`) so only the rows
//! visible in the viewport materialise and their covers load lazily.

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::messages::Message;
use crate::state::{AppModel, HandleCache};
use crate::tidal::models::{FeedActivity, FeedItem, FeedRow};
use crate::views::components::rows::{build_album_row, build_thumbnail};
use crate::views::components::{
    back_button, fading_text_column, list_item, scrollable_element, virtual_list_row,
};

impl AppModel {
    /// Render the feed view showing new releases grouped by time period.
    pub fn view_feed(&self) -> Element<'_, Message> {
        let header = widget::Row::new()
            .push(back_button(Message::ShowMain))
            .push(text(fl!("feed")).size(18))
            .push(widget::space::horizontal())
            .spacing(8)
            .align_y(Alignment::Center);

        let content: Element<'_, Message> = if self.feed_content.is_empty() {
            if self.is_loading {
                text(fl!("loading")).size(14).into()
            } else {
                widget::Column::new()
                    .push(text(fl!("no-feed")).size(14))
                    .push(button::text(fl!("refresh")).on_press(Message::LoadFeed))
                    .spacing(8)
                    .into()
            }
        } else {
            let loaded_images = &self.loaded_images;
            let list =
                cosmic::iced::widget::list::List::new(&self.feed_content, move |_index, row| {
                    build_feed_row(loaded_images, row)
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

/// Build a single feed row (time-period header or activity) for the virtual `List`.
fn build_feed_row<'a>(loaded_images: &HandleCache, row: &FeedRow) -> Element<'a, Message> {
    let inner: Element<'a, Message> = match row {
        FeedRow::SectionHeader(title) => widget::container(text(title.clone()).size(14))
            .padding([8, 0, 2, 0])
            .into(),
        FeedRow::Activity(activity) => build_feed_activity_row(loaded_images, activity),
    };
    virtual_list_row(inner, 4)
}

/// Build a single feed activity row (new album release or history mix).
fn build_feed_activity_row<'a>(
    loaded_images: &HandleCache,
    activity: &FeedActivity,
) -> Element<'a, Message> {
    match &activity.item {
        FeedItem::AlbumRelease(album) => build_album_row(loaded_images, album),
        FeedItem::HistoryMix {
            id,
            title,
            subtitle,
            image_url,
        } => {
            let info = fading_text_column(vec![
                text(title.clone()).size(13).wrapping(Wrapping::None).into(),
                text(subtitle.clone())
                    .size(11)
                    .wrapping(Wrapping::None)
                    .into(),
            ]);

            let row = widget::Row::new()
                .push(build_thumbnail(
                    loaded_images,
                    image_url.as_deref(),
                    "media-playlist-shuffle-symbolic",
                ))
                .push(info)
                .spacing(8)
                .align_y(Alignment::Center)
                .width(Length::Fill);

            list_item(row, Message::ShowMixDetail(id.clone(), title.clone()), 6)
        }
    }
}
