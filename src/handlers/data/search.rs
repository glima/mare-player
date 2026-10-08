// SPDX-License-Identifier: GPL-3.0-only

//! Search message handlers for Maré Player.

use cosmic::prelude::*;

use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::models::{SearchCategory, SearchResults};

/// Results per category in the initial (all-category) search.
const SEARCH_LIMIT: u32 = 20;
/// Results per "load more" page of a single category.
const SEARCH_PAGE_LIMIT: u32 = 50;

// =============================================================================
// Task Helper Methods
// =============================================================================

impl AppModel {
    /// Perform a search query
    pub(crate) fn perform_search(&self, query: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        let key = format!("search:{query}");
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.search(&query, &SearchCategory::ALL, SEARCH_LIMIT, 0).await.map_err(|e| e.to_string())
                };
                if let Ok(ref results) = result {
                    crate::handlers::view_cache::cache_put(db, &key, results);
                }
                result
            },
            |result| cosmic::Action::App(Message::SearchComplete(result)),
        )
    }

    /// Rebuild the search view's virtual-`List` rows from `search_results`
    /// and `search_category` (via [`SearchResults::rows`]).
    ///
    /// `new_identity` replaces the rows and the list's widget identity, which
    /// also resets its scroll position — for a new result set or category.
    /// Otherwise (appending a page) the existing rows are edited in place:
    /// rows past the common prefix are removed and the rest pushed. That
    /// keeps your scroll position, and it must be incremental — a list that
    /// keeps its widget identity but gets a wholesale-replaced `Content`
    /// lays out with stale row heights (see the Explore view's tests).
    pub(crate) fn rebuild_search_rows(&mut self, new_identity: bool) {
        let results = self.search_results.as_ref();
        let rows = results.map(|r| r.rows(self.search_category)).unwrap_or_default();
        if new_identity {
            self.search_rows = rows.into_iter().collect();
            self.search_rows_revision = self.search_rows_revision.wrapping_add(1);
        } else {
            let keep = rows.iter().enumerate().take_while(|&(i, row)| self.search_rows.get(i) == Some(row)).count();
            while self.search_rows.len() > keep {
                self.search_rows.remove(self.search_rows.len() - 1);
            }
            rows.into_iter().skip(keep).for_each(|row| self.search_rows.push(row));
        }
        self.search_tracks_arc = results.map(|r| r.tracks.as_slice()).unwrap_or_default().into();
        self.search_videos_arc = results.map(|r| r.videos.as_slice()).unwrap_or_default().into();
    }
}

// =============================================================================
// Message Handlers
// =============================================================================

