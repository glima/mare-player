// SPDX-License-Identifier: MIT

//! Navigation message handlers for Maré Player.
//!
//! This module handles view state transitions and navigation between screens.
//! Detail pages (album, artist, mix, track detail) push the current view onto
//! `nav_stack` before entering, and `NavigateBack` pops the stack to return.
//! This supports arbitrarily deep chains like
//! Main → Album → Artist → Album → back → back → …

#[cfg(feature = "panel-applet")]
use cosmic::iced::Limits;
#[cfg(feature = "panel-applet")]
use cosmic::iced::platform_specific::shell::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::window::Id;
use cosmic::prelude::*;
use cosmic::widget::text_input;
use std::sync::LazyLock;

use crate::tidal::models::{Album, Track};

use crate::messages::Message;
use crate::state::{AppModel, ViewState};

/// Static ID for the search input widget
pub(crate) static SEARCH_INPUT_ID: LazyLock<cosmic::widget::Id> =
    LazyLock::new(|| cosmic::widget::Id::new("search-input"));

impl AppModel {
    /// Rebuild the virtual track list for the history view, applying the
    /// current filter if active.
    pub(crate) fn rebuild_history_track_list(&mut self) {
        let all_tracks = self.play_history.tracks();
        let tracks: Vec<_> = if self.history_filter_visible && !self.history_filter_query.is_empty()
        {
            let query = self.history_filter_query.to_lowercase();
            all_tracks
                .into_iter()
                .filter(|t| {
                    t.title.to_lowercase().contains(&query)
                        || t.artist_name.to_lowercase().contains(&query)
                        || t.album_name
                            .as_deref()
                            .is_some_and(|a| a.to_lowercase().contains(&query))
                })
                .collect()
        } else {
            all_tracks
        };
        self.set_track_list(tracks);
    }

    /// Rebuild the virtual track list for the favorite tracks view,
    /// applying the current filter if active.
    pub(crate) fn rebuild_favorites_track_list(&mut self) {
        let all_tracks = self.user_favorite_tracks.clone();
        let tracks: Vec<_> = if self.favorite_tracks_filter_visible
            && !self.favorite_tracks_filter_query.is_empty()
        {
            let query = self.favorite_tracks_filter_query.to_lowercase();
            all_tracks
                .into_iter()
                .filter(|t| {
                    t.title.to_lowercase().contains(&query)
                        || t.artist_name.to_lowercase().contains(&query)
                        || t.album_name
                            .as_deref()
                            .is_some_and(|a| a.to_lowercase().contains(&query))
                })
                .collect()
        } else {
            all_tracks
        };
        self.set_track_list(tracks);
    }

    // =========================================================================
    // Popup lifecycle (panel-applet) / window lifecycle (standalone)
    // =========================================================================

    /// Handle popup toggle (panel-applet) or no-op (standalone).
    #[cfg(feature = "panel-applet")]
    pub fn handle_toggle_popup(&mut self) -> Task<cosmic::Action<Message>> {
        if let Some(p) = self.popup.take() {
            destroy_popup(p)
        } else if let Some(main_window_id) = self.core.main_window_id() {
            let new_id = Id::unique();
            self.popup.replace(new_id);
            let mut popup_settings =
                self.core
                    .applet
                    .get_popup_settings(main_window_id, new_id, None, None, None);
            popup_settings.positioner.size_limits = Limits::NONE
                .max_width(400.0)
                .min_width(350.0)
                .min_height(300.0)
                .max_height(600.0);
            get_popup(popup_settings)
        } else {
            Task::none()
        }
    }

    /// In standalone mode there is no popup to toggle — this is a no-op.
    #[cfg(not(feature = "panel-applet"))]
    pub fn handle_toggle_popup(&mut self) -> Task<cosmic::Action<Message>> {
        Task::none()
    }

    /// Handle popup closed event (panel-applet only).
    #[cfg(feature = "panel-applet")]
    pub fn handle_popup_closed(&mut self, id: Id) {
        if self.popup.as_ref() == Some(&id) {
            self.popup = None;
        }
    }

    /// In standalone mode there is no popup — this is a no-op.
    #[cfg(not(feature = "panel-applet"))]
    pub fn handle_popup_closed(&mut self, _id: Id) {}

