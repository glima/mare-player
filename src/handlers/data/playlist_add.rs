// SPDX-License-Identifier: GPL-3.0-only

//! "Add to playlist" handlers for Maré Player.
//!
//! The picker adds a track or video to one of the user's playlists, or to a
//! new one, private or public as the user picks. An item already in the
//! playlist is left alone. The picker closes as soon as a choice is made;
//! the result arrives as a brief message, and the playlists are reloaded so
//! their counts, covers and any new playlist show up.

use cosmic::prelude::*;

use crate::messages::Message;
use crate::state::{AddToPlaylist, AppModel, ViewState};
use crate::tidal::models::Track;

/// A finished add: which playlist, and whether the item went in (`false`:
/// it was already there).
#[derive(Debug, Clone)]
pub struct PlaylistAdd {
    pub title: String,
    pub uuid: String,
    pub added: bool,
}

impl AppModel {
    /// Open the picker for `track`. Opened from a playlist (or from a view
    /// reached from one), it leaves that playlist out.
    pub fn handle_show_add_to_playlist(&mut self, track: Track) -> Task<cosmic::Action<Message>> {
        // Reopened from its own now-playing bar, the picker just switches
        // track instead of stacking another copy of itself.
        if self.view_state != ViewState::AddToPlaylist {
            self.nav_stack.push(self.view_state.clone());
        }
        let in_playlist = self.nav_stack.contains(&ViewState::PlaylistDetail);
        let exclude = if in_playlist { self.selected_playlist_uuid.clone() } else { None };
        self.add_to_playlist = Some(AddToPlaylist { track, exclude, ..Default::default() });
        self.view_state = ViewState::AddToPlaylist;
        if !self.user_playlists.is_empty() {
            return Task::none();
        }
        self.is_loading = true;
        Task::batch([
            self.read_view_cache::<Vec<crate::tidal::models::Playlist>, _>("library:playlists", |p| {
                Message::PlaylistsLoaded(Ok(p))
            }),
            self.load_playlists(),
        ])
    }

    /// Add the picker's item to the playlist `uuid` and close the picker.
    pub fn handle_add_to_playlist(&mut self, uuid: String) -> Task<cosmic::Action<Message>> {
        let Some(picker) = self.add_to_playlist.take() else {
            return Task::none();
        };
        let title = self.user_playlists.iter().find(|p| p.uuid == uuid).map(|p| p.title.clone()).unwrap_or_default();
        let client = self.tidal_client.clone();
        let add = Task::perform(
            async move {
                let client = client.lock().await;
                let added = client.add_to_playlist(&uuid, &picker.track).await.map_err(|e| e.to_string())?;
                Ok(PlaylistAdd { title, uuid, added })
            },
            |result| cosmic::Action::App(Message::AddedToPlaylist(result)),
        );
        Task::batch([self.handle_navigate_back(), add])
    }

    /// Create the picker's new playlist with its item in it and close the
    /// picker. Does nothing until the playlist has a name.
    pub fn handle_create_playlist_with_track(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(title) = self.add_to_playlist.as_ref().map(|p| p.new_name.trim().to_string()) else {
            return Task::none();
        };
        if title.is_empty() {
            return Task::none();
        }
        let Some(picker) = self.add_to_playlist.take() else {
            return Task::none();
        };
        let client = self.tidal_client.clone();
        let create = Task::perform(
            async move {
                let client = client.lock().await;
                let uuid = client.create_playlist(&title, picker.new_public).await.map_err(|e| e.to_string())?;
                let added = client.add_to_playlist(&uuid, &picker.track).await.map_err(|e| e.to_string())?;
                Ok(PlaylistAdd { title, uuid, added })
            },
            |result| cosmic::Action::App(Message::AddedToPlaylist(result)),
        );
        Task::batch([self.handle_navigate_back(), create])
    }