impl AppModel {
    /// Handle search query changed - debounces search requests
    pub fn handle_search_query_changed(&mut self, query: String) -> Task<cosmic::Action<Message>> {
        self.search_query = query.clone();

        // Clear results if query is empty
        if query.is_empty() {
            self.search_results = None;
            self.rebuild_search_rows(true);
            self.is_loading = false;
            return Task::none();
        }

        // Increment debounce version and schedule a debounced search
        self.search_debounce_version = self.search_debounce_version.wrapping_add(1);
        let version = self.search_debounce_version;

        // Schedule search after 300ms debounce delay
        Task::perform(
            async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                version
            },
            |v| cosmic::Action::App(Message::PerformSearchDebounced(v)),
        )
    }

    /// Handle debounced search execution
    pub fn handle_perform_search_debounced(&mut self, version: u64) -> Task<cosmic::Action<Message>> {
        // Only perform search if version matches (no newer keystrokes)
        if version == self.search_debounce_version && !self.search_query.is_empty() {
            self.is_loading = true;
            let q = self.search_query.clone();
            Task::batch([
                self.read_view_cache::<SearchResults, _>(format!("search:{q}"), |r| Message::SearchComplete(Ok(r))),
                self.perform_search(q),
            ])
        } else {
            Task::none()
        }
    }

    /// Handle immediate search execution
    pub fn handle_perform_search(&mut self) -> Task<cosmic::Action<Message>> {
        if !self.search_query.is_empty() {
            self.is_loading = true;
            let q = self.search_query.clone();
            Task::batch([
                self.read_view_cache::<SearchResults, _>(format!("search:{q}"), |r| Message::SearchComplete(Ok(r))),
                self.perform_search(q),
            ])
        } else {
            Task::none()
        }
    }

    /// Handle search complete
    pub fn handle_search_complete(&mut self, result: Result<SearchResults, String>) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(results) => {
                self.search_results = Some(results);
                self.rebuild_search_rows(true);
                // Rows are a virtual list: each visible row requests its own
                // cover lazily via get_or_request, so don't bulk-fetch covers.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Search failed: {}", e);
                self.error_message = Some(format!("Search failed: {}", e));
                Task::none()
            }
        }
    }

    /// Switch between the Top overview (`None`) and one category in full.
    pub fn handle_select_search_category(&mut self, category: Option<SearchCategory>) -> Task<cosmic::Action<Message>> {
        self.search_category = category;
        self.rebuild_search_rows(true);
        Task::none()
    }

    /// Fetch the next page of `category`, continuing from what's loaded.
    pub fn handle_load_more_search_results(&mut self, category: SearchCategory) -> Task<cosmic::Action<Message>> {
        let Some(results) = &self.search_results else { return Task::none() };
        if self.search_loading_more || !results.has_more(category) {
            return Task::none();
        }
        self.search_loading_more = true;

        let offset = results.len(category);
        let query = self.search_query.clone();
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.search(&query, &[category], SEARCH_PAGE_LIMIT, offset as u32).await.map_err(|e| e.to_string())
                };
                (query, result)
            },
            move |(query, result)| cosmic::Action::App(Message::MoreSearchResultsLoaded(query, category, offset, result)),
        )
    }

    /// Append a loaded page — unless the results moved on while it was in
    /// flight (a new query, or a fresh search reset the list), in which case
    /// the page no longer continues what's on screen and is dropped.
    pub fn handle_more_search_results_loaded(
        &mut self,
        query: String,
        category: SearchCategory,
        offset: usize,
        result: Result<SearchResults, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.search_loading_more = false;
        match result {
            Ok(page) => {
                if let Some(results) = self.search_results.as_mut()
                    && query == self.search_query
                    && results.len(category) == offset
                {
                    results.append_page(category, page);
                }
            }
            Err(e) => {
                tracing::error!("Loading more search results failed: {}", e);
                self.error_message = Some(format!("Search failed: {}", e));
            }
        }
        self.rebuild_search_rows(false);
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use crate::state::{AppModel, ViewState};
    use crate::tidal::models::{SearchCategory, SearchResults, SearchRow, SearchTotals, Track};
    use cosmic::Application;

    fn app() -> AppModel {
        let (app, startup) = AppModel::init(cosmic::Core::default(), ());
        drop(startup);
        app
    }

    fn tracks(range: std::ops::Range<usize>) -> Vec<Track> {
        range.map(|i| Track { id: i.to_string(), ..Default::default() }).collect()
    }

    fn page(range: std::ops::Range<usize>, total: u32) -> SearchResults {
        SearchResults {
            tracks: tracks(range),
            totals: SearchTotals { tracks: total, ..Default::default() },
            ..Default::default()
        }
    }

    fn rows(app: &AppModel) -> Vec<SearchRow> {
        (0..app.search_rows.len()).filter_map(|i| app.search_rows.get(i).copied()).collect()
    }

    /// A searched-for "beat" showing 20 of TIDAL's 120 track matches.
    fn searched() -> AppModel {
        let mut app = app();
        app.search_query = "beat".into();
        drop(app.handle_search_complete(Ok(page(0..20, 120))));
        app
    }

    #[tokio::test]
    async fn selecting_a_category_lists_it_in_full_with_a_fresh_identity() {
        let mut app = searched();
        assert_eq!(rows(&app).len(), 6, "Top previews a header and 5 tracks");
        let revision = app.search_rows_revision;

        drop(app.handle_select_search_category(Some(SearchCategory::Tracks)));
        let listed = rows(&app);
        assert_eq!(listed.len(), 21);
        assert_eq!(listed[20], SearchRow::LoadMore(SearchCategory::Tracks));
        assert_ne!(app.search_rows_revision, revision);
        assert_eq!(app.search_tracks_arc.len(), 20);
    }

    #[tokio::test]
    async fn load_more_appends_in_place_and_keeps_the_list_identity() {
        let mut app = searched();
        drop(app.handle_select_search_category(Some(SearchCategory::Tracks)));
        let revision = app.search_rows_revision;

        drop(app.handle_load_more_search_results(SearchCategory::Tracks));
        assert!(app.search_loading_more);
        // A second click while in flight doesn't start another fetch.
        drop(app.handle_load_more_search_results(SearchCategory::Tracks));

        drop(app.handle_more_search_results_loaded("beat".into(), SearchCategory::Tracks, 20, Ok(page(20..70, 120))));
        assert!(!app.search_loading_more);
        assert_eq!(app.search_rows_revision, revision, "appending must keep the scroll position");
        let results = app.search_results.as_ref().unwrap();
        assert_eq!(rows(&app), results.rows(Some(SearchCategory::Tracks)));
        assert_eq!(rows(&app).len(), 71);
        assert_eq!(app.search_tracks_arc.len(), 70);
        assert_eq!(app.search_tracks_arc[69].id, "69");
    }

    #[tokio::test]
    async fn pages_that_no_longer_continue_the_list_are_dropped() {
        let mut app = searched();
        drop(app.handle_select_search_category(Some(SearchCategory::Tracks)));

        // The query changed while the page was in flight.
        app.search_loading_more = true;
        drop(app.handle_more_search_results_loaded("beatles".into(), SearchCategory::Tracks, 20, Ok(page(20..70, 120))));
        assert!(!app.search_loading_more);
        assert_eq!(app.search_results.as_ref().unwrap().tracks.len(), 20);

        // A fresh search reset the list under it (offset no longer matches).
        drop(app.handle_more_search_results_loaded("beat".into(), SearchCategory::Tracks, 70, Ok(page(70..120, 120))));
        assert_eq!(app.search_results.as_ref().unwrap().tracks.len(), 20);
        assert_eq!(rows(&app).len(), 21);
    }

    #[tokio::test]
    async fn returning_to_search_restores_the_category_and_going_home_resets_it() {
        let mut app = searched();
        app.view_state = ViewState::Search;
        drop(app.handle_select_search_category(Some(SearchCategory::Tracks)));
        let revision = app.search_rows_revision;

        app.nav_stack.push(ViewState::Search);
        app.view_state = ViewState::AlbumDetail;
        drop(app.handle_navigate_back());
        assert_eq!(app.view_state, ViewState::Search);
        assert_eq!(app.search_category, Some(SearchCategory::Tracks));
        assert_eq!(rows(&app).len(), 21);
        assert_ne!(app.search_rows_revision, revision);

        app.handle_show_main();
        assert_eq!(app.search_category, None);
        assert!(app.search_rows.is_empty());
    }
}