    // =========================================================================
    // Top-level views (clear the nav stack — these are roots)
    // =========================================================================

    /// Handle show main view
    pub fn handle_show_main(&mut self) {
        self.nav_stack.clear();
        self.view_state = ViewState::Main;
        self.search_query.clear();
        self.search_results = None;
        self.history_filter_visible = false;
        self.history_filter_query.clear();
        self.favorite_tracks_filter_visible = false;
        self.favorite_tracks_filter_query.clear();
    }

    /// Handle show search view
    pub fn handle_show_search(&mut self) -> Task<cosmic::Action<Message>> {
        self.nav_stack.clear();
        self.view_state = ViewState::Search;
        text_input::focus(SEARCH_INPUT_ID.clone())
    }

    /// Handle show settings view
    pub fn handle_show_settings(&mut self) -> Task<cosmic::Action<Message>> {
        self.nav_stack.clear();
        self.view_state = ViewState::Settings;

        // Trigger loading of the profile picture if we have one
        let client = self.tidal_client.blocking_lock();
        if let crate::tidal::auth::AuthState::Authenticated { profile } = client.auth_state()
            && let Some(pic_url) = &profile.picture_url
        {
            let urls = vec![pic_url.clone()];
            drop(client);
            return self.load_images_for_urls(urls);
        }
        drop(client);
        Task::none()
    }

    /// Handle show playlists view
    pub fn handle_show_playlists(&mut self) -> Task<cosmic::Action<Message>> {
        self.nav_stack.clear();
        self.view_state = ViewState::Playlists;
        if self.user_playlists.is_empty() {
            self.is_loading = true;
            // Paint the last-seen playlists instantly from cache while the
            // network refreshes them in the background.
            Task::batch([
                self.read_view_cache::<Vec<crate::tidal::models::Playlist>, _>(
                    "library:playlists",
                    |p| Message::PlaylistsLoaded(Ok(p)),
                ),
                self.load_playlists(),
            ])
        } else {
            // Playlists already loaded — generate any missing grid thumbnails
            Task::done(cosmic::Action::App(Message::GeneratePlaylistThumbnails))
        }
    }

    /// Handle show albums view
    pub fn handle_show_albums(&mut self) -> Task<cosmic::Action<Message>> {
        self.nav_stack.clear();
        self.view_state = ViewState::Albums;
        if self.user_albums.is_empty() {
            self.is_loading = true;
            Task::batch([
                self.read_view_cache::<Vec<crate::tidal::models::Album>, _>(
                    "library:albums",
                    |a| Message::AlbumsLoaded(Ok(a)),
                ),
                self.load_albums(),
            ])
        } else {
            Task::none()
        }
    }

    /// Handle show favorite tracks view
    pub fn handle_show_favorite_tracks(&mut self) -> Task<cosmic::Action<Message>> {
        self.nav_stack.clear();
        self.view_state = ViewState::FavoriteTracks;
        if self.user_favorite_tracks.is_empty() {
            self.is_loading = true;
            Task::batch([
                self.read_view_cache::<Vec<crate::tidal::models::Track>, _>(
                    "favorites:tracks",
                    |t| Message::FavoriteTracksLoaded(Ok(t)),
                ),
                self.load_favorite_tracks(),
            ])
        } else {
            self.rebuild_favorites_track_list();
            Task::none()
        }
    }

    /// Handle show mixes & radio view
    pub fn handle_show_mixes(&mut self) -> Task<cosmic::Action<Message>> {
        self.nav_stack.clear();
        self.view_state = ViewState::Mixes;
        if self.user_mixes.is_empty() {
            self.is_loading = true;
            Task::batch([
                self.read_view_cache::<Vec<crate::tidal::models::Mix>, _>("library:mixes", |m| {
                    Message::MixesLoaded(Ok(m))
                }),
                self.load_mixes(),
            ])
        } else {
            self.load_images_for_mixes()
        }
    }

