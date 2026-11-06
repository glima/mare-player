// SPDX-License-Identifier: GPL-3.0-only

//! Library data loading handlers for Maré Player.
//!
//! Handles loading playlists, albums, tracks, mixes, artist detail, album
//! detail (by ID), track radio, track detail recommendations, and followed
//! artists (profiles). Also contains Task helper methods for initiating
//! async data fetches.

use cosmic::prelude::*;

use crate::messages::Message;
use crate::state::{AppModel, ViewState};
use crate::tidal::models::{Album, Artist, ArtistRow, FeedActivity, FeedRow, Mix, Playlist, Track, TrackDetailRow};

// =============================================================================
// Task Helper Methods
// =============================================================================

impl AppModel {
    /// Load user playlists from TIDAL
    pub(crate) fn load_playlists(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_user_playlists(Some(50), None).await.map_err(|e| e.to_string())
                };
                if let Ok(ref playlists) = result {
                    crate::handlers::view_cache::cache_put(db, "library:playlists", playlists);
                }
                result
            },
            |result| cosmic::Action::App(Message::PlaylistsLoaded(result)),
        )
    }

    /// Load tracks for a specific playlist
    pub(crate) fn load_playlist_tracks(&self, playlist_uuid: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        let key = format!("playlist:{playlist_uuid}:tracks");
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_playlist_tracks(&playlist_uuid, None, None).await.map_err(|e| e.to_string())
                };
                if let Ok(ref tracks) = result {
                    crate::handlers::view_cache::cache_put(db, &key, tracks);
                }
                result
            },
            |result| cosmic::Action::App(Message::PlaylistTracksLoaded(result)),
        )
    }

    /// Load user favorite albums from TIDAL
    pub(crate) fn load_albums(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_user_favorite_albums(None).await.map_err(|e| e.to_string())
                };
                if let Ok(ref albums) = result {
                    crate::handlers::view_cache::cache_put(db, "library:albums", albums);
                }
                result
            },
            |result| cosmic::Action::App(Message::AlbumsLoaded(result)),
        )
    }

    /// Load tracks for a specific album
    pub(crate) fn load_album_tracks(&self, album_id: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        let key = format!("album:{album_id}:tracks");
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_album_tracks(&album_id, None, None).await.map_err(|e| e.to_string())
                };
                if let Ok(ref tracks) = result {
                    crate::handlers::view_cache::cache_put(db, &key, tracks);
                }
                result
            },
            |result| cosmic::Action::App(Message::AlbumTracksLoaded(result)),
        )
    }

    /// Load user favorite tracks from TIDAL
    pub(crate) fn load_favorite_tracks(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_user_favorite_tracks(None).await.map_err(|e| e.to_string())
                };
                if let Ok(ref tracks) = result {
                    crate::handlers::view_cache::cache_put(db, "favorites:tracks", tracks);
                }
                result
            },
            |result| cosmic::Action::App(Message::FavoriteTracksLoaded(result)),
        )
    }

    /// Load personalized mixes from the TIDAL home feed
    pub(crate) fn load_mixes(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_mixes().await.map_err(|e| e.to_string())
                };
                if let Ok(ref mixes) = result {
                    crate::handlers::view_cache::cache_put(db, "library:mixes", mixes);
                }
                result
            },
            |result| cosmic::Action::App(Message::MixesLoaded(result)),
        )
    }

    /// Load tracks for a specific mix
    pub(crate) fn load_mix_tracks(&self, mix_id: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        let key = format!("mix:{mix_id}:tracks");
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_mix_tracks(&mix_id).await.map_err(|e| e.to_string())
                };
                if let Ok(ref tracks) = result {
                    crate::handlers::view_cache::cache_put(db, &key, tracks);
                }
                result
            },
            |result| cosmic::Action::App(Message::MixTracksLoaded(result)),
        )
    }

    /// Load the track-seeded mix for a track (TIDAL's "Track Radio").
    ///
    /// A present `(mix_id, tracks)` lets the view attribute plays as
    /// `MIX:<mix_id>` — the attribution that surfaces track-radio
    /// listening in TIDAL's Recently Played.  See
    /// [`TidalAppClient::get_track_mix`](crate::tidal::client::TidalAppClient::get_track_mix).
    pub(crate) fn load_track_radio(&self, track_id: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let version = self.track_radio_request_version;
        Task::perform(
            async move {
                let client = client.lock().await;
                let result = client.get_track_mix(&track_id).await.map_err(|e| e.to_string());
                (track_id, result)
            },
            move |(track_id, result)| cosmic::Action::App(Message::TrackRadioLoaded(version, track_id, result)),
        )
    }

    /// Load credits for a specific track.
    ///
    /// Returns an empty [`TrackCredits`](crate::tidal::models::TrackCredits) (not an error) when TIDAL has no
    /// credits for the track; only genuine network/parse failures end in
    /// `Err`.  See
    /// [`TidalAppClient::get_track_credits`](crate::tidal::client::TidalAppClient::get_track_credits).
    pub(crate) fn load_track_credits(&self, track_id: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        let key = format!("credits:{track_id}");
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_track_credits(&track_id).await.map_err(|e| e.to_string())
                };
                // Cache the result (including "no credits") so re-opening the
                // view paints instantly next time.
                if let Ok(ref credits) = result {
                    crate::handlers::view_cache::cache_put(db, &key, credits);
                }
                result
            },
            |result| cosmic::Action::App(Message::TrackCreditsLoaded(result)),
        )
    }

    /// Load lyrics for a specific track.
    ///
    /// Returns an empty [`TrackLyrics`](crate::tidal::models::TrackLyrics) (not an error) when TIDAL has
    /// no lyrics; only genuine network/parse failures end in `Err`.
    /// See [`TidalAppClient::get_track_lyrics`](crate::tidal::client::TidalAppClient::get_track_lyrics).
    pub(crate) fn load_track_lyrics(&self, track_id: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        let key = format!("lyrics:{track_id}");
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_track_lyrics(&track_id).await.map_err(|e| e.to_string())
                };
                // Cache the result (including "no lyrics") so the lyrics view
                // and the now-playing icon check read it instantly next time.
                if let Ok(ref lyrics) = result {
                    crate::handlers::view_cache::cache_put(db, &key, lyrics);
                }
                result
            },
            |result| cosmic::Action::App(Message::TrackLyricsLoaded(result)),
        )
    }

    /// Refresh the now-playing lyrics-availability flag for `track`, so the
    /// now-playing bar only shows the lyrics icon when the track actually has
    /// lyrics. Reads the DB lyrics cache first and only hits the network on a
    /// miss (caching the result, including "no lyrics").
    pub(crate) fn refresh_now_playing_lyrics(&mut self, track: &Track) -> Task<cosmic::Action<Message>> {
        let track_id = track.id.clone();
        // Unknown until the check returns — hide the icon in the meantime.
        self.now_playing_lyrics = None;
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        let key = format!("lyrics:{track_id}");
        Task::perform(
            async move {
                // DB cache first; only hit the network on a miss.
                if let Some(db) = &db
                    && let Some(bytes) = db.get_view(&key).await
                    && let Ok(lyrics) = serde_json::from_slice::<crate::tidal::models::TrackLyrics>(&bytes)
                {
                    return (track_id, !lyrics.is_empty());
                }
                let lyrics = {
                    let client = client.lock().await;
                    client.get_track_lyrics(&track_id).await.ok()
                };
                let has = match lyrics {
                    Some(l) => {
                        crate::handlers::view_cache::cache_put(db, &key, &l);
                        !l.is_empty()
                    }
                    None => false,
                };
                (track_id, has)
            },
            |(id, has)| cosmic::Action::App(Message::NowPlayingLyricsChecked(id, has)),
        )
    }

    /// Store the result of a now-playing lyrics-availability check: when it's
    /// still the current track, update the flag the now-playing bar reads.
    pub fn handle_now_playing_lyrics_checked(&mut self, track_id: String, has_lyrics: bool) {
        if self.now_playing.as_ref().is_some_and(|np| np.track_id == track_id) {
            self.now_playing_lyrics = Some((track_id, has_lyrics));
        }
    }

    /// Load albums by the track's artist (for "More Albums by {Artist}" section).
    pub(crate) fn load_track_detail_artist_albums(&self, artist_id: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client.get_artist_albums(&artist_id, Some(20)).await.map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::TrackDetailArtistAlbumsLoaded(result)),
        )
    }

    /// Load similar/related artists for a track detail view.
    pub(crate) fn load_track_detail_related_artists(&self, artist_id: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client.get_similar_artists(&artist_id, Some(20)).await.map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::TrackDetailRelatedArtistsLoaded(result)),
        )
    }

    /// Load one album per similar artist to build the "Related Albums" section.
    ///
    /// Fetches each artist's discography (limit 1) in parallel and flattens
    /// the results.  Failures for individual artists are silently skipped.
    pub(crate) fn load_track_detail_related_albums(&self, artist_ids: Vec<String>) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                let mut albums = Vec::new();
                for id in &artist_ids {
                    if let Ok(mut artist_albums) = client.get_artist_albums(id, Some(1)).await {
                        albums.append(&mut artist_albums);
                    }
                }
                Ok(albums)
            },
            |result| cosmic::Action::App(Message::TrackDetailRelatedAlbumsLoaded(result)),
        )
    }

    /// Load feed activities (new releases from followed artists)
    pub(crate) fn load_feed(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_feed().await.map_err(|e| e.to_string())
                };
                if let Ok(ref feed) = result {
                    crate::handlers::view_cache::cache_put(db, "feed", feed);
                }
                result
            },
            |result| cosmic::Action::App(Message::FeedLoaded(result)),
        )
    }

    /// Fetch an Explore (browse) page by slug.
    pub(crate) fn load_explore_page(&self, slug: &str) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let slug = slug.to_string();
        Task::perform(
            async move {
                let client = client.lock().await;
                client.get_explore_page(&slug).await.map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::ExploreLoaded(result)),
        )
    }

    /// Load followed artists (profiles)
    pub(crate) fn load_profiles(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_followed_artists().await.map_err(|e| e.to_string())
                };
                if let Ok(ref artists) = result {
                    crate::handlers::view_cache::cache_put(db, "profiles", artists);
                }
                result
            },
            |result| cosmic::Action::App(Message::ProfilesLoaded(result)),
        )
    }
}

