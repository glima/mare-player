// SPDX-License-Identifier: MIT

//! Favorite and follow toggle handlers for Maré Player.
//!
//! Handles toggling favorite tracks, favorite albums, and followed artists,
//! including the optimistic UI update on the response.

use cosmic::prelude::*;

use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::models::{Album, Artist, Track};

// =============================================================================
// Track Favorite Toggle
// =============================================================================

impl AppModel {
    /// Handle toggle favorite for a track
    pub fn handle_toggle_favorite(&self, track: Track) -> Task<cosmic::Action<Message>> {
        let track_id = track.id.clone();
        let is_favorite = self.favorite_track_ids.contains(&track_id);
        let client = self.tidal_client.clone();

        Task::perform(
            async move {
                let client = client.lock().await;
                if is_favorite {
                    client
                        .remove_favorite_track(&track_id)
                        .await
                        .map(|_| (track, false))
                        .map_err(|e| e.to_string())
                } else {
                    client
                        .add_favorite_track(&track_id)
                        .await
                        .map(|_| (track, true))
                        .map_err(|e| e.to_string())
                }
            },
            |result| cosmic::Action::App(Message::FavoriteToggled(result)),
        )
    }

    /// Handle favorite toggled result
    pub fn handle_favorite_toggled(&mut self, result: Result<(Track, bool), String>) {
        match result {
            Ok((track, is_now_favorite)) => {
                if is_now_favorite {
                    self.favorite_track_ids.insert(track.id.clone());
                    // Add to the displayed list if not already there
                    if !self.user_favorite_tracks.iter().any(|t| t.id == track.id) {
                        self.user_favorite_tracks.insert(0, track);
                    }
                } else {
                    self.favorite_track_ids.remove(&track.id);
                    // Also remove from the displayed list
                    self.user_favorite_tracks.retain(|t| t.id != track.id);
                }
            }
            Err(e) => {
                tracing::error!("Failed to update favorite: {}", e);
                self.error_message = Some(format!("Failed to update favorite: {}", e));
            }
        }
    }
}

// =============================================================================
// Album Favorite Toggle
// =============================================================================

impl AppModel {
    /// Handle toggle favorite for an album
    pub fn handle_toggle_favorite_album(&self, album: Album) -> Task<cosmic::Action<Message>> {
        let album_id = album.id.clone();
        let is_favorite = self.favorite_album_ids.contains(&album_id);
        let client = self.tidal_client.clone();

        Task::perform(
            async move {
                let client = client.lock().await;
                if is_favorite {
                    client
                        .remove_favorite_album(&album_id)
                        .await
                        .map(|_| (album, false))
                        .map_err(|e| e.to_string())
                } else {
                    client
                        .add_favorite_album(&album_id)
                        .await
                        .map(|_| (album, true))
                        .map_err(|e| e.to_string())
                }
            },
            |result| cosmic::Action::App(Message::FavoriteAlbumToggled(result)),
        )
    }

    /// Handle album favorite toggled result
    pub fn handle_favorite_album_toggled(&mut self, result: Result<(Album, bool), String>) {
        match result {
            Ok((album, is_now_favorite)) => {
                if is_now_favorite {
                    self.favorite_album_ids.insert(album.id.clone());
                    // Add to user_albums if not already there
                    if !self.user_albums.iter().any(|a| a.id == album.id) {
                        self.user_albums.insert(0, album);
                    }
                } else {
                    self.favorite_album_ids.remove(&album.id);
                    // Also remove from the displayed list
                    self.user_albums.retain(|a| a.id != album.id);
                }
            }
            Err(e) => {
                tracing::error!("Failed to update album favorite: {}", e);
                self.error_message = Some(format!("Failed to update album favorite: {}", e));
            }
        }
    }
}

// =============================================================================
// Follow / Unfollow Artist
// =============================================================================

impl AppModel {
    /// Handle toggling follow status for an artist
    pub fn handle_toggle_follow_artist(&self, artist: Artist) -> Task<cosmic::Action<Message>> {
        let artist_id = artist.id.clone();
        let is_followed = self.followed_artist_ids.contains(&artist_id);
        let client = self.tidal_client.clone();

        Task::perform(
            async move {
                let client = client.lock().await;
                if is_followed {
                    client
                        .unfollow_artist(&artist_id)
                        .await
                        .map(|_| (artist, false))
                        .map_err(|e| e.to_string())
                } else {
                    client
                        .follow_artist(&artist_id)
                        .await
                        .map(|_| (artist, true))
                        .map_err(|e| e.to_string())
                }
            },
            |result| cosmic::Action::App(Message::FollowArtistToggled(result)),
        )
    }

    /// Handle follow artist toggled result
    pub fn handle_follow_artist_toggled(&mut self, result: Result<(Artist, bool), String>) {
        match result {
            Ok((artist, is_now_followed)) => {
                if is_now_followed {
                    self.followed_artist_ids.insert(artist.id.clone());
                    // Add to user_followed_artists in sorted position if not already there
                    if !self.user_followed_artists.iter().any(|a| a.id == artist.id) {
                        let pos = self
                            .user_followed_artists
                            .iter()
                            .position(|a| a.name.to_lowercase() > artist.name.to_lowercase())
                            .unwrap_or(self.user_followed_artists.len());
                        self.user_followed_artists.insert(pos, artist);
                    }
                } else {
                    self.followed_artist_ids.remove(&artist.id);
                    // Also remove from the displayed list
                    self.user_followed_artists.retain(|a| a.id != artist.id);
                }
            }
            Err(e) => {
                tracing::error!("Failed to update artist follow: {}", e);
                self.error_message = Some(format!("Failed to update artist follow: {}", e));
            }
        }
    }
}