    /// Handle show play history view
    pub fn handle_show_history(&mut self) -> Task<cosmic::Action<Message>> {
        self.nav_stack.clear();
        self.view_state = ViewState::History;

        // Populate virtual track list for this view
        self.rebuild_history_track_list();

        // Eager-preload covers for the first ~30 history entries (enough to
        // populate the initial viewport without a network round-trip per
        // visible row).  Beyond that, `HandleCache::get_or_request` lazy-loads
        // covers as rows scroll into view — critical for users whose history
        // has accumulated hundreds or thousands of entries, where eager-
        // preloading everything would saturate the CDN and the LRU cache.
        const HISTORY_EAGER_PRELOAD: usize = 30;
        let urls: Vec<String> = (0..self.track_list_content.len().min(HISTORY_EAGER_PRELOAD))
            .filter_map(|i| {
                self.track_list_content
                    .get(i)
                    .and_then(|t| t.cover_url.clone())
            })
            .collect();
        if urls.is_empty() {
            Task::none()
        } else {
            self.load_images_for_urls(urls)
        }
    }

    /// Handle show followed artists (profiles) view
    pub fn handle_show_profiles(&mut self) -> Task<cosmic::Action<Message>> {
        self.nav_stack.clear();
        self.view_state = ViewState::Profiles;
        if self.user_followed_artists.is_empty() {
            self.is_loading = true;
            Task::batch([
                self.read_view_cache::<Vec<crate::tidal::models::Artist>, _>("profiles", |a| {
                    Message::ProfilesLoaded(Ok(a))
                }),
                self.load_profiles(),
            ])
        } else {
            self.load_images_for_profiles()
        }
    }

    /// Handle show feed view (new releases from followed artists)
    pub fn handle_show_feed(&mut self) -> Task<cosmic::Action<Message>> {
        self.nav_stack.clear();
        self.view_state = ViewState::Feed;
        if self.feed_activities.is_empty() {
            self.is_loading = true;
            Task::batch([
                self.read_view_cache::<Vec<crate::tidal::models::FeedActivity>, _>("feed", |f| {
                    Message::FeedLoaded(Ok(f))
                }),
                self.load_feed(),
            ])
        } else {
            Task::none()
        }
    }

    /// Handle show Explore view (TIDAL browse pages).
    ///
    /// Resets the in-view back stack to the root "explore" page and loads
    /// it (always re-fetched — the page is time-sensitive featured content).
    pub fn handle_show_explore(&mut self) -> Task<cosmic::Action<Message>> {
        self.nav_stack.clear();
        self.view_state = ViewState::Explore;
        self.explore_stack = vec!["explore".to_string()];
        self.explore_page = None;
        self.explore_loading = true;
        self.load_explore_page("explore")
    }

    // =========================================================================
    // List-level views (push parent onto the stack)
    // =========================================================================

    /// Handle show playlist detail view
    pub fn handle_show_playlist_detail(
        &mut self,
        uuid: String,
        name: String,
    ) -> Task<cosmic::Action<Message>> {
        self.nav_stack.push(self.view_state.clone());
        self.selected_playlist_name = Some(name);
        self.selected_playlist_uuid = Some(uuid.clone());
        self.selected_playlist_tracks.clear();
        self.view_state = ViewState::PlaylistDetail;
        let cache_read = self.read_view_cache::<Vec<crate::tidal::models::Track>, _>(
            format!("playlist:{uuid}:tracks"),
            |t| Message::PlaylistTracksLoaded(Ok(t)),
        );
        Task::batch([cache_read, self.load_playlist_tracks(uuid)])
    }

    /// Handle show mix detail view (tracks in a mix)
    pub fn handle_show_mix_detail(
        &mut self,
        mix_id: String,
        mix_name: String,
    ) -> Task<cosmic::Action<Message>> {
        self.nav_stack.push(self.view_state.clone());
        self.selected_mix_name = Some(mix_name);
        self.selected_mix_id = Some(mix_id.clone());
        self.selected_mix_tracks.clear();
        self.is_loading = true;
        self.view_state = ViewState::MixDetail;
        let cache_read = self.read_view_cache::<Vec<crate::tidal::models::Track>, _>(
            format!("mix:{mix_id}:tracks"),
            |t| Message::MixTracksLoaded(Ok(t)),
        );
        Task::batch([cache_read, self.load_mix_tracks(mix_id)])
    }