// =============================================================================
// Message Handlers
// =============================================================================

impl AppModel {
    /// Handle load playlists request
    pub fn handle_load_playlists(&self) -> Task<cosmic::Action<Message>> {
        self.load_playlists()
    }

    /// Handle playlists loaded
    pub fn handle_playlists_loaded(&mut self, result: Result<Vec<Playlist>, String>) -> Task<cosmic::Action<Message>> {
        // Only clear is_loading when the user is actually viewing playlists,
        // so background pre-fetches don't clobber loading state for other views.
        if self.view_state == ViewState::Playlists {
            self.is_loading = false;
        }
        match result {
            Ok(playlists) => {
                self.user_playlists = playlists;
                // Covers load lazily per visible row (HandleCache::get_or_request).
                // Kick off 2×2 grid thumbnail generation in the background.
                Task::done(cosmic::Action::App(Message::GeneratePlaylistThumbnails))
            }
            Err(e) => {
                tracing::error!("Failed to load playlists: {}", e);
                self.error_message = Some(format!("Failed to load playlists: {}", e));
                Task::none()
            }
        }
    }

    /// Handle playlist tracks loaded
    pub fn handle_playlist_tracks_loaded(&mut self, result: Result<Vec<Track>, String>) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(tracks) => {
                self.set_track_list(tracks.clone());
                self.selected_playlist_tracks = tracks;
                // Covers load lazily per visible row via get_or_request.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load tracks: {}", e);
                self.error_message = Some(format!("Failed to load tracks: {}", e));
                Task::none()
            }
        }
    }

    /// Handle load albums request
    pub fn handle_load_albums(&self) -> Task<cosmic::Action<Message>> {
        self.load_albums()
    }

    /// Rebuild the mixes virtual-`List` content from `user_mixes`.
    pub(crate) fn rebuild_mixes_content(&mut self) {
        self.mixes_content = self.user_mixes.iter().cloned().collect();
    }

    /// Rebuild the followed-artists (profiles) virtual-`List` content.
    pub(crate) fn rebuild_profiles_content(&mut self) {
        self.profiles_content = self.user_followed_artists.iter().cloned().collect();
    }

    /// Rebuild the favorite-albums virtual-`List` content from `user_albums`,
    /// in the chosen order.
    pub(crate) fn rebuild_albums_content(&mut self) {
        self.albums_content = crate::handlers::data::favorites::sort_favorites(
            self.user_albums.clone(),
            self.config.favorite_albums_sort,
            |a| &a.title,
            |a| a.added_at,
        )
        .into_iter()
        .collect();
    }

    /// Rebuild the track-detail recommendations into virtual-`List` content:
    /// the track header, then a header + cards (or a loading placeholder) for
    /// each of the three recommendation sections. Called as each section loads.
    pub(crate) fn rebuild_track_detail_rows(&mut self) {
        use TrackDetailRow as R;
        let mut rows: Vec<R> = Vec::new();

        if let Some(track) = &self.selected_detail_track {
            rows.push(R::Header(Box::new(track.clone())));
        }
        let artist_name = self.selected_detail_track.as_ref().map(|t| t.artist_name.clone()).unwrap_or_default();

        // Section 1: More Albums by {Artist}
        if !self.track_detail_artist_albums.is_empty() {
            rows.push(R::SectionHeader(crate::fl!("more-albums-by", artist = artist_name.clone())));
            rows.extend(self.track_detail_artist_albums.iter().cloned().map(|a| R::ArtistAlbum(Box::new(a))));
        } else if self.is_loading {
            rows.push(R::SectionHeader(crate::fl!("more-albums-by", artist = artist_name.clone())));
            rows.push(R::Loading);
        }

        // Section 2: Related Albums
        if !self.track_detail_related_albums.is_empty() {
            rows.push(R::SectionHeader(crate::fl!("related-albums")));
            rows.extend(self.track_detail_related_albums.iter().cloned().map(|a| R::RelatedAlbum(Box::new(a))));
        } else if !self.track_detail_related_artists.is_empty() {
            rows.push(R::SectionHeader(crate::fl!("related-albums")));
            rows.push(R::Loading);
        }

        // Section 3: Related Artists
        if !self.track_detail_related_artists.is_empty() {
            rows.push(R::SectionHeader(crate::fl!("related-artists")));
            rows.extend(self.track_detail_related_artists.iter().cloned().map(|a| R::RelatedArtist(Box::new(a))));
        } else if self.is_loading {
            rows.push(R::SectionHeader(crate::fl!("related-artists")));
            rows.push(R::Loading);
        }

        self.track_detail_rows = rows.into_iter().collect();
    }

    /// Rebuild the feed virtual-`List` content from `feed_activities`, grouping
    /// activities into time buckets (New / Last week / Last month / Older) with
    /// a section header per non-empty bucket. Only visible rows render, so
    /// covers load lazily on scroll.
    pub(crate) fn rebuild_feed_content(&mut self) {
        let now = chrono::Utc::now();
        let mut new_updates: Vec<FeedActivity> = Vec::new();
        let mut last_week: Vec<FeedActivity> = Vec::new();
        let mut last_month: Vec<FeedActivity> = Vec::new();
        let mut older: Vec<FeedActivity> = Vec::new();

        for activity in &self.feed_activities {
            let days = chrono::DateTime::parse_from_rfc3339(&activity.occurred_at)
                .ok()
                .map(|date| now.signed_duration_since(date).num_days());
            match days {
                Some(d) if d <= 2 => new_updates.push(activity.clone()),
                Some(d) if d <= 7 => last_week.push(activity.clone()),
                Some(d) if d <= 30 => last_month.push(activity.clone()),
                _ => older.push(activity.clone()),
            }
        }

        let mut rows: Vec<FeedRow> = Vec::new();
        if !new_updates.is_empty() {
            rows.push(FeedRow::SectionHeader(crate::fl!("feed-new-updates")));
            rows.extend(new_updates.into_iter().map(|a| FeedRow::Activity(Box::new(a))));
        }
        if !last_week.is_empty() {
            rows.push(FeedRow::SectionHeader(crate::fl!("feed-last-week")));
            rows.extend(last_week.into_iter().map(|a| FeedRow::Activity(Box::new(a))));
        }
        if !last_month.is_empty() {
            rows.push(FeedRow::SectionHeader(crate::fl!("feed-last-month")));
            rows.extend(last_month.into_iter().map(|a| FeedRow::Activity(Box::new(a))));
        }
        if !older.is_empty() {
            rows.push(FeedRow::SectionHeader(crate::fl!("feed-older")));
            rows.extend(older.into_iter().map(|a| FeedRow::Activity(Box::new(a))));
        }
        self.feed_content = rows.into_iter().collect();
    }

    /// Handle albums loaded
    pub fn handle_albums_loaded(&mut self, result: Result<Vec<Album>, String>) -> Task<cosmic::Action<Message>> {
        if self.view_state == ViewState::Albums {
            self.is_loading = false;
        }
        match result {
            Ok(albums) => {
                self.user_albums = albums;
                // Populate favorite album IDs so we know which albums are favorited
                self.populate_favorite_album_ids();
                self.rebuild_albums_content();
                // Covers load lazily per visible card via get_or_request.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load albums: {}", e);
                self.error_message = Some(format!("Failed to load albums: {}", e));
                Task::none()
            }
        }
    }

    /// Handle album tracks loaded
    pub fn handle_album_tracks_loaded(&mut self, result: Result<Vec<Track>, String>) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(tracks) => {
                self.set_track_list(tracks.clone());
                self.selected_album_tracks = tracks;
                // Covers load lazily per visible row via get_or_request.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load album tracks: {}", e);
                self.error_message = Some(format!("Failed to load album tracks: {}", e));
                Task::none()
            }
        }
    }

    /// Handle album info loaded (when navigating by ID from now-playing or artist view)
    pub fn handle_album_info_loaded(&mut self, result: Result<Album, String>) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(album) => {
                let mut urls: Vec<String> = Vec::new();
                if let Some(url) = &album.cover_url {
                    urls.push(url.clone());
                }
                // If the album already has a review (fetched by get_album_info),
                // we're done.  Otherwise kick off a background fetch.
                let needs_review = album.review.is_none();
                let album_id = album.id.clone();
                self.selected_album = Some(album);
                let img_task = self.load_images_for_urls(urls);
                if needs_review { Task::batch([img_task, self.load_album_review(album_id)]) } else { img_task }
            }
            Err(e) => {
                tracing::error!("Failed to load album info: {}", e);
                self.error_message = Some(format!("Failed to load album info: {}", e));
                Task::none()
            }
        }
    }

    /// Handle album review text loaded (asynchronous, best-effort).
    pub fn handle_album_review_loaded(&mut self, result: Result<String, String>) {
        match result {
            Ok(review) => {
                if let Some(album) = &mut self.selected_album {
                    album.review = Some(review);
                }
            }
            Err(e) => {
                // Many albums have no review — this is expected, not an error.
                tracing::debug!("No album review available: {}", e);
            }
        }
    }

    /// Fire a background task to fetch the album review text from TIDAL.
    pub(crate) fn load_album_review(&self, album_id: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        let db = self.cache_db.clone();
        let key = format!("album:{album_id}:review");
        Task::perform(
            async move {
                let result = {
                    let client = client.lock().await;
                    client.get_album_review(&album_id).await.map_err(|e| e.to_string())
                };
                if let Ok(ref review) = result {
                    crate::handlers::view_cache::cache_put(db, &key, review);
                }
                result
            },
            |result| cosmic::Action::App(Message::AlbumReviewLoaded(result)),
        )
    }

    /// Rebuild the flattened artist-detail rows (`artist_rows`) from the current
    /// artist info, top tracks, videos, and discography. Those four payloads load
    /// in parallel, so this is called whenever any of them arrives to keep the
    /// single virtual list in sync. Only visible rows render, so covers load
    /// lazily on scroll (no bulk prefetch).
    pub(crate) fn rebuild_artist_rows(&mut self) {
        let mut rows: Vec<ArtistRow> = Vec::new();
        if let Some(artist) = &self.selected_artist {
            rows.push(ArtistRow::Info(Box::new(artist.clone())));
        }
        if !self.selected_artist_top_tracks.is_empty() {
            rows.push(ArtistRow::SectionHeader(crate::fl!("top-tracks")));
            rows.extend((0..self.selected_artist_top_tracks.len()).map(ArtistRow::TopTrack));
        }
        if !self.selected_artist_videos.is_empty() {
            rows.push(ArtistRow::SectionHeader(crate::fl!("videos")));
            rows.extend((0..self.selected_artist_videos.len()).map(ArtistRow::Video));
        }
        if !self.selected_artist_albums.is_empty() {
            rows.push(ArtistRow::SectionHeader(crate::fl!("discography")));
            rows.extend(self.selected_artist_albums.iter().cloned().map(|a| ArtistRow::Album(Box::new(a))));
        }
        self.artist_rows = rows.into_iter().collect();
    }

    /// Handle artist info loaded
    pub fn handle_artist_info_loaded(&mut self, result: Result<Artist, String>) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(artist) => {
                let mut urls: Vec<String> = Vec::new();
                if let Some(url) = &artist.picture_url {
                    urls.push(url.clone());
                }
                self.selected_artist = Some(artist);
                self.rebuild_artist_rows();
                self.load_images_for_urls(urls)
            }
            Err(e) => {
                self.is_loading = false;
                tracing::error!("Failed to load artist info: {}", e);
                self.error_message = Some(format!("Failed to load artist info: {}", e));
                Task::none()
            }
        }
    }

    /// Handle artist top tracks loaded
    pub fn handle_artist_top_tracks_loaded(&mut self, result: Result<Vec<Track>, String>) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(tracks) => {
                self.set_track_list(tracks.clone());
                self.selected_artist_top_tracks = tracks;
                self.rebuild_artist_rows();
                // Covers load lazily per visible row via get_or_request.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load artist tracks: {}", e);
                self.error_message = Some(format!("Failed to load artist tracks: {}", e));
                Task::none()
            }
        }
    }

    /// Handle artist albums (discography) loaded
    pub fn handle_artist_albums_loaded(&mut self, result: Result<Vec<Album>, String>) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(albums) => {
                self.selected_artist_albums = albums;
                self.rebuild_artist_rows();
                // Covers load lazily per visible row via get_or_request.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load artist albums: {}", e);
                self.error_message = Some(format!("Failed to load artist albums: {}", e));
                Task::none()
            }
        }
    }

    /// Handle artist music videos loaded
    pub fn handle_artist_videos_loaded(&mut self, result: Result<Vec<Track>, String>) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(videos) => {
                self.selected_artist_videos = videos;
                self.rebuild_artist_rows();
                // Covers load lazily per visible row via get_or_request.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load artist videos: {}", e);
                // Non-fatal: the videos section just stays hidden.
                Task::none()
            }
        }
    }

    /// Handle load favorite tracks request
    pub fn handle_load_favorite_tracks(&self) -> Task<cosmic::Action<Message>> {
        self.load_favorite_tracks()
    }

    /// Handle favorite tracks loaded
    pub fn handle_favorite_tracks_loaded(&mut self, result: Result<Vec<Track>, String>) -> Task<cosmic::Action<Message>> {
        if self.view_state == ViewState::FavoriteTracks {
            self.is_loading = false;
        }
        match result {
            Ok(tracks) => {
                // Populate favorite track IDs set
                self.favorite_track_ids = tracks.iter().map(|t| t.id.clone()).collect();
                self.user_favorite_tracks = tracks;
                if self.view_state == ViewState::FavoriteTracks {
                    self.rebuild_favorites_track_list();
                }
                // Covers load lazily per visible row via get_or_request.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load favorite tracks: {}", e);
                self.error_message = Some(format!("Failed to load favorite tracks: {}", e));
                Task::none()
            }
        }
    }

    /// Populate favorite_album_ids from the loaded user_albums list.
    /// Called after user_albums are loaded so we know which albums are favorited.
    pub fn populate_favorite_album_ids(&mut self) {
        self.favorite_album_ids = self.user_albums.iter().map(|a| a.id.clone()).collect();
    }

    /// Handle loading mixes
    pub fn handle_load_mixes(&mut self) -> Task<cosmic::Action<Message>> {
        self.is_loading = true;
        self.load_mixes()
    }

    /// Handle mixes loaded result
    pub fn handle_mixes_loaded(&mut self, result: Result<Vec<Mix>, String>) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(mixes) => {
                tracing::info!("Loaded {} mixes", mixes.len());
                self.user_mixes = mixes;
                self.rebuild_mixes_content();
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load mixes: {}", e);
                self.error_message = Some(format!("Failed to load mixes: {}", e));
                Task::none()
            }
        }
    }

    /// Handle mix tracks loaded result
    pub fn handle_mix_tracks_loaded(&mut self, result: Result<Vec<Track>, String>) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(tracks) => {
                tracing::info!("Loaded {} mix tracks", tracks.len());
                self.set_track_list(tracks.clone());
                self.selected_mix_tracks = tracks;
                // Covers load lazily per visible row via get_or_request.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load mix tracks: {}", e);
                self.error_message = Some(format!("Failed to load mix tracks: {}", e));
                Task::none()
            }
        }
    }

    /// Handle track radio loaded result.
    ///
    /// Stores the backing mix id (for `MIX:<mix_id>` play attribution)
    /// alongside the resolved track list.
    pub fn handle_track_radio_loaded(
        &mut self,
        version: u64,
        track_id: String,
        result: Result<Option<(String, Vec<Track>)>, String>,
    ) -> Task<cosmic::Action<Message>> {
        if version != self.track_radio_request_version {
            return Task::none();
        }
        if matches!(&result, Ok(None)) {
            std::sync::Arc::make_mut(&mut self.unavailable_radio_tracks).insert(track_id.clone());
        }
        if !self.selected_radio_source_track.as_ref().is_some_and(|track| track.id == track_id) {
            return Task::none();
        }
        self.track_radio_loading = false;
        let visible = self.view_state == ViewState::TrackRadio;
        match result {
            Ok(Some((mix_id, tracks))) => {
                tracing::info!("Loaded track radio: mix={} tracks={}", mix_id, tracks.len());
                if visible {
                    self.set_track_list(tracks.clone());
                    self.error_message = None;
                }
                self.selected_radio_tracks = tracks;
                self.selected_radio_mix_id = Some(mix_id);
            }
            Ok(None) => {
                tracing::info!("TIDAL has no radio mix for track {}", track_id);
                self.selected_radio_tracks.clear();
                self.selected_radio_mix_id = None;
                if visible {
                    self.set_track_list(Vec::new());
                    self.error_message = None;
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load track radio for {}: {}", track_id, e);
                if visible {
                    self.error_message = Some(e);
                }
            }
        }
        Task::none()
    }

    /// Handle lyrics loaded result.
    ///
    /// Stores the lyrics (or an empty `TrackLyrics` when TIDAL has
    /// none) and recomputes `current_lyric_index` from the current
    /// playback position so the UI is correct on first render — the
    /// tick handler also keeps it fresh from there on.  Errors are
    /// surfaced in the error banner but don't block the view; the
    /// lyrics view falls back to its "failed to load" state.
    pub fn handle_track_lyrics_loaded(
        &mut self,
        result: Result<crate::tidal::models::TrackLyrics, String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(lyrics) => {
                tracing::info!(
                    "Lyrics loaded: provider={:?} plain={} synced={} lines={}",
                    lyrics.provider,
                    lyrics.plain_text.is_some(),
                    lyrics.is_synced(),
                    lyrics.lrc_lines.len()
                );
                self.current_lyric_index = lyrics.line_index_at(self.playback_position);
                self.selected_track_lyrics = Some(lyrics);
            }
            Err(e) => {
                tracing::error!("Failed to load lyrics: {}", e);
                self.error_message = Some(format!("Failed to load lyrics: {}", e));
                // Leave selected_track_lyrics = None; the view shows the
                // "failed to load" empty state in that case.
            }
        }
        Task::none()
    }

    /// Handle credits loaded result.
    ///
    /// Stores the credits (or an empty `TrackCredits` when TIDAL has none).
    /// Errors are surfaced in the error banner but don't block the view; the
    /// credits view falls back to its loading/empty state.
    pub fn handle_track_credits_loaded(&mut self, result: Result<crate::tidal::models::TrackCredits, String>) {
        match result {
            Ok(credits) => {
                tracing::info!(
                    "Credits loaded: roles={} label={} isrc={} bpm={:?}",
                    credits.roles.len(),
                    credits.copyright.is_some(),
                    credits.isrc.is_some(),
                    credits.bpm
                );
                self.selected_track_credits = Some(credits);
            }
            Err(e) => {
                tracing::error!("Failed to load credits: {}", e);
                self.error_message = Some(format!("Failed to load credits: {}", e));
                // Leave selected_track_credits = None; the view keeps showing
                // its loading state in that case.
            }
        }
    }

    /// Handle "More Albums by {Artist}" loaded for the track detail view.
    pub fn handle_track_detail_artist_albums_loaded(
        &mut self,
        result: Result<Vec<Album>, String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(albums) => {
                tracing::info!("Track detail: loaded {} artist albums", albums.len());
                self.track_detail_artist_albums = albums;
                self.rebuild_track_detail_rows();
                // Covers load lazily per visible row via get_or_request.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load artist albums for track detail: {}", e);
                Task::none()
            }
        }
    }

    /// Handle similar/related artists loaded for the track detail view.
    ///
    /// After storing the artists, kicks off a follow-up fetch of one album per
    /// similar artist to populate the "Related Albums" section.
    pub fn handle_track_detail_related_artists_loaded(
        &mut self,
        result: Result<Vec<Artist>, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(artists) => {
                tracing::info!("Track detail: loaded {} related artists", artists.len());
                let artist_ids: Vec<String> = artists.iter().map(|a| a.id.clone()).collect();
                self.track_detail_related_artists = artists;

                // Fetch related albums (one per similar artist) in a follow-up.
                // Pictures load lazily per visible row via get_or_request.
                let albums_task =
                    if artist_ids.is_empty() { Task::none() } else { self.load_track_detail_related_albums(artist_ids) };
                self.rebuild_track_detail_rows();
                albums_task
            }
            Err(e) => {
                tracing::error!("Failed to load related artists for track detail: {}", e);
                Task::none()
            }
        }
    }

    /// Handle related albums loaded for the track detail view.
    pub fn handle_track_detail_related_albums_loaded(
        &mut self,
        result: Result<Vec<Album>, String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(albums) => {
                tracing::info!("Track detail: loaded {} related albums", albums.len());
                self.track_detail_related_albums = albums;
                self.rebuild_track_detail_rows();
                // Covers load lazily per visible row via get_or_request.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load related albums for track detail: {}", e);
                Task::none()
            }
        }
    }

    /// Handle loading feed
    pub fn handle_load_feed(&mut self) -> Task<cosmic::Action<Message>> {
        self.is_loading = true;
        self.load_feed()
    }

    /// Handle feed loaded result
    pub fn handle_feed_loaded(&mut self, result: Result<Vec<FeedActivity>, String>) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(activities) => {
                tracing::info!("Loaded {} feed activities", activities.len());
                self.feed_activities = activities;
                self.rebuild_feed_content();
                // Covers load lazily per visible row via get_or_request.
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load feed: {}", e);
                self.error_message = Some(format!("Failed to load feed: {}", e));
                Task::none()
            }
        }
    }

    /// Open an Explore sub-page or retry the current page. Retrying preserves
    /// the back stack; opening a different page adds its slug.
    pub fn handle_load_explore_page(&mut self, slug: String) -> Task<cosmic::Action<Message>> {
        if self.explore_stack.last() != Some(&slug) {
            self.explore_stack.push(slug.clone());
        }
        self.begin_explore_page_load(&slug)
    }

    /// Pop one level off the Explore back stack and reload the parent page.
    pub fn handle_explore_back(&mut self) -> Task<cosmic::Action<Message>> {
        if self.explore_stack.len() > 1 {
            self.explore_stack.pop();
        }
        if let Some(slug) = self.explore_stack.last().cloned() { self.begin_explore_page_load(&slug) } else { Task::none() }
    }

    /// Activate an Explore card/promo target.
    pub fn handle_open_explore_target(&mut self, target: crate::tidal::models::ExploreTarget) -> Task<cosmic::Action<Message>> {
        use crate::tidal::models::ExploreTarget;
        match target {
            ExploreTarget::Album(id) => self.handle_show_album_detail_by_id(id),
            ExploreTarget::Artist(id) => self.handle_show_artist_detail(id),
            ExploreTarget::Playlist(uuid) => self.handle_show_playlist_detail(uuid, String::new()),
            ExploreTarget::Mix(id) => self.handle_show_mix_detail(id, String::new()),
            ExploreTarget::Page(slug) => self.handle_load_explore_page(slug),
            ExploreTarget::None => Task::none(),
        }
    }

    /// Show a loading state without leaving the previous page's rows active.
    pub(crate) fn begin_explore_page_load(&mut self, slug: &str) -> Task<cosmic::Action<Message>> {
        self.explore_page = None;
        self.rebuild_explore_rows();
        self.explore_loading = true;
        self.error_message = None;
        self.load_explore_page(slug)
    }

    /// Replace the row set and its widget identity together. List layout and
    /// scroll state belong to one page, while artwork redraws keep that state.
    pub(crate) fn rebuild_explore_rows(&mut self) {
        self.explore_rows = self.explore_page.as_ref().map(|page| page.into_rows().into_iter().collect()).unwrap_or_default();
        self.explore_rows_revision = self.explore_rows_revision.wrapping_add(1);
        tracing::debug!(revision = self.explore_rows_revision, rows = self.explore_rows.len(), "Rebuilt Explore list");
    }

    /// Handle an Explore page finishing loading: store it and preload covers.
    pub fn handle_explore_loaded(
        &mut self,
        result: Result<crate::tidal::models::ExplorePage, String>,
    ) -> Task<cosmic::Action<Message>> {
        use crate::tidal::models::ExploreSection;
        self.explore_loading = false;
        match result {
            Ok(page) => {
                tracing::info!("Loaded explore page '{}' ({} sections)", page.title, page.sections.len());
                // Collect every cover URL across the page for preloading.
                let mut urls: Vec<String> = Vec::new();
                for section in &page.sections {
                    match section {
                        ExploreSection::Featured { items, .. } => {
                            urls.extend(items.iter().filter_map(|c| c.image_url.clone()));
                        }
                        ExploreSection::Albums { albums, .. } => {
                            urls.extend(albums.iter().filter_map(|a| a.cover_url.clone()));
                        }
                        ExploreSection::Playlists { playlists, .. } => {
                            urls.extend(playlists.iter().filter_map(|p| p.image_url.clone()));
                        }
                        ExploreSection::Artists { artists, .. } => {
                            urls.extend(artists.iter().filter_map(|a| a.picture_url.clone()));
                        }
                        ExploreSection::Links { .. } => {}
                    }
                }
                self.explore_page = Some(page);
                self.rebuild_explore_rows();
                self.load_images_for_urls(urls)
            }
            Err(e) => {
                tracing::error!("Failed to load explore page: {}", e);
                self.error_message = Some(format!("Failed to load Explore: {}", e));
                Task::none()
            }
        }
    }

    /// Handle loading profiles
    pub fn handle_load_profiles(&mut self) -> Task<cosmic::Action<Message>> {
        self.is_loading = true;
        self.load_profiles()
    }

    /// Handle profiles loaded result
    pub fn handle_profiles_loaded(&mut self, result: Result<Vec<Artist>, String>) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(mut artists) => {
                tracing::info!("Loaded {} followed artists", artists.len());
                artists.sort_by_key(|a| a.name.to_lowercase());
                self.followed_artist_ids = artists.iter().map(|a| a.id.clone()).collect();
                self.user_followed_artists = artists;
                self.rebuild_profiles_content();
                Task::none()
            }
            Err(e) => {
                tracing::error!("Failed to load followed artists: {}", e);
                self.error_message = Some(format!("Failed to load followed artists: {}", e));
                Task::none()
            }
        }
    }
}

