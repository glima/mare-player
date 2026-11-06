// SPDX-License-Identifier: GPL-3.0-only

//! Favorite and follow toggle handlers for Maré Player.
//!
//! Handles toggling favorite tracks, favorite albums, and followed artists,
//! including the optimistic UI update on the response, and the ordering of
//! the favourites lists.

use std::cmp::Ordering;

use cosmic::prelude::*;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::config::{ListSort, SortKey};
use crate::messages::Message;
use crate::state::{AppModel, ViewState};
use crate::tidal::models::{Album, Artist, Track};

// =============================================================================
// Sorting
// =============================================================================

/// Order a favourites list by `sort`.
///
/// A–Z compares `name`s without case or accents, so "Água" sorts with the
/// A's rather than after Z. By date added, entries without a date (cached
/// before dates were kept) go last in either direction, in A–Z order.
pub(crate) fn sort_favorites<T>(
    items: Vec<T>,
    sort: ListSort,
    name: impl Fn(&T) -> &str,
    added_at: impl Fn(&T) -> Option<i64>,
) -> Vec<T> {
    let directed = |order: Ordering| if sort.descending { order.reverse() } else { order };
    let mut keyed: Vec<_> = items.into_iter().map(|item| (collation_key(name(&item)), added_at(&item), item)).collect();
    keyed.sort_by(|(a_name, a_added, _), (b_name, b_added, _)| {
        let by_name = a_name.cmp(b_name);
        match sort.key {
            SortKey::Name => directed(by_name),
            SortKey::DateAdded => match (a_added, b_added) {
                (Some(a), Some(b)) => directed(a.cmp(b)).then(by_name),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => by_name,
            },
        }
    });
    keyed.into_iter().map(|(_, _, item)| item).collect()
}