    /// Handle show track radio view (similar tracks based on a seed track)
    pub fn handle_show_track_radio(&mut self, track: Track) -> Task<cosmic::Action<Message>> {
        self.nav_stack.push(self.view_state.clone());
        self.selected_radio_source_track = Some(track.clone());
        self.selected_radio_tracks.clear();
        self.selected_radio_mix_id = None;
        self.is_loading = true;
        self.view_state = ViewState::TrackRadio;
        self.load_track_radio(track.id)
    }

    /// Handle show lyrics view for a specific track.
    ///
    /// Pushes the nav stack, resets prior lyrics state, switches to the
    /// `Lyrics` view, and kicks off the async fetch.  The view renders
    /// a loading state until `TrackLyricsLoaded` arrives.
    pub fn handle_show_lyrics(&mut self, track: Track) -> Task<cosmic::Action<Message>> {
        self.nav_stack.push(self.view_state.clone());
        let track_id = track.id.clone();
        self.selected_lyrics_track = Some(track);
        self.selected_track_lyrics = None;
        self.current_lyric_index = None;
        self.view_state = ViewState::Lyrics;
        // Paint last-seen lyrics from the DB instantly, then refresh from TIDAL.
        Task::batch([
            self.read_view_cache::<crate::tidal::models::TrackLyrics, _>(
                format!("lyrics:{track_id}"),
                |l| Message::TrackLyricsLoaded(Ok(l)),
            ),
            self.load_track_lyrics(track_id),
        ])
    }

    /// Handle show track detail view (recommendations seeded from a track).
    ///
    /// Loads three recommendation sections in parallel:
    /// 1. More albums by the track's artist (`get_artist_albums`)
    /// 2. Related/similar artists (`get_similar_artists`)
    ///
    /// Related albums are derived in a second pass once similar artists arrive
    /// (see `handle_track_detail_related_artists_loaded`).
    pub fn handle_show_track_detail(&mut self, track: Track) -> Task<cosmic::Action<Message>> {
        self.nav_stack.push(self.view_state.clone());

        // Clear previous data
        self.track_detail_artist_albums.clear();
        self.track_detail_related_artists.clear();
        self.track_detail_related_albums.clear();
        self.selected_detail_track = Some(track.clone());
        self.is_loading = true;
        self.view_state = ViewState::TrackDetail;

        let artist_id = track.artist_id.clone().unwrap_or_default();
        if artist_id.is_empty() {
            self.is_loading = false;
            return Task::none();
        }

        // Kick off artist-albums and similar-artists in parallel
        let albums_task = self.load_track_detail_artist_albums(artist_id.clone());
        let similar_task = self.load_track_detail_related_artists(artist_id);

        // Pre-load the track's own cover art so the header looks right
        let image_task = if let Some(url) = &track.cover_url {
            self.load_images_for_urls(vec![url.clone()])
        } else {
            Task::none()
        };

        Task::batch([albums_task, similar_task, image_task])
    }

    // =========================================================================
    // Detail views (push current view, then enter)
    // =========================================================================

    /// Handle show album detail view (from favorites list where we already have the Album)
    pub fn handle_show_album_detail(&mut self, album: Album) -> Task<cosmic::Action<Message>> {
        self.nav_stack.push(self.view_state.clone());
        let album_id = album.id.clone();
        self.selected_album = Some(album);
        self.selected_album_tracks.clear();
        self.view_state = ViewState::AlbumDetail;
        // Paint last-seen tracks instantly from cache, then fetch tracks +
        // album review in parallel (review is best-effort).
        let cache_read = self.read_view_cache::<Vec<crate::tidal::models::Track>, _>(
            format!("album:{album_id}:tracks"),
            |t| Message::AlbumTracksLoaded(Ok(t)),
        );
        let review_read = self
            .read_view_cache::<String, _>(format!("album:{album_id}:review"), |r| {
                Message::AlbumReviewLoaded(Ok(r))
            });
        let tracks_task = self.load_album_tracks(album_id.clone());
        let review_task = self.load_album_review(album_id);
        Task::batch([cache_read, review_read, tracks_task, review_task])
    }