    /// Report an add, and reload the playlists unless nothing changed. A
    /// failure reloads them too: creating may have worked before adding
    /// failed.
    pub fn handle_added_to_playlist(&mut self, result: Result<PlaylistAdd, String>) -> Task<cosmic::Action<Message>> {
        let note = match result {
            Ok(PlaylistAdd { title, added: false, .. }) => {
                return self.show_note(crate::fl!("already-in-playlist", playlist = title));
            }
            Ok(PlaylistAdd { title, uuid, added: true }) => {
                // The 2×2 cover grid comes from the first tracks, so a short
                // playlist's grid may change: drop it to have it redrawn.
                self.playlist_thumbnails.remove(&uuid);
                self.show_note(crate::fl!("added-to-playlist", playlist = title))
            }
            Err(e) => {
                tracing::error!("Failed to add to playlist: {}", e);
                self.error_message = Some(format!("Failed to add to playlist: {}", e));
                Task::none()
            }
        };
        Task::batch([note, self.load_playlists()])
    }
}

#[cfg(test)]
mod tests {
    use super::PlaylistAdd;
    use crate::state::{AppModel, ViewState};
    use crate::tidal::models::{Playlist, Track};
    use cosmic::Application;

    fn app() -> AppModel {
        let (mut app, startup) = AppModel::init(cosmic::Core::default(), ());
        drop(startup);
        app.user_playlists =
            ["a", "b"].map(|uuid| Playlist { uuid: uuid.into(), title: uuid.to_uppercase(), ..Default::default() }).into();
        app
    }

    fn track() -> Track {
        Track { id: "t1".into(), title: "Song".into(), ..Default::default() }
    }

    #[tokio::test]
    async fn the_picker_leaves_out_the_playlist_it_was_opened_from() {
        let mut app = app();
        app.view_state = ViewState::Playlists;
        drop(app.handle_show_playlist_detail("a".into(), "A".into()));
        drop(app.handle_show_add_to_playlist(track()));
        assert_eq!(app.view_state, ViewState::AddToPlaylist);
        assert_eq!(app.add_to_playlist.as_ref().unwrap().exclude.as_deref(), Some("a"));
        // Reopening it for another track doesn't stack a second picker.
        drop(app.handle_show_add_to_playlist(Track { id: "t2".into(), ..track() }));
        assert_eq!(app.nav_stack, [ViewState::Playlists, ViewState::PlaylistDetail]);
        assert_eq!(app.add_to_playlist.as_ref().unwrap().track.id, "t2");

        // Out of the playlist, the last one opened is offered again.
        drop(app.handle_navigate_back());
        drop(app.handle_navigate_back());
        drop(app.handle_show_add_to_playlist(track()));
        assert_eq!(app.add_to_playlist.as_ref().unwrap().exclude, None);
    }

    #[tokio::test]
    async fn picking_closes_the_picker_and_a_new_playlist_needs_a_name() {
        let mut app = app();
        app.view_state = ViewState::FavoriteTracks;
        drop(app.handle_show_add_to_playlist(track()));
        drop(app.handle_create_playlist_with_track());
        assert_eq!(app.view_state, ViewState::AddToPlaylist, "no name, no playlist");
        app.add_to_playlist.as_mut().unwrap().new_name = "  ".into();
        drop(app.handle_create_playlist_with_track());
        assert_eq!(app.view_state, ViewState::AddToPlaylist, "a blank name isn't a name");

        drop(app.handle_add_to_playlist("b".into()));
        assert_eq!(app.view_state, ViewState::FavoriteTracks);
        assert!(app.add_to_playlist.is_none());
    }

    #[tokio::test]
    async fn the_result_is_reported() {
        let mut app = app();
        let done = |added| Ok(PlaylistAdd { title: "B".into(), uuid: "b".into(), added });
        drop(app.handle_added_to_playlist(done(false)));
        assert!(app.error_message.as_deref().unwrap().contains("B"));
        drop(app.handle_added_to_playlist(done(true)));
        assert!(app.error_message.as_deref().unwrap().contains("B"));
        drop(app.handle_added_to_playlist(Err("HTTP 412".into())));
        assert_eq!(app.error_message.as_deref(), Some("Failed to add to playlist: HTTP 412"));
    }
}