#[cfg(test)]
mod track_radio_tests {
    use crate::state::{AppModel, ViewState};
    use crate::tidal::models::Track;
    use cosmic::Application;
    use std::sync::Arc;

    fn app() -> AppModel {
        let (mut app, startup) = AppModel::init(cosmic::Core::default(), ());
        drop(startup);
        app.is_loading = false;
        app
    }

    fn track(id: &str) -> Track {
        Track { id: id.into(), title: id.into(), ..Track::default() }
    }

    #[tokio::test]
    async fn no_mix_shows_an_empty_state_and_remembers_only_that_seed() {
        let mut app = app();
        drop(app.handle_show_track_radio(track("missing")));
        let version = app.track_radio_request_version;
        drop(app.handle_track_radio_loaded(version, "missing".into(), Ok(None)));
        assert!(!app.track_radio_loading);
        assert!(app.error_message.is_none());
        assert!(app.selected_radio_tracks.is_empty());
        assert!(app.track_list_arc.is_empty());
        assert!(app.selected_radio_mix_id.is_none());
        assert!(app.unavailable_radio_tracks.contains("missing"));
        assert!(!app.unavailable_radio_tracks.contains("other"));
        assert_eq!(app.handle_show_track_radio(track("missing")).units(), 0);
        assert!(!app.track_radio_loading);
        assert!(app.handle_show_track_radio(track("other")).units() > 0);
        assert!(app.track_radio_loading);
    }

