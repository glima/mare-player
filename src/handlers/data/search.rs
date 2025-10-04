// SPDX-License-Identifier: MIT

//! Search message handlers for Maré Player.

use cosmic::prelude::*;

use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::models::SearchResults;

// =============================================================================
// Task Helper Methods
// =============================================================================

impl AppModel {
    /// Perform a search query
    pub(crate) fn perform_search(&self, query: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client.search(&query, 20).await.map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::SearchComplete(result)),
        )
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
    pub fn handle_perform_search_debounced(
        &mut self,
        version: u64,
    ) -> Task<cosmic::Action<Message>> {
        // Only perform search if version matches (no newer keystrokes)
        if version == self.search_debounce_version && !self.search_query.is_empty() {
            self.is_loading = true;
            self.perform_search(self.search_query.clone())
        } else {
            Task::none()
        }
    }

    /// Handle immediate search execution
    pub fn handle_perform_search(&mut self) -> Task<cosmic::Action<Message>> {
        if !self.search_query.is_empty() {
            self.is_loading = true;
            self.perform_search(self.search_query.clone())
        } else {
            Task::none()
        }
    }

    /// Handle search complete
    pub fn handle_search_complete(
        &mut self,
        result: Result<SearchResults, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(results) => {
                // Collect image URLs to load from search results
                let mut urls: Vec<String> = Vec::new();
                urls.extend(results.tracks.iter().filter_map(|t| t.cover_url.clone()));
                urls.extend(results.albums.iter().filter_map(|a| a.cover_url.clone()));
                urls.extend(results.playlists.iter().filter_map(|p| p.image_url.clone()));
                self.search_results = Some(results);
                self.load_images_for_urls(urls)
            }
            Err(e) => {
                tracing::error!("Search failed: {}", e);
                self.error_message = Some(format!("Search failed: {}", e));
                Task::none()
            }
        }
    }
}
