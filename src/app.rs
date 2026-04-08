// SPDX-License-Identifier: MIT

//! COSMIC application implementation for Maré Player.
//!
//! This module wires up the [`cosmic::Application`] trait — defining the
//! init, update, view, and subscription lifecycle — and re-exports the
//! core types ([`AppModel`], [`Message`], [`ViewState`]) that the rest
//! of the crate depends on.

use crate::config::Config;
use crate::image_cache::ImageCache;
#[cfg(not(feature = "panel-applet"))]
use crate::menu;
use crate::tidal::{
    client::TidalAppClient,
    player::{PlaybackState, Player},
};
use crate::views::visualizer::VisualizerState;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::keyboard::Key;
use cosmic::iced::window::Id;
use cosmic::iced::{Subscription, time};
use cosmic::prelude::*;
use futures_util::SinkExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

// Re-export types from state and messages modules
pub use crate::messages::Message;
pub use crate::state::{AppModel, ViewState};

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;

    type Flags = ();

    type Message = Message;

    /// Unique identifier in RDNN (reverse domain name notation) format.
    #[cfg(feature = "panel-applet")]
    const APP_ID: &'static str = "io.github.cosmic-applet-mare";
    #[cfg(not(feature = "panel-applet"))]
    const APP_ID: &'static str = "io.github.mare-player";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    /// Initializes the application with any given flags and startup commands.
    fn init(
        // `core` is only mutated in standalone mode (`core.set_header_title`);
        // in panel-applet mode the binding is never written.
        #[allow(unused_mut)] mut core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        // In standalone mode, set the CSD header bar title early on core.
        #[cfg(not(feature = "panel-applet"))]
        core.set_header_title("Maré Player".to_string());

        let config = cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
            .map(|context| match Config::get_entry(&context) {
                Ok(config) => config,
                Err((_errors, config)) => config,
            })
            .unwrap_or_default();

        // Initialize audio player
        let mut visualizer_state = VisualizerState::new();
        let player = match Player::new() {
            Ok(p) => {
                tracing::info!("Audio player initialized");
                // Apply saved volume from config
                let saved_volume = config.volume_level.clamp(0.0, 1.0);
                if let Err(e) = p.set_volume(saved_volume) {
                    tracing::warn!(
                        "Failed to set initial volume to {:.0}%: {}",
                        saved_volume * 100.0,
                        e
                    );
                } else {
                    tracing::info!("Restored volume to {:.0}%", saved_volume * 100.0);
                }
                // Give the visualizer widget a direct handle to the spectrum
                // analyzer so it can self-animate without going through
                // update() → view().
                visualizer_state.set_analyzer(p.spectrum_analyzer());
                Some(p)
            }
            Err(e) => {
                tracing::warn!("Failed to initialize audio player: {}", e);
                None
            }
        };

        let image_cache_max_mb = config.image_cache_max_mb;
        let audio_cache_max_mb = config.audio_cache_max_mb;
        let saved_volume = config.volume_level.clamp(0.0, 1.0);

        // Build the single TidalAppClient up-front so we can load persisted
        // play history from its API cache *before* moving it into the Arc.
        let client = TidalAppClient::new_with_audio_cache_mb(audio_cache_max_mb);
        let play_history = crate::tidal::play_history::PlayHistory::load(client.api_cache());

        // `app` is only mutated in standalone mode (`app.set_window_title`);
        // in panel-applet mode the binding is never written.
        #[allow(unused_mut)]
        let mut app = AppModel {
            core,
            config,
            popup: None,
            tidal_client: Arc::new(Mutex::new(client)),
            play_history,
            track_list_content: Default::default(),
            track_list_arc: Arc::from([]),
            history_filter_visible: false,
            history_filter_query: String::new(),
            favorite_tracks_filter_visible: false,
            favorite_tracks_filter_query: String::new(),
            view_state: ViewState::Loading,
            device_code_info: None,
            search_query: String::new(),
            search_results: None,
            user_playlists: Vec::new(),
            playlist_thumbnails: HashMap::new(),
            user_albums: Vec::new(),
            user_favorite_tracks: Vec::new(),
            user_mixes: Vec::new(),
            user_followed_artists: Vec::new(),
            selected_mix_tracks: Vec::new(),
            selected_mix_name: None,
            selected_radio_tracks: Vec::new(),
            selected_radio_source_track: None,
            selected_detail_track: None,
            track_detail_artist_albums: Vec::new(),
            track_detail_related_artists: Vec::new(),
            track_detail_related_albums: Vec::new(),
            selected_playlist_tracks: Vec::new(),
            selected_album_tracks: Vec::new(),
            selected_playlist_name: None,
            selected_album: None,
            selected_artist: None,
            selected_artist_top_tracks: Vec::new(),
            selected_artist_albums: Vec::new(),
            favorite_album_ids: HashSet::new(),
            followed_artist_ids: HashSet::new(),
            nav_stack: Vec::new(),
            is_loading: true,
            error_message: None,
            session_restore_attempted: false,
            player,
            playback_state: PlaybackState::Stopped,
            now_playing: None,
            playback_position: 0.0,
            playback_queue: Vec::new(),
            playback_queue_index: 0,
            shuffle_enabled: false,
            loop_status: crate::tidal::mpris::LoopStatus::None,
            playback_context: None,
            image_cache: ImageCache::new(image_cache_max_mb),
            loaded_images: HashMap::new(),
            pending_image_loads: HashSet::new(),
            favorite_track_ids: HashSet::new(),
            mpris_handle: None,
            mpris_command_rx: None,
            search_debounce_version: 0,
            visualizer_state,
            loading_progress: 0.0,
            pending_seek: None,
            seek_debounce_version: 0,
            volume_level: saved_volume,
            show_volume_bar: false,
            volume_bar_shown_at: None,
            #[cfg(not(feature = "panel-applet"))]
            show_volume_popup: false,
            window_width: 0.0,
            #[cfg(not(feature = "panel-applet"))]
            menu_key_binds: HashMap::new(),
        };

        // In standalone mode, set the Wayland/compositor window title so the
        // SSD header bar displays "Maré Player".
        #[cfg(not(feature = "panel-applet"))]
        let title_task: Task<cosmic::Action<Self::Message>> = {
            let main_id = app.core().main_window_id();
            tracing::info!("Standalone title setup: main_window_id = {:?}", main_id);
            if let Some(id) = main_id {
                let task = app.set_window_title("Maré Player".to_string(), id);
                task
            } else {
                tracing::warn!(
                    "main_window_id() returned None — cannot set window title during init"
                );
                Task::none()
            }
        };
        #[cfg(feature = "panel-applet")]
        let title_task: Task<cosmic::Action<Self::Message>> = Task::none();

        // Start MPRIS service
        let mpris_task = Task::perform(
            async {
                crate::tidal::mpris::start_mpris_service()
                    .await
                    .map(|(handle, rx)| (handle, Arc::new(Mutex::new(rx))))
            },
            |result| match result {
                Ok((handle, rx)) => {
                    cosmic::Action::App(Message::MprisServiceStarted(Ok((handle, rx))))
                }
                Err(e) => cosmic::Action::App(Message::MprisServiceStarted(Err(e))),
            },
        );

        (app, Task::batch([mpris_task, title_task]))
    }

    /// Track the current window size so views can scale text limits, etc.
    fn on_window_resize(&mut self, _id: Id, width: f32, _height: f32) {
        self.window_width = width;
    }

    #[cfg(feature = "panel-applet")]
    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    /// Place the responsive menu bar (Navigate, Playback, Account) on the
    /// left side of the CSD header bar in standalone mode.
    ///
    /// In panel-applet mode there is no header bar, so this returns nothing.
    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        #[cfg(not(feature = "panel-applet"))]
        {
            vec![menu::menu_bar(self.core(), self, &self.menu_key_binds)]
        }
        #[cfg(feature = "panel-applet")]
        {
            vec![]
        }
    }

    /// Place a search button on the right side of the CSD header bar in
    /// standalone mode.
    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        #[cfg(not(feature = "panel-applet"))]
        {
            vec![
                cosmic::widget::button::icon(cosmic::widget::icon::from_name(
                    "system-search-symbolic",
                ))
                .on_press(Message::ShowSearch)
                .padding(8)
                .into(),
            ]
        }
        #[cfg(feature = "panel-applet")]
        {
            vec![]
        }
    }

    /// Describes the interface based on the current state of the application model.
    ///
    /// In **panel-applet** mode this renders the small panel button (delegating
    /// to [`AppModel::view_panel`]).  In **standalone** mode the full content
    /// view is shown directly inside the application window.
    fn view(&self) -> Element<'_, Self::Message> {
        #[cfg(feature = "panel-applet")]
        {
            self.view_panel()
        }
        #[cfg(not(feature = "panel-applet"))]
        {
            self.view_standalone()
        }
    }

    /// The applet's popup window will be drawn using this view method.
    /// Delegates to the popup view module.
    #[cfg(feature = "panel-applet")]
    fn view_window(&self, id: Id) -> Element<'_, Self::Message> {
        self.view_popup(id)
    }

    /// Register subscriptions for this application.
    fn subscription(&self) -> Subscription<Self::Message> {
        struct MySubscription;

        let mut subs = vec![
            // Create a subscription which emits updates through a channel.
            Subscription::run_with(std::any::TypeId::of::<MySubscription>(), |_| {
                cosmic::iced::stream::channel(4, async move |mut channel| {
                    _ = channel.send(Message::SubscriptionChannel).await;
                    futures_util::future::pending().await
                })
            }),
            // Watch for application configuration changes.
            self.core()
                .watch_config::<Config>(Self::APP_ID)
                .map(|update| Message::UpdateConfig(update.config)),
        ];

        // Only tick when something needs it: active playback, visualizer
        // fade-out in progress, or volume bar auto-hide pending.
        //
        // The visualizer widget self-animates via `shell.request_redraw()`
        // and never emits a Message, so the tick only needs to:
        //   • update the seek-slider position (~1 Hz visual change)
        //   • poll engine events (track-ended, errors, state transitions)
        //   • auto-hide the volume bar after ~1 s
        // 500 ms is more than enough for all of these.
        let needs_tick = self.playback_state == PlaybackState::Playing
            || self.playback_state == PlaybackState::Loading
            || self.visualizer_state.needs_tick()
            || self.show_volume_bar;

        if needs_tick {
            subs.push(
                time::every(std::time::Duration::from_millis(500)).map(|_| Message::PlaybackTick),
            );
        }

        // Screenshot hotkey: Ctrl+Shift+S
        subs.push(
            cosmic::iced::keyboard::listen().filter_map(|event| match event {
                cosmic::iced::keyboard::Event::KeyPressed { key, modifiers, .. }
                    if modifiers.control() && modifiers.shift() =>
                {
                    match key.as_ref() {
                        Key::Character("s" | "S") => Some(Message::TakeScreenshot),
                        _ => None,
                    }
                }
                _ => None,
            }),
        );

        // Add MPRIS command subscription.
        // We wrap the Arc receiver in a newtype that implements Hash (by
        // pointer identity) so it can be passed as `data` to `run_with`.
        // The `fn(&D) -> S` builder dereferences and clones the Arc from
        // the wrapper without capturing any external state.
        if let Some(rx) = &self.mpris_command_rx {
            /// Newtype wrapper around the MPRIS command receiver that
            /// implements [`Hash`] via the [`Arc`] pointer address, allowing
            /// it to be used as `run_with` subscription data.
            struct MprisRx(
                Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<crate::tidal::mpris::MprisCommand>>>,
            );

            impl std::hash::Hash for MprisRx {
                fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                    Arc::as_ptr(&self.0).hash(state);
                }
            }

            subs.push(Subscription::run_with(
                MprisRx(rx.clone()),
                |data: &MprisRx| {
                    let rx = data.0.clone();
                    cosmic::iced::stream::channel(4, async move |mut channel| {
                        let mut rx = rx.lock().await;
                        while let Some(cmd) = rx.recv().await {
                            if channel.send(Message::MprisCommand(cmd)).await.is_err() {
                                break;
                            }
                        }
                        futures_util::future::pending().await
                    })
                },
            ));
        }

        Subscription::batch(subs)
    }

    /// Handles messages emitted by the application and its widgets.
    ///
    /// This function dispatches messages to the appropriate handler modules:
    /// - `handlers::auth` - Authentication (login, OAuth, logout)
    /// - `handlers::navigation` - View state transitions
    /// - `handlers::data` - Data loading, further split into:
    ///   - `data::library` - Playlists, albums, mixes, profiles, artist/album/track detail
    ///   - `data::search` - Search query debouncing and result handling
    ///   - `data::favorites` - Favorite track/album toggle and follow/unfollow artist
    ///   - `data::thumbnails` - 2×2 playlist grid thumbnail generation
    /// - `handlers::playback` - Playback control (play, pause, seek, queue)
    /// - `handlers::misc` - Config, images, sharing, MPRIS
    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        // Log all incoming messages for debugging
        match &message {
            // Skip logging for very frequent messages
            Message::SubscriptionChannel
            | Message::PlaybackTick
            | Message::ClearError
            | Message::MprisCommand(_)
            | Message::PerformSearchDebounced(_)
            | Message::SearchQueryChanged(_)
            | Message::HistoryFilterChanged(_)
            | Message::FavoriteTracksFilterChanged(_)
            | Message::AdjustVolume(_)
            | Message::SetVolume(_)
            | Message::ArtistTopTracksLoaded(_)
            | Message::ArtistAlbumsLoaded(_)
            | Message::ToggleVolumePopup
            | Message::CloseVolumePopup
            | Message::Surface(_) => {}
            // Log image loads at trace level (too frequent, dumps huge byte arrays)
            Message::LoadImage(_)
            | Message::ImageLoaded(_, _, _, _)
            | Message::PlaylistThumbnailGenerated(_, _, _, _)
            | Message::GeneratePlaylistThumbnails
            | Message::ScreenshotCaptured(_) => {
                tracing::trace!("update() received: {:?}", message);
            }
            // Log data loads at debug level
            Message::PlaylistsLoaded(_)
            | Message::AlbumsLoaded(_)
            | Message::PlaylistTracksLoaded(_)
            | Message::AlbumTracksLoaded(_)
            | Message::AlbumInfoLoaded(_)
            | Message::AlbumReviewLoaded(_)
            | Message::ArtistInfoLoaded(_)
            | Message::FavoriteTracksLoaded(_)
            | Message::SearchComplete(_)
            | Message::MixesLoaded(_)
            | Message::MixTracksLoaded(_)
            | Message::TrackRadioLoaded(_)
            | Message::TrackDetailArtistAlbumsLoaded(_)
            | Message::TrackDetailRelatedArtistsLoaded(_)
            | Message::TrackDetailRelatedAlbumsLoaded(_)
            | Message::ProfilesLoaded(_)
            | Message::FollowArtistToggled(_) => {
                tracing::debug!("update() received: {:?}", message);
            }
            // Log important messages at info level
            msg => tracing::info!("update() received: {:?}", msg),
        }

        // Dispatch to handler modules
        match message {
            // Misc handlers - startup and config
            Message::SubscriptionChannel => self.handle_subscription_channel(),
            Message::UpdateConfig(config) => {
                self.handle_update_config(config);
                Task::none()
            }

            // Navigation handlers
            Message::TogglePopup => self.handle_toggle_popup(),
            Message::PopupClosed(id) => {
                self.handle_popup_closed(id);
                Task::none()
            }
            Message::ShowMain => {
                self.handle_show_main();
                Task::none()
            }
            Message::ShowSearch => self.handle_show_search(),
            Message::ShowSettings => self.handle_show_settings(),
            Message::ShowHistory => self.handle_show_history(),
            Message::ToggleHistoryFilter => {
                self.history_filter_visible = !self.history_filter_visible;
                if self.history_filter_visible {
                    cosmic::widget::text_input::focus(cosmic::widget::Id::new(
                        "history-filter-input",
                    ))
                } else {
                    self.history_filter_query.clear();
                    self.rebuild_history_track_list();
                    Task::none()
                }
            }
            Message::HistoryFilterChanged(query) => {
                self.history_filter_query = query;
                self.rebuild_history_track_list();
                Task::none()
            }
            Message::ToggleFavoriteTracksFilter => {
                self.favorite_tracks_filter_visible = !self.favorite_tracks_filter_visible;
                if self.favorite_tracks_filter_visible {
                    cosmic::widget::text_input::focus(cosmic::widget::Id::new(
                        "favorite-tracks-filter-input",
                    ))
                } else {
                    self.favorite_tracks_filter_query.clear();
                    self.rebuild_favorites_track_list();
                    Task::none()
                }
            }
            Message::FavoriteTracksFilterChanged(query) => {
                self.favorite_tracks_filter_query = query;
                self.rebuild_favorites_track_list();
                Task::none()
            }
            Message::ShowMixes => self.handle_show_mixes(),
            Message::ShowPlaylists => self.handle_show_playlists(),
            Message::ShowAlbums => self.handle_show_albums(),
            Message::ShowFavoriteTracks => self.handle_show_favorite_tracks(),
            Message::ShowProfiles => self.handle_show_profiles(),
            Message::ShowMixDetail(mix_id, mix_name) => {
                self.handle_show_mix_detail(mix_id, mix_name)
            }
            Message::ShowPlaylistDetail(uuid, name) => self.handle_show_playlist_detail(uuid, name),
            Message::ShowAlbumDetail(album) => self.handle_show_album_detail(album),
            Message::ShowAlbumDetailById(album_id) => self.handle_show_album_detail_by_id(album_id),
            Message::ShowArtistDetail(artist_id) => self.handle_show_artist_detail(artist_id),
            Message::NavigateBack => self.handle_navigate_back(),

            // Auth handlers
            Message::StartLogin => self.handle_start_login(),
            Message::LoginOAuthReceived(result) => self.handle_login_oauth_received(result),
            Message::OpenOAuthUrl => {
                self.handle_open_oauth_url();
                Task::none()
            }
            Message::OAuthComplete(result) => self.handle_oauth_complete(result),
            Message::SessionRestored(result) => self.handle_session_restored(result),
            Message::Logout => self.handle_logout(),

            // Data handlers - mixes
            Message::LoadMixes => self.handle_load_mixes(),
            Message::MixesLoaded(result) => self.handle_mixes_loaded(result),
            Message::MixTracksLoaded(result) => self.handle_mix_tracks_loaded(result),

            // Data handlers - track radio
            Message::ShowTrackRadio(track) => self.handle_show_track_radio(track),
            Message::TrackRadioLoaded(result) => self.handle_track_radio_loaded(result),

            // Data handlers - track detail (recommendations)
            Message::ShowTrackDetail(track) => self.handle_show_track_detail(track),
            Message::TrackDetailArtistAlbumsLoaded(result) => {
                self.handle_track_detail_artist_albums_loaded(result)
            }
            Message::TrackDetailRelatedArtistsLoaded(result) => {
                self.handle_track_detail_related_artists_loaded(result)
            }
            Message::TrackDetailRelatedAlbumsLoaded(result) => {
                self.handle_track_detail_related_albums_loaded(result)
            }

            // Data handlers - profiles
            Message::LoadProfiles => self.handle_load_profiles(),
            Message::ProfilesLoaded(result) => self.handle_profiles_loaded(result),

            // Data handlers - search
            Message::SearchQueryChanged(query) => self.handle_search_query_changed(query),
            Message::PerformSearchDebounced(version) => {
                self.handle_perform_search_debounced(version)
            }
            Message::PerformSearch => self.handle_perform_search(),
            Message::SearchComplete(result) => self.handle_search_complete(result),

            // Data handlers - playlists
            Message::LoadPlaylists => self.handle_load_playlists(),
            Message::PlaylistsLoaded(result) => self.handle_playlists_loaded(result),
            Message::PlaylistTracksLoaded(result) => self.handle_playlist_tracks_loaded(result),
            Message::GeneratePlaylistThumbnails => self.handle_generate_playlist_thumbnails(),
            Message::PlaylistThumbnailGenerated(uuid, width, height, pixels) => {
                self.handle_playlist_thumbnail_generated(uuid, width, height, pixels);
                Task::none()
            }

            // Data handlers - albums
            Message::LoadAlbums => self.handle_load_albums(),
            Message::AlbumsLoaded(result) => self.handle_albums_loaded(result),
            Message::AlbumTracksLoaded(result) => self.handle_album_tracks_loaded(result),
            Message::AlbumInfoLoaded(result) => self.handle_album_info_loaded(result),
            Message::AlbumReviewLoaded(result) => {
                self.handle_album_review_loaded(result);
                Task::none()
            }

            // Data handlers - artist detail
            Message::ArtistInfoLoaded(result) => self.handle_artist_info_loaded(result),
            Message::ArtistTopTracksLoaded(result) => self.handle_artist_top_tracks_loaded(result),
            Message::ArtistAlbumsLoaded(result) => self.handle_artist_albums_loaded(result),

            // Data handlers - favorites
            Message::LoadFavoriteTracks => self.handle_load_favorite_tracks(),
            Message::FavoriteTracksLoaded(result) => self.handle_favorite_tracks_loaded(result),
            Message::ToggleFavorite(track) => self.handle_toggle_favorite(track),
            Message::FavoriteToggled(result) => {
                self.handle_favorite_toggled(result);
                Task::none()
            }
            Message::ToggleFavoriteAlbum(album) => self.handle_toggle_favorite_album(album),
            Message::FavoriteAlbumToggled(result) => {
                self.handle_favorite_album_toggled(result);
                Task::none()
            }
            Message::ToggleFollowArtist(artist) => self.handle_toggle_follow_artist(artist),
            Message::FollowArtistToggled(result) => {
                self.handle_follow_artist_toggled(result);
                Task::none()
            }

            // Playback handlers
            Message::PlayTrackList(tracks, index, context) => {
                self.handle_play_track_list(tracks, index, context)
            }
            Message::ShufflePlay(tracks, context) => self.handle_shuffle_play(tracks, context),
            Message::NextTrack => self.handle_next_track(),
            Message::PreviousTrack => self.handle_previous_track(),
            Message::ToggleShuffle => {
                self.handle_toggle_shuffle();
                Task::none()
            }
            Message::CyclePlaybackMode => {
                use crate::tidal::mpris::LoopStatus;
                // Cycle: Off → Shuffle → Repeat All → Repeat Track → Off
                if !self.shuffle_enabled && self.loop_status == LoopStatus::None {
                    // Off → Shuffle
                    self.handle_toggle_shuffle();
                } else if self.shuffle_enabled {
                    // Shuffle → Repeat All (disable shuffle, enable playlist loop)
                    self.handle_toggle_shuffle(); // turns shuffle off
                    self.loop_status = LoopStatus::Playlist;
                } else if self.loop_status == LoopStatus::Playlist {
                    // Repeat All → Repeat Track
                    self.loop_status = LoopStatus::Track;
                } else {
                    // Repeat Track → Off
                    self.loop_status = LoopStatus::None;
                }
                self.update_mpris_state()
            }
            Message::SetLoopStatus(status) => {
                if status != self.loop_status {
                    self.loop_status = status;
                    self.update_mpris_state()
                } else {
                    Task::none()
                }
            }
            Message::PlaybackUrlReceived(result) => self.handle_playback_url_received(result),
            Message::PreloadNextTrack => self.handle_preload_next_track(),
            Message::PreloadUrlReceived(result) => self.handle_preload_url_received(result),
            Message::GaplessTransition => self.handle_gapless_transition(),
            Message::SeekTo(percent) => self.handle_seek_to(percent),
            Message::SeekDebounced(version) => self.handle_seek_debounced(version),
            Message::TogglePlayPause => self.handle_toggle_play_pause(),
            Message::StopPlayback => self.handle_stop_playback(),
            Message::PlaybackTick => self.handle_playback_tick(),

            // Misc handlers - errors and images
            Message::ClearError => {
                self.handle_clear_error();
                Task::none()
            }
            Message::LoadImage(url) => self.handle_load_image(url),
            Message::ImageLoaded(url, width, height, pixels) => {
                self.handle_image_loaded(url, width, height, pixels);
                Task::none()
            }

            // Misc handlers - settings
            Message::SetAudioQuality(quality) => self.handle_set_audio_quality(quality),
            Message::SetAudioCacheMaxMb(mb) => {
                self.handle_set_audio_cache_max_mb(mb);
                Task::none()
            }
            Message::ClearAudioCache => {
                self.handle_clear_audio_cache();
                Task::none()
            }
            Message::ClearHistory => {
                self.handle_clear_history();
                Task::none()
            }

            // Misc handlers - sharing
            Message::ShowSharePrompt(track) => {
                self.handle_show_share_prompt(track);
                Task::none()
            }
            Message::ShareTrack(track_id, track_title) => {
                self.handle_share_track(track_id, track_title)
            }
            Message::ShareAlbum(album_id, album_title) => {
                self.handle_share_album(album_id, album_title)
            }
            Message::CancelShare => {
                self.handle_cancel_share();
                Task::none()
            }
            Message::ShareLinkGenerated(result) => self.handle_share_link_generated(result),

            // Misc handlers - MPRIS
            Message::MprisServiceStarted(result) => self.handle_mpris_service_started(result),
            Message::MprisCommand(cmd) => self.handle_mpris_command(cmd),

            // Volume control
            Message::AdjustVolume(delta) => self.handle_adjust_volume(delta),
            Message::SetVolume(level) => {
                let delta = level.clamp(0.0, 1.0) - self.volume_level;
                self.handle_adjust_volume(delta)
            }
            Message::ToggleVolumePopup => {
                #[cfg(not(feature = "panel-applet"))]
                {
                    self.show_volume_popup = !self.show_volume_popup;
                }
                Task::none()
            }
            Message::CloseVolumePopup => {
                #[cfg(not(feature = "panel-applet"))]
                {
                    self.show_volume_popup = false;
                }
                Task::none()
            }

            // Screenshot
            Message::TakeScreenshot => self.handle_take_screenshot(),
            Message::ScreenshotCaptured(screenshot) => {
                self.handle_screenshot_captured(screenshot);
                Task::none()
            }

            // Wayland surface action forwarding (responsive menu bar popups)
            Message::Surface(action) => {
                cosmic::task::message(cosmic::Action::Cosmic(cosmic::app::Action::Surface(action)))
            }
        }
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        #[cfg(feature = "panel-applet")]
        {
            Some(cosmic::applet::style())
        }
        #[cfg(not(feature = "panel-applet"))]
        {
            None
        }
    }
}