    #[tokio::test]
    async fn transient_failure_keeps_radio_enabled_and_retryable() {
        let mut app = app();
        drop(app.handle_show_track_radio(track("retry")));
        let version = app.track_radio_request_version;
        drop(app.handle_track_radio_loaded(version, "retry".into(), Err("TIDAL is busy. Try again shortly".into())));
        assert!(!app.track_radio_loading);
        assert_eq!(app.error_message.as_deref(), Some("TIDAL is busy. Try again shortly"));
        assert!(app.unavailable_radio_tracks.is_empty());
        assert!(app.handle_show_track_radio(track("retry")).units() > 0);
        assert!(app.error_message.is_none());
    }

    #[tokio::test]
    async fn a_late_response_cannot_change_the_new_selection() {
        let mut app = app();
        drop(app.handle_show_track_radio(track("old")));
        let old_version = app.track_radio_request_version;
        drop(app.handle_show_track_radio(track("new")));
        drop(app.handle_track_radio_loaded(old_version, "old".into(), Ok(None)));
        assert!(app.track_radio_loading);
        assert!(app.unavailable_radio_tracks.is_empty());
        assert_eq!(app.selected_radio_source_track.as_ref().unwrap().id, "new");
        drop(app.handle_track_radio_loaded(old_version, "old".into(), Err("late error".into())));
        assert!(app.error_message.is_none());
    }

