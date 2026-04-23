// SPDX-License-Identifier: MIT

//! Feed view for Maré Player.
//!
//! Shows new releases from followed artists, grouped by time period.

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::models::{FeedActivity, FeedItem};
use crate::views::components::constants::THUMBNAIL_SIZE;
use crate::views::components::{fading_text_column, list_item, scrollable_list};

impl AppModel {
    /// Render the feed view showing new releases grouped by time period.
    pub fn view_feed(&self) -> Element<'_, Message> {
        let header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::ShowMain)
                    .padding(4),
            )
            .push(text(fl!("feed")).size(18))
            .push(widget::space::horizontal())
            .spacing(8)
            .align_y(Alignment::Center);

        let content: Element<'_, Message> = if self.feed_activities.is_empty() {
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
            // Group activities by time period
            let now = chrono::Utc::now();
            let mut new_updates: Vec<&FeedActivity> = Vec::new();
            let mut last_week: Vec<&FeedActivity> = Vec::new();
            let mut last_month: Vec<&FeedActivity> = Vec::new();
            let mut older: Vec<&FeedActivity> = Vec::new();

            for activity in &self.feed_activities {
                if let Ok(date) = chrono::DateTime::parse_from_rfc3339(&activity.occurred_at) {
                    let age = now.signed_duration_since(date);
                    if age.num_days() <= 2 {
                        new_updates.push(activity);
                    } else if age.num_days() <= 7 {
                        last_week.push(activity);
                    } else if age.num_days() <= 30 {
                        last_month.push(activity);
                    } else {
                        older.push(activity);
                    }
                } else {
                    older.push(activity);
                }
            }

            let mut list_col = widget::Column::new().spacing(4);

            if !new_updates.is_empty() {
                list_col = list_col.push(text(fl!("feed-new-updates")).size(14));
                for activity in &new_updates {
                    list_col = list_col.push(self.feed_activity_row(activity));
                }
            }

            if !last_week.is_empty() {
                list_col = list_col.push(widget::Space::new().height(8));
                list_col = list_col.push(text(fl!("feed-last-week")).size(14));
                for activity in &last_week {
                    list_col = list_col.push(self.feed_activity_row(activity));
                }
            }

            if !last_month.is_empty() {
                list_col = list_col.push(widget::Space::new().height(8));
                list_col = list_col.push(text(fl!("feed-last-month")).size(14));
                for activity in &last_month {
                    list_col = list_col.push(self.feed_activity_row(activity));
                }
            }

            if !older.is_empty() {
                list_col = list_col.push(widget::Space::new().height(8));
                list_col = list_col.push(text(fl!("feed-older")).size(14));
                for activity in &older {
                    list_col = list_col.push(self.feed_activity_row(activity));
                }
            }

            scrollable_list(list_col)
        };

        widget::Column::new()
            .push(header)
            .push(content)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }

    /// Build a single feed activity row.
    fn feed_activity_row<'a>(&self, activity: &FeedActivity) -> Element<'a, Message> {
        match &activity.item {
            FeedItem::AlbumRelease(album) => self.album_row(album),
            FeedItem::HistoryMix {
                id,
                title,
                subtitle,
                image_url,
            } => {
                let thumbnail: Element<'_, Message> = if let Some(url) = image_url {
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

                let info = fading_text_column(vec![
                    text(title.clone()).size(13).wrapping(Wrapping::None).into(),
                    text(subtitle.clone())
                        .size(11)
                        .wrapping(Wrapping::None)
                        .into(),
                ]);

                let row = widget::Row::new()
                    .push(thumbnail)
                    .push(info)
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .width(Length::Fill);

                list_item(row, Message::ShowMixDetail(id.clone(), title.clone()), 6)
            }
        }
    }
}