    /// Handle show album detail by ID (from now-playing bar or artist view)
    pub fn handle_show_album_detail_by_id(
        &mut self,
        album_id: String,
    ) -> Task<cosmic::Action<Message>> {
        self.nav_stack.push(self.view_state.clone());
        self.selected_album = None;
        self.selected_album_tracks.clear();
        self.is_loading = true;
        self.view_state = ViewState::AlbumDetail;

        // Load album info; paint last-seen tracks from cache, then refresh.
        let client1 = self.tidal_client.clone();
        let id1 = album_id.clone();

        let info_task = Task::perform(
            async move {
                let client = client1.lock().await;
                client.get_album_info(&id1).await.map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::AlbumInfoLoaded(result)),
        );

        let cache_read = self.read_view_cache::<Vec<crate::tidal::models::Track>, _>(
            format!("album:{album_id}:tracks"),
            |t| Message::AlbumTracksLoaded(Ok(t)),
        );
        let review_read = self
            .read_view_cache::<String, _>(format!("album:{album_id}:review"), |r| {
                Message::AlbumReviewLoaded(Ok(r))
            });
        let tracks_task = self.load_album_tracks(album_id);

        Task::batch(vec![info_task, cache_read, review_read, tracks_task])
    }

    /// Handle show artist detail view
    pub fn handle_show_artist_detail(
        &mut self,
        artist_id: String,
    ) -> Task<cosmic::Action<Message>> {
        self.nav_stack.push(self.view_state.clone());
        self.selected_artist = None;
        self.selected_artist_top_tracks.clear();
        self.selected_artist_albums.clear();
        self.is_loading = true;
        self.view_state = ViewState::ArtistDetail;

        // Load artist info, top tracks, and albums in parallel, caching each
        // payload on success and painting the last-seen versions instantly.
        let client1 = self.tidal_client.clone();
        let client2 = self.tidal_client.clone();
        let client3 = self.tidal_client.clone();
        let db1 = self.cache_db.clone();
        let db2 = self.cache_db.clone();
        let db3 = self.cache_db.clone();
        let id1 = artist_id.clone();
        let id2 = artist_id.clone();
        let id3 = artist_id.clone();

        let info_task = Task::perform(
            async move {
                let key = format!("artist:{id1}:info");
                let result = {
                    let client = client1.lock().await;
                    client
                        .get_artist_info(&id1)
                        .await
                        .map_err(|e| e.to_string())
                };
                if let Ok(ref artist) = result {
                    crate::handlers::view_cache::cache_put(db1, &key, artist).await;
                }
                result
            },
            |result| cosmic::Action::App(Message::ArtistInfoLoaded(result)),
        );

        let tracks_task = Task::perform(
            async move {
                let key = format!("artist:{id2}:toptracks");
                let result = {
                    let client = client2.lock().await;
                    client
                        .get_artist_top_tracks(&id2, Some(20))
                        .await
                        .map_err(|e| e.to_string())
                };
                if let Ok(ref tracks) = result {
                    crate::handlers::view_cache::cache_put(db2, &key, tracks).await;
                }
                result
            },
            |result| cosmic::Action::App(Message::ArtistTopTracksLoaded(result)),
        );

        let albums_task = Task::perform(
            async move {
                let key = format!("artist:{id3}:albums");
                let result = {
                    let client = client3.lock().await;
                    client
                        .get_artist_albums(&id3, Some(50))
                        .await
                        .map_err(|e| e.to_string())
                };
                if let Ok(ref albums) = result {
                    crate::handlers::view_cache::cache_put(db3, &key, albums).await;
                }
                result
            },
            |result| cosmic::Action::App(Message::ArtistAlbumsLoaded(result)),
        );

        let read_info = self.read_view_cache::<crate::tidal::models::Artist, _>(
            format!("artist:{artist_id}:info"),
            |a| Message::ArtistInfoLoaded(Ok(a)),
        );
        let read_tracks = self.read_view_cache::<Vec<crate::tidal::models::Track>, _>(
            format!("artist:{artist_id}:toptracks"),
            |t| Message::ArtistTopTracksLoaded(Ok(t)),
        );
        let read_albums = self.read_view_cache::<Vec<crate::tidal::models::Album>, _>(
            format!("artist:{artist_id}:albums"),
            |a| Message::ArtistAlbumsLoaded(Ok(a)),
        );

        Task::batch(vec![
            read_info,
            read_tracks,
            read_albums,
            info_task,
            tracks_task,
            albums_task,
        ])
    }