    #[tokio::test]
    async fn successful_radio_keeps_mix_attribution_and_tracks() {
        let mut app = app();
        drop(app.handle_show_track_radio(track("seed")));
        let version = app.track_radio_request_version;
        drop(app.handle_track_radio_loaded(version, "seed".into(), Ok(Some(("mix-id".into(), vec![track("recommended")])))));
        assert!(!app.track_radio_loading);
        assert_eq!(app.selected_radio_mix_id.as_deref(), Some("mix-id"));
        assert_eq!(app.selected_radio_tracks[0].id, "recommended");
        assert_eq!(app.track_list_arc.len(), 1);
        assert!(app.error_message.is_none());
        assert!(app.unavailable_radio_tracks.is_empty());
    }

    #[tokio::test]
    async fn a_mix_with_no_items_does_not_mark_the_seed_unavailable() {
        let mut app = app();
        drop(app.handle_show_track_radio(track("empty")));
        let version = app.track_radio_request_version;
        drop(app.handle_track_radio_loaded(version, "empty".into(), Ok(Some(("mix-id".into(), Vec::new())))));
        assert!(!app.track_radio_loading);
        assert!(app.selected_radio_tracks.is_empty());
        assert!(app.unavailable_radio_tracks.is_empty());
    }

    #[tokio::test]
    async fn leaving_radio_preserves_the_other_views_list_and_loading_state() {
        let mut app = app();
        drop(app.handle_show_track_radio(track("missing")));
        let version = app.track_radio_request_version;
        app.view_state = ViewState::History;
        app.set_track_list(vec![track("history")]);
        app.is_loading = true;
        drop(app.handle_track_radio_loaded(version, "missing".into(), Ok(None)));
        assert_eq!(app.view_state, ViewState::History);
        assert_eq!(app.track_list_arc[0].id, "history");
        assert!(app.is_loading);
        assert!(!app.track_radio_loading);
        assert!(app.unavailable_radio_tracks.contains("missing"));
    }

