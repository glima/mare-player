// SPDX-License-Identifier: GPL-3.0-only

//! Search view for Maré Player.
//!
//! Below the search bar, category chips switch between the Top overview (a
//! short preview of every category) and one category in full — every loaded
//! result plus "load more" while TIDAL has further matches. Both are the
//! same virtual `List` over [`SearchRow`]s (see [`SearchResults::rows`]).
//!
//! [`SearchResults::rows`]: crate::tidal::models::SearchResults::rows

use std::sync::Arc;

use crate::fl;
use cosmic::Element;
use cosmic::iced::widget::text_input;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::models::{PlaybackSource, SearchCategory, SearchResults, SearchRow};
use crate::views::components::rows::{build_album_row, build_profile_artist_row, build_track_row};
use crate::views::components::{TrackRowOptions, back_button, scrollable_element, virtual_list_row};

impl AppModel {
    /// Render the search view with search bar, category chips, and results.
    pub fn view_search(&self) -> Element<'_, Message> {
        // Inside a category, back returns to the Top overview first.
        let back_msg = if self.search_category.is_some() { Message::SelectSearchCategory(None) } else { Message::ShowMain };
        let header = widget::Row::new()
            .push(back_button(back_msg))
            .push(text(fl!("search")).size(18))
            .spacing(8)
            .align_y(Alignment::Center);

        let search_bar = widget::Row::new()
            .push(
                text_input(&fl!("search-placeholder"), &self.search_query)
                    .id("search-input")
                    .on_input(Message::SearchQueryChanged)
                    .on_submit(Message::PerformSearch)
                    .width(Length::Fill),
            )
            .push(button::icon(widget::icon::from_name("system-search-symbolic")).on_press(Message::PerformSearch).padding(4))
            .spacing(8)
            .align_y(Alignment::Center);

        let mut col = widget::Column::new().push(header).push(search_bar);

        // Chips stay put while a query exists, so they don't jump around
        // as results come and go between keystrokes.
        if !self.search_query.is_empty() {
            col = col.push(self.search_category_chips());
        }

        let results_content: Element<'_, Message> = if self.is_loading {
            text(fl!("searching")).size(14).into()
        } else if self.search_results.is_none() {
            text(fl!("enter-search-term")).size(14).into()
        } else if self.search_rows.is_empty() {
            text(fl!("no-results")).size(14).into()
        } else {
            let source = Some(PlaybackSource::ad_hoc(fl!("context-search")));
            let track_opts = TrackRowOptions {
                tracks: Arc::clone(&self.search_tracks_arc),
                source: source.clone(),
                ..self.track_row_options()
            };
            let video_opts = TrackRowOptions { tracks: Arc::clone(&self.search_videos_arc), source, ..self.track_row_options() };
            let list = cosmic::iced::widget::list::List::new(&self.search_rows, move |index, row| {
                virtual_list_row(self.search_row(index, *row, &track_opts, &video_opts), 4)
            });
            let id = cosmic::iced::core::widget::Id::new(format!("mare-search-rows-{}", self.search_rows_revision));
            widget::id_container(scrollable_element(list), id).into()
        };

        col.push(results_content).spacing(12).padding(12).width(Length::Fill).into()
    }

    /// The Top / per-category chip bar. Wraps onto a second line when the
    /// popup (or a translation) is too narrow to fit them all.
    fn search_category_chips(&self) -> Element<'_, Message> {
        let chip = |label: String, category: Option<SearchCategory>| -> Element<'_, Message> {
            let class =
                if self.search_category == category { cosmic::theme::Button::Suggested } else { cosmic::theme::Button::Standard };
            button::custom(text(label).size(12))
                .padding([4, 12])
                .class(class)
                .on_press(Message::SelectSearchCategory(category))
                .into()
        };
        let chips = std::iter::once(chip(fl!("search-top"), None))
            .chain(SearchCategory::ALL.into_iter().map(|c| chip(category_label(c), Some(c))))
            .collect();
        widget::flex_row(chips).spacing(6).into()
    }

    /// Build one search row. `index` is the row's position in the list.
    fn search_row<'a>(
        &'a self,
        index: usize,
        row: SearchRow,
        track_opts: &TrackRowOptions,
        video_opts: &TrackRowOptions,
    ) -> Element<'a, Message> {
        let Some(results) = &self.search_results else { return widget::space::horizontal().into() };
        match row {
            SearchRow::SectionHeader(category) => {
                let label = widget::Row::new()
                    .push(text(category_label(category)).size(12))
                    .push(widget::icon::from_name("go-next-symbolic").size(12))
                    .spacing(4)
                    .align_y(Alignment::Center);
                let header = button::custom(label)
                    .padding([2, 4])
                    .class(cosmic::theme::Button::Text)
                    .on_press(Message::SelectSearchCategory(Some(category)));
                // Gap above every section but the first.
                widget::container(header).padding([if index == 0 { 0 } else { 8 }, 0, 0, 0]).into()
            }
            SearchRow::Item(category, i) => self
                .search_item(results, category, i, track_opts, video_opts)
                .unwrap_or_else(|| widget::space::horizontal().into()),
            SearchRow::LoadMore(category) => {
                let (label, on_press) = if self.search_loading_more {
                    (fl!("loading"), None)
                } else {
                    (fl!("load-more"), Some(Message::LoadMoreSearchResults(category)))
                };
                widget::container(button::text(label).on_press_maybe(on_press)).center_x(Length::Fill).into()
            }
        }
    }

    /// The `i`-th result of `category`, or `None` for a transiently stale index
    /// (rows and results are rebuilt together, so this normally always hits).
    fn search_item<'a>(
        &'a self,
        results: &SearchResults,
        category: SearchCategory,
        i: usize,
        track_opts: &TrackRowOptions,
        video_opts: &TrackRowOptions,
    ) -> Option<Element<'a, Message>> {
        let images = &self.loaded_images;
        Some(match category {
            SearchCategory::Tracks => build_track_row(images, results.tracks.get(i)?, i, track_opts),
            SearchCategory::Videos => build_track_row(images, results.videos.get(i)?, i, video_opts),
            SearchCategory::Artists => build_profile_artist_row(images, results.artists.get(i)?),
            SearchCategory::Albums => build_album_row(images, results.albums.get(i)?),
            SearchCategory::Playlists => self.playlist_row(results.playlists.get(i)?),
        })
    }
}

fn category_label(category: SearchCategory) -> String {
    match category {
        SearchCategory::Tracks => fl!("tracks"),
        SearchCategory::Artists => fl!("artists"),
        SearchCategory::Albums => fl!("albums"),
        SearchCategory::Playlists => fl!("playlists"),
        SearchCategory::Videos => fl!("videos"),
    }
}