    // =========================================================================
    // Back navigation (pop the stack)
    // =========================================================================

    /// Handle NavigateBack: pop the nav stack and restore the previous view.
    ///
    /// Data for parent views (artist info, album tracks, etc.) is still in
    /// memory so we just switch the view state — no refetching needed.
    pub fn handle_navigate_back(&mut self) -> Task<cosmic::Action<Message>> {
        let target = self.nav_stack.pop().unwrap_or(ViewState::Main);

        // Rebuild the target view's virtual-list content from its own source
        // data: restores the correct rows AND forces the `list::List` widget to
        // rebuild its cached per-row widget state. See `rebuild_virtual_list_for`.
        self.rebuild_virtual_list_for(&target);

        // For views that need focus or lazy-loading, handle specially
        match &target {
            ViewState::Search => {
                self.view_state = ViewState::Search;
                return text_input::focus(SEARCH_INPUT_ID.clone());
            }
            ViewState::Playlists => {
                self.view_state = ViewState::Playlists;
                // Regenerate any missing grid thumbnails (e.g. after visiting a
                // playlist detail where tracks were freshly cached).
                return Task::done(cosmic::Action::App(Message::GeneratePlaylistThumbnails));
            }
            _ => {
                self.view_state = target;
            }
        }

        Task::none()
    }

    /// Rebuild the target view's virtual-list `Content` from its own backing
    /// data when returning to it via back navigation.
    ///
    /// This serves two purposes:
    ///
    /// 1. Correctness. Several views share the single `track_list_content`
    ///    field, and the per-view `selected_*` source vectors are only
    ///    repopulated on forward navigation. Without rebuilding here, going
    ///    back to a parent track view (e.g. TrackRadio -> back -> PlaylistDetail)
    ///    would render whatever tracks the child view left behind.
    ///
    /// 2. Crash avoidance. Every `list::List` shares one widget tag (its
    ///    private `State` type is not generic over the item type), so iced
    ///    reuses a List's cached widget-state across view changes whenever a
    ///    List sits at the same position in the widget tree. That cached state
    ///    holds child widget-trees built for the previous view's rows; if the
    ///    target view renders rows with a different widget structure (e.g.
    ///    Explore's mixed header/link/promo rows vs. a uniform track list), the
    ///    List reuses those stale child-trees and iced downcasts a child's
    ///    state to the wrong widget type, panicking with "Downcast widget
    ///    state" (iced `widget/tree.rs`). Recreating the `Content` (via
    ///    `set_track_list`, the `rebuild_*` helpers, or a fresh collect) marks
    ///    it "new", which makes the List recompute from scratch and drop the
    ///    stale child-trees -- exactly what forward navigation already does.
    ///
    /// Views without a virtual `List` have no cached row state to reset, so
    /// they're left untouched.
    fn rebuild_virtual_list_for(&mut self, target: &ViewState) {
        match target {
            ViewState::AlbumDetail => {
                let tracks = self.selected_album_tracks.clone();
                self.set_track_list(tracks);
            }
            ViewState::PlaylistDetail => {
                let tracks = self.selected_playlist_tracks.clone();
                self.set_track_list(tracks);
            }
            ViewState::MixDetail => {
                let tracks = self.selected_mix_tracks.clone();
                self.set_track_list(tracks);
            }
            ViewState::TrackRadio => {
                let tracks = self.selected_radio_tracks.clone();
                self.set_track_list(tracks);
            }
            ViewState::ArtistDetail => {
                let tracks = self.selected_artist_top_tracks.clone();
                self.set_track_list(tracks);
            }
            ViewState::History => self.rebuild_history_track_list(),
            ViewState::FavoriteTracks => self.rebuild_favorites_track_list(),
            ViewState::Explore => {
                self.explore_rows = self
                    .explore_page
                    .as_ref()
                    .map(|page| page.into_rows().into_iter().collect())
                    .unwrap_or_default();
            }
            _ => {}
        }
    }
}