    #[tokio::test]
    async fn back_from_pending_radio_does_not_leave_a_loaded_parent_on_a_spinner() {
        for parent in [ViewState::PlaylistDetail, ViewState::MixDetail] {
            for result in [Ok(None), Err("offline".into()), Ok(Some(("mix-id".into(), vec![track("recommendation")])))] {
                let mut app = app();
                app.view_state = parent.clone();
                app.selected_playlist_tracks = vec![track("parent")];
                app.selected_mix_tracks = vec![track("parent")];
                app.set_track_list(vec![track("parent")]);
                drop(app.handle_show_track_radio(track("seed")));
                let version = app.track_radio_request_version;
                assert!(app.track_radio_loading);
                assert!(!app.is_loading);
                drop(app.handle_navigate_back());
                assert_eq!(app.view_state, parent);
                assert!(!app.is_loading);
                assert_eq!(app.track_list_arc[0].id, "parent");
                drop(app.handle_track_radio_loaded(version, "seed".into(), result));
                assert!(!app.track_radio_loading);
                assert!(!app.is_loading);
                assert_eq!(app.track_list_arc[0].id, "parent");
                assert!(app.error_message.is_none());
            }
        }
    }

    #[tokio::test]
    async fn logout_clears_availability_and_rejects_old_session_responses() {
        let mut app = app();
        Arc::make_mut(&mut app.unavailable_radio_tracks).insert("known-missing".into());
        drop(app.handle_show_track_radio(track("pending")));
        let version = app.track_radio_request_version;
        drop(app.handle_logout());
        assert!(app.unavailable_radio_tracks.is_empty());
        drop(app.handle_track_radio_loaded(version, "pending".into(), Ok(None)));
        assert!(app.unavailable_radio_tracks.is_empty());
    }
}