/// `name` lowercased and without accents, for A–Z ordering.
fn collation_key(name: &str) -> String {
    name.nfd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

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
                    client.remove_favorite_track(&track_id).await.map(|_| (track, false)).map_err(|e| e.to_string())
                } else {
                    client.add_favorite_track(&track_id).await.map(|_| (track, true)).map_err(|e| e.to_string())
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
                        self.user_favorite_tracks.insert(0, Track { added_at: Some(now_ms()), ..track });
                    }
                } else {
                    self.favorite_track_ids.remove(&track.id);
                    // Also remove from the displayed list
                    self.user_favorite_tracks.retain(|t| t.id != track.id);
                }
                if self.view_state == ViewState::FavoriteTracks {
                    self.rebuild_favorites_track_list();
                }
            }
            Err(e) => {
                tracing::error!("Failed to update favorite: {}", e);
                self.error_message = Some(format!("Failed to update favorite: {}", e));
            }
        }
    }

    /// Press a favourite tracks sort button (see [`ListSort::pressed`]) and
    /// reorder the list. Not persisted; [`Self::handle_sort_favorite_tracks`]
    /// also saves it.
    pub(crate) fn press_favorite_tracks_sort(&mut self, key: SortKey) {
        self.config.favorite_tracks_sort = self.config.favorite_tracks_sort.pressed(key);
        self.rebuild_favorites_track_list();
    }

    /// Handle a favourite tracks sort button, remembering the choice.
    pub fn handle_sort_favorite_tracks(&mut self, key: SortKey) {
        self.press_favorite_tracks_sort(key);
        self.save_config("favourite tracks sort");
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
                    client.remove_favorite_album(&album_id).await.map(|_| (album, false)).map_err(|e| e.to_string())
                } else {
                    client.add_favorite_album(&album_id).await.map(|_| (album, true)).map_err(|e| e.to_string())
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
                    client.unfollow_artist(&artist_id).await.map(|_| (artist, false)).map_err(|e| e.to_string())
                } else {
                    client.follow_artist(&artist_id).await.map(|_| (artist, true)).map_err(|e| e.to_string())
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

#[cfg(test)]
mod tests {
    use super::sort_favorites;
    use crate::config::{ListSort, SortKey};
    use crate::state::{AppModel, ViewState};
    use crate::tidal::models::Track;
    use cosmic::Application;

    const A_Z: ListSort = ListSort { key: SortKey::Name, descending: false };
    const Z_A: ListSort = ListSort { key: SortKey::Name, descending: true };
    const OLDEST: ListSort = ListSort { key: SortKey::DateAdded, descending: false };
    const NEWEST: ListSort = ListSort { key: SortKey::DateAdded, descending: true };

    fn track(title: &str, added_at: Option<i64>) -> Track {
        Track { id: title.to_lowercase(), title: title.into(), added_at, ..Default::default() }
    }

    fn sorted(tracks: &[Track], sort: ListSort) -> Vec<String> {
        sort_favorites(tracks.to_vec(), sort, |t| &t.title, |t| t.added_at).into_iter().map(|t| t.title).collect()
    }

    #[test]
    fn a_to_z_ignores_case_and_accents() {
        let tracks = [track("bossa", None), track("Água", None), track("Zebra", None), track("abacate", None)];
        assert_eq!(sorted(&tracks, A_Z), ["abacate", "Água", "bossa", "Zebra"]);
        assert_eq!(sorted(&tracks, Z_A), ["Zebra", "bossa", "Água", "abacate"]);
    }

    #[test]
    fn date_added_orders_by_date_ties_a_to_z_and_undated_last() {
        let tracks = [
            track("Undated b", None),
            track("Middle", Some(2)),
            track("Newest", Some(3)),
            track("Undated a", None),
            track("Oldest", Some(1)),
            track("Also middle", Some(2)),
        ];
        assert_eq!(sorted(&tracks, NEWEST), ["Newest", "Also middle", "Middle", "Oldest", "Undated a", "Undated b"]);
        assert_eq!(sorted(&tracks, OLDEST), ["Oldest", "Also middle", "Middle", "Newest", "Undated a", "Undated b"]);
    }

    fn shown(app: &AppModel) -> Vec<&str> {
        app.track_list_arc.iter().map(|t| t.title.as_str()).collect()
    }

    // `AppModel::init` starts tidlers' play reporter, which needs a runtime.
    #[tokio::test]
    async fn the_favourite_tracks_view_follows_the_sort_buttons() {
        let (mut app, startup) = AppModel::init(cosmic::Core::default(), ());
        drop(startup);
        // `init` reads the real config; start from the default order.
        app.config.favorite_tracks_sort = ListSort::default();
        app.view_state = ViewState::FavoriteTracks;

        // Loading while the view is open shows the tracks, A–Z.
        let loaded = vec![track("Bravo", Some(1)), track("Charlie", Some(3)), track("Alpha", Some(2))];
        drop(app.handle_favorite_tracks_loaded(Ok(loaded)));
        assert_eq!(shown(&app), ["Alpha", "Bravo", "Charlie"]);

        app.press_favorite_tracks_sort(SortKey::DateAdded);
        assert_eq!(shown(&app), ["Charlie", "Alpha", "Bravo"]);
        app.press_favorite_tracks_sort(SortKey::DateAdded);
        assert_eq!(shown(&app), ["Bravo", "Alpha", "Charlie"]);
        app.press_favorite_tracks_sort(SortKey::Name);
        assert_eq!(shown(&app), ["Alpha", "Bravo", "Charlie"]);
        app.press_favorite_tracks_sort(SortKey::Name);
        assert_eq!(shown(&app), ["Charlie", "Bravo", "Alpha"]);

        // The filter keeps the chosen order.
        app.favorite_tracks_filter_visible = true;
        app.favorite_tracks_filter_query = "a".into();
        app.rebuild_favorites_track_list();
        assert_eq!(shown(&app), ["Charlie", "Bravo", "Alpha"]);
        app.favorite_tracks_filter_query = "r".into();
        app.rebuild_favorites_track_list();
        assert_eq!(shown(&app), ["Charlie", "Bravo"]);
        app.favorite_tracks_filter_visible = false;

        // A track liked now is the newest, and unliking removes it at once.
        app.press_favorite_tracks_sort(SortKey::DateAdded);
        app.handle_favorite_toggled(Ok((track("Delta", None), true)));
        assert_eq!(shown(&app), ["Delta", "Charlie", "Alpha", "Bravo"]);
        assert!(app.track_list_arc[0].added_at.is_some());
        app.handle_favorite_toggled(Ok((track("Charlie", None), false)));
        assert_eq!(shown(&app), ["Delta", "Alpha", "Bravo"]);
    }
}
