// SPDX-License-Identifier: MIT

//! Library data loading handlers for Maré Player.
//!
//! Handles loading playlists, albums, tracks, mixes, artist detail, album
//! detail (by ID), track radio, track detail recommendations, and followed
//! artists (profiles). Also contains Task helper methods for initiating
//! async data fetches.

use cosmic::prelude::*;

use crate::messages::Message;
use crate::state::{AppModel, ViewState};
use crate::tidal::models::{Album, Artist, Mix, Playlist, Track};

// =============================================================================
// Task Helper Methods
// =============================================================================

impl AppModel {
    /// Load user playlists from TIDAL
    pub(crate) fn load_playlists(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client
                    .get_user_playlists(Some(50), None)
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::PlaylistsLoaded(result)),
        )
    }

    /// Load tracks for a specific playlist
    pub(crate) fn load_playlist_tracks(
        &self,
        playlist_uuid: String,
    ) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client
                    .get_playlist_tracks(&playlist_uuid, None, None)
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::PlaylistTracksLoaded(result)),
        )
    }

    /// Load user favorite albums from TIDAL
    pub(crate) fn load_albums(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client
                    .get_user_favorite_albums(None)
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::AlbumsLoaded(result)),
        )
    }

    /// Load tracks for a specific album
    pub(crate) fn load_album_tracks(&self, album_id: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client
                    .get_album_tracks(&album_id, None, None)
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::AlbumTracksLoaded(result)),
        )
    }

    /// Load user favorite tracks from TIDAL
    pub(crate) fn load_favorite_tracks(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client
                    .get_user_favorite_tracks(None)
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::FavoriteTracksLoaded(result)),
        )
    }

    /// Load personalized mixes from the TIDAL home feed
    pub(crate) fn load_mixes(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client.get_mixes().await.map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::MixesLoaded(result)),
        )
    }

    /// Load tracks for a specific mix
    pub(crate) fn load_mix_tracks(&self, mix_id: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client
                    .get_mix_tracks(&mix_id)
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::MixTracksLoaded(result)),
        )
    }

    /// Load radio tracks for a specific track (similar/recommended tracks)
    pub(crate) fn load_track_radio(&self, track_id: String) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client
                    .get_track_radio(&track_id, None)
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::TrackRadioLoaded(result)),
        )
    }

    /// Load albums by the track's artist (for "More Albums by {Artist}" section).
    pub(crate) fn load_track_detail_artist_albums(
        &self,
        artist_id: String,
    ) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client
                    .get_artist_albums(&artist_id, Some(20))
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::TrackDetailArtistAlbumsLoaded(result)),
        )
    }

    /// Load similar/related artists for a track detail view.
    pub(crate) fn load_track_detail_related_artists(
        &self,
        artist_id: String,
    ) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client
                    .get_similar_artists(&artist_id, Some(20))
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::TrackDetailRelatedArtistsLoaded(result)),
        )
    }

    /// Load one album per similar artist to build the "Related Albums" section.
    ///
    /// Fetches each artist's discography (limit 1) in parallel and flattens
    /// the results.  Failures for individual artists are silently skipped.
    pub(crate) fn load_track_detail_related_albums(
        &self,
        artist_ids: Vec<String>,
    ) -> Task<cosmic::Action<Message>> {
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

    /// Load followed artists (profiles)
    pub(crate) fn load_profiles(&self) -> Task<cosmic::Action<Message>> {
        let client = self.tidal_client.clone();
        Task::perform(
            async move {
                let client = client.lock().await;
                client
                    .get_followed_artists()
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::ProfilesLoaded(result)),
        )
    }

    /// Trigger image loads for mix cover art
    pub(crate) fn load_images_for_mixes(&self) -> Task<cosmic::Action<Message>> {
        let urls: Vec<String> = self
            .user_mixes
            .iter()
            .filter_map(|m| m.image_url.clone())
            .collect();
        self.load_images_for_urls(urls)
    }

    /// Trigger image loads for followed artist pictures
    pub(crate) fn load_images_for_profiles(&self) -> Task<cosmic::Action<Message>> {
        let urls: Vec<String> = self
            .user_followed_artists
            .iter()
            .filter_map(|a| a.picture_url.clone())
            .collect();
        self.load_images_for_urls(urls)
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
    pub fn handle_playlists_loaded(
        &mut self,
        result: Result<Vec<Playlist>, String>,
    ) -> Task<cosmic::Action<Message>> {
        // Only clear is_loading when the user is actually viewing playlists,
        // so background pre-fetches don't clobber loading state for other views.
        if self.view_state == ViewState::Playlists {
            self.is_loading = false;
        }
        match result {
            Ok(playlists) => {
                // Collect image URLs to load
                let urls: Vec<String> = playlists
                    .iter()
                    .filter_map(|p| p.image_url.clone())
                    .collect();
                self.user_playlists = playlists;
                let img_task = self.load_images_for_urls(urls);
                // Kick off 2×2 grid thumbnail generation in the background
                let thumb_task =
                    Task::done(cosmic::Action::App(Message::GeneratePlaylistThumbnails));
                Task::batch([img_task, thumb_task])
            }
            Err(e) => {
                tracing::error!("Failed to load playlists: {}", e);
                self.error_message = Some(format!("Failed to load playlists: {}", e));
                Task::none()
            }
        }
    }

    /// Handle playlist tracks loaded
    pub fn handle_playlist_tracks_loaded(
        &mut self,
        result: Result<Vec<Track>, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(tracks) => {
                // Collect cover URLs to load
                let urls: Vec<String> = tracks.iter().filter_map(|t| t.cover_url.clone()).collect();
                self.selected_playlist_tracks = tracks;
                self.load_images_for_urls(urls)
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

    /// Handle albums loaded
    pub fn handle_albums_loaded(
        &mut self,
        result: Result<Vec<Album>, String>,
    ) -> Task<cosmic::Action<Message>> {
        if self.view_state == ViewState::Albums {
            self.is_loading = false;
        }
        match result {
            Ok(albums) => {
                // Collect image URLs to load
                let urls: Vec<String> = albums.iter().filter_map(|a| a.cover_url.clone()).collect();
                self.user_albums = albums;
                // Populate favorite album IDs so we know which albums are favorited
                self.populate_favorite_album_ids();
                self.load_images_for_urls(urls)
            }
            Err(e) => {
                tracing::error!("Failed to load albums: {}", e);
                self.error_message = Some(format!("Failed to load albums: {}", e));
                Task::none()
            }
        }
    }

    /// Handle album tracks loaded
    pub fn handle_album_tracks_loaded(
        &mut self,
        result: Result<Vec<Track>, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(tracks) => {
                // Collect cover URLs to load
                let urls: Vec<String> = tracks.iter().filter_map(|t| t.cover_url.clone()).collect();
                self.selected_album_tracks = tracks;
                self.load_images_for_urls(urls)
            }
            Err(e) => {
                tracing::error!("Failed to load album tracks: {}", e);
                self.error_message = Some(format!("Failed to load album tracks: {}", e));
                Task::none()
            }
        }
    }

    /// Handle album info loaded (when navigating by ID from now-playing or artist view)
    pub fn handle_album_info_loaded(
        &mut self,
        result: Result<Album, String>,
    ) -> Task<cosmic::Action<Message>> {
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
                if needs_review {
                    Task::batch([img_task, self.load_album_review(album_id)])
                } else {
                    img_task
                }
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
        Task::perform(
            async move {
                let client = client.lock().await;
                client
                    .get_album_review(&album_id)
                    .await
                    .map_err(|e| e.to_string())
            },
            |result| cosmic::Action::App(Message::AlbumReviewLoaded(result)),
        )
    }

    /// Handle artist info loaded
    pub fn handle_artist_info_loaded(
        &mut self,
        result: Result<Artist, String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(artist) => {
                let mut urls: Vec<String> = Vec::new();
                if let Some(url) = &artist.picture_url {
                    urls.push(url.clone());
                }
                self.selected_artist = Some(artist);
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
    pub fn handle_artist_top_tracks_loaded(
        &mut self,
        result: Result<Vec<Track>, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(tracks) => {
                let urls: Vec<String> = tracks.iter().filter_map(|t| t.cover_url.clone()).collect();
                self.selected_artist_top_tracks = tracks;
                self.load_images_for_urls(urls)
            }
            Err(e) => {
                tracing::error!("Failed to load artist tracks: {}", e);
                self.error_message = Some(format!("Failed to load artist tracks: {}", e));
                Task::none()
            }
        }
    }

    /// Handle artist albums (discography) loaded
    pub fn handle_artist_albums_loaded(
        &mut self,
        result: Result<Vec<Album>, String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(albums) => {
                let urls: Vec<String> = albums.iter().filter_map(|a| a.cover_url.clone()).collect();
                self.selected_artist_albums = albums;
                self.load_images_for_urls(urls)
            }
            Err(e) => {
                tracing::error!("Failed to load artist albums: {}", e);
                self.error_message = Some(format!("Failed to load artist albums: {}", e));
                Task::none()
            }
        }
    }

    /// Handle load favorite tracks request
    pub fn handle_load_favorite_tracks(&self) -> Task<cosmic::Action<Message>> {
        self.load_favorite_tracks()
    }

    /// Handle favorite tracks loaded
    pub fn handle_favorite_tracks_loaded(
        &mut self,
        result: Result<Vec<Track>, String>,
    ) -> Task<cosmic::Action<Message>> {
        if self.view_state == ViewState::FavoriteTracks {
            self.is_loading = false;
        }
        match result {
            Ok(tracks) => {
                // Collect cover URLs to load
                let urls: Vec<String> = tracks.iter().filter_map(|t| t.cover_url.clone()).collect();
                // Populate favorite track IDs set
                self.favorite_track_ids = tracks.iter().map(|t| t.id.clone()).collect();
                self.user_favorite_tracks = tracks;
                self.load_images_for_urls(urls)
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
    pub fn handle_mixes_loaded(
        &mut self,
        result: Result<Vec<Mix>, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(mixes) => {
                tracing::info!("Loaded {} mixes", mixes.len());
                self.user_mixes = mixes;
                self.load_images_for_mixes()
            }
            Err(e) => {
                tracing::error!("Failed to load mixes: {}", e);
                self.error_message = Some(format!("Failed to load mixes: {}", e));
                Task::none()
            }
        }
    }

    /// Handle mix tracks loaded result
    pub fn handle_mix_tracks_loaded(
        &mut self,
        result: Result<Vec<Track>, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(tracks) => {
                tracing::info!("Loaded {} mix tracks", tracks.len());
                let urls: Vec<String> = tracks.iter().filter_map(|t| t.cover_url.clone()).collect();
                self.selected_mix_tracks = tracks;
                self.load_images_for_urls(urls)
            }
            Err(e) => {
                tracing::error!("Failed to load mix tracks: {}", e);
                self.error_message = Some(format!("Failed to load mix tracks: {}", e));
                Task::none()
            }
        }
    }

    /// Handle track radio loaded result
    pub fn handle_track_radio_loaded(
        &mut self,
        result: Result<Vec<Track>, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(tracks) => {
                tracing::info!("Loaded {} track radio tracks", tracks.len());
                let urls: Vec<String> = tracks.iter().filter_map(|t| t.cover_url.clone()).collect();
                self.selected_radio_tracks = tracks;
                self.load_images_for_urls(urls)
            }
            Err(e) => {
                tracing::error!("Failed to load track radio: {}", e);
                self.error_message = Some(format!("Failed to load track radio: {}", e));
                Task::none()
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
                let urls: Vec<String> = albums.iter().filter_map(|a| a.cover_url.clone()).collect();
                self.track_detail_artist_albums = albums;
                self.load_images_for_urls(urls)
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
                let picture_urls: Vec<String> = artists
                    .iter()
                    .filter_map(|a| a.picture_url.clone())
                    .collect();
                let artist_ids: Vec<String> = artists.iter().map(|a| a.id.clone()).collect();
                self.track_detail_related_artists = artists;

                // Fetch related albums (one per similar artist) in a follow-up
                let albums_task = if artist_ids.is_empty() {
                    Task::none()
                } else {
                    self.load_track_detail_related_albums(artist_ids)
                };

                Task::batch([self.load_images_for_urls(picture_urls), albums_task])
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
                let urls: Vec<String> = albums.iter().filter_map(|a| a.cover_url.clone()).collect();
                self.track_detail_related_albums = albums;
                self.load_images_for_urls(urls)
            }
            Err(e) => {
                tracing::error!("Failed to load related albums for track detail: {}", e);
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
    pub fn handle_profiles_loaded(
        &mut self,
        result: Result<Vec<Artist>, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_loading = false;
        match result {
            Ok(mut artists) => {
                tracing::info!("Loaded {} followed artists", artists.len());
                artists.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                self.followed_artist_ids = artists.iter().map(|a| a.id.clone()).collect();
                self.user_followed_artists = artists;
                self.load_images_for_profiles()
            }
            Err(e) => {
                tracing::error!("Failed to load followed artists: {}", e);
                self.error_message = Some(format!("Failed to load followed artists: {}", e));
                Task::none()
            }
        }
    }
}
