// SPDX-License-Identifier: MIT

//! Header menu bar for Maré Player standalone mode.
//!
//! This module provides a responsive menu bar (Navigate, Playback, Account)
//! that integrates into the COSMIC CSD header bar via [`header_start()`].
//! It follows the same pattern used by cosmic-files and other COSMIC apps:
//!
//! 1. A [`TidalMenuAction`] enum implements [`MenuAction`] to bridge menu
//!    selections into the application's [`Message`] type.
//! 2. [`menu_bar()`] builds a [`responsive_menu_bar`] widget with top-level
//!    menu categories and their items.
//! 3. The `Application::header_start()` override returns this widget so it
//!    appears in the CSD header bar (left side, next to the window icon).
//!
//! When the window is wide enough the menus are rendered as separate text
//! buttons; when condensed they collapse into a single hamburger overflow
//! menu — all handled automatically by libcosmic.

use std::collections::HashMap;
use std::sync::LazyLock;

use cosmic::app::Core;
use cosmic::widget::menu::action::MenuAction;
use cosmic::widget::menu::key_bind::KeyBind;
use cosmic::widget::menu::{self, ItemHeight, ItemWidth};
use cosmic::widget::responsive_menu_bar;
use cosmic::{Element, theme};

use crate::messages::Message;
use crate::state::AppModel;

/// Stable widget ID for the responsive menu bar (used by libcosmic to track
/// the menu bar's measured size for the collapse/expand logic).
static MENU_ID: LazyLock<cosmic::widget::Id> =
    LazyLock::new(|| cosmic::widget::Id::new("tidal-responsive-menu"));

// ---------------------------------------------------------------------------
// Menu action enum
// ---------------------------------------------------------------------------

/// Actions that can be triggered from menu items.
///
/// This is a separate type from [`Message`] because [`MenuAction`] requires
/// `Clone + Copy + Eq + PartialEq`, which [`Message`] intentionally does not
/// satisfy (it contains non-Copy payloads).  Each variant maps to exactly one
/// [`Message`] via the [`MenuAction`] impl below.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TidalMenuAction {
    // Navigate
    ShowCollection,
    ShowSearch,
    ShowMixes,
    ShowPlaylists,
    ShowAlbums,
    ShowFavoriteTracks,
    ShowProfiles,

    // Playback
    TogglePlayPause,
    NextTrack,
    PreviousTrack,
    CyclePlaybackMode,
    StopPlayback,

    // Account
    ShowSettings,
    Logout,
}

impl MenuAction for TidalMenuAction {
    type Message = Message;

    fn message(&self) -> Message {
        match self {
            // Navigate
            Self::ShowCollection => Message::ShowMain,
            Self::ShowSearch => Message::ShowSearch,
            Self::ShowMixes => Message::ShowMixes,
            Self::ShowPlaylists => Message::ShowPlaylists,
            Self::ShowAlbums => Message::ShowAlbums,
            Self::ShowFavoriteTracks => Message::ShowFavoriteTracks,
            Self::ShowProfiles => Message::ShowProfiles,

            // Playback
            Self::TogglePlayPause => Message::TogglePlayPause,
            Self::NextTrack => Message::NextTrack,
            Self::PreviousTrack => Message::PreviousTrack,
            Self::CyclePlaybackMode => Message::CyclePlaybackMode,
            Self::StopPlayback => Message::StopPlayback,

            // Account
            Self::ShowSettings => Message::ShowSettings,
            Self::Logout => Message::Logout,
        }
    }
}

// ---------------------------------------------------------------------------
// Helper to build a menu item that is enabled or disabled based on a flag
// ---------------------------------------------------------------------------

/// Return a `Button` when `enabled` is true, otherwise a `ButtonDisabled`.
fn menu_button_optional(
    label: &'static str,
    action: TidalMenuAction,
    enabled: bool,
) -> menu::Item<TidalMenuAction, &'static str> {
    if enabled {
        menu::Item::Button(label, None, action)
    } else {
        menu::Item::ButtonDisabled(label, None, action)
    }
}

// ---------------------------------------------------------------------------
// Menu bar builder
// ---------------------------------------------------------------------------

/// Build the responsive header menu bar for standalone mode.
///
/// The returned [`Element`] is meant to be placed inside
/// `Application::header_start()` so it renders in the CSD header bar.
pub fn menu_bar<'a>(
    core: &'a Core,
    app: &'a AppModel,
    key_binds: &'a HashMap<KeyBind, TidalMenuAction>,
) -> Element<'a, Message> {
    let is_playing = app.now_playing.is_some();
    let mode_label = if app.shuffle_enabled {
        "Mode: Shuffle"
    } else {
        match app.loop_status {
            crate::tidal::mpris::LoopStatus::None => "Mode: Normal",
            crate::tidal::mpris::LoopStatus::Track => "Mode: Repeat Track",
            crate::tidal::mpris::LoopStatus::Playlist => "Mode: Repeat All",
        }
    };

    responsive_menu_bar()
        .item_height(ItemHeight::Dynamic(40))
        .item_width(ItemWidth::Uniform(300))
        .spacing(theme::active().cosmic().spacing.space_xxxs.into())
        .into_element(
            core,
            key_binds,
            MENU_ID.clone(),
            Message::Surface,
            vec![
                // ── Navigate ────────────────────────────────────
                (
                    "Navigate",
                    vec![
                        menu::Item::Button("Collection", None, TidalMenuAction::ShowCollection),
                        menu::Item::Button("Search", None, TidalMenuAction::ShowSearch),
                        menu::Item::Divider,
                        menu::Item::Button("Mixes & Radio", None, TidalMenuAction::ShowMixes),
                        menu::Item::Button("Playlists", None, TidalMenuAction::ShowPlaylists),
                        menu::Item::Button("Albums", None, TidalMenuAction::ShowAlbums),
                        menu::Item::Button("Tracks", None, TidalMenuAction::ShowFavoriteTracks),
                        menu::Item::Button("Profiles", None, TidalMenuAction::ShowProfiles),
                    ],
                ),
                // ── Playback ────────────────────────────────────
                (
                    "Playback",
                    vec![
                        menu_button_optional(
                            "Play / Pause",
                            TidalMenuAction::TogglePlayPause,
                            is_playing,
                        ),
                        menu_button_optional("Next Track", TidalMenuAction::NextTrack, is_playing),
                        menu_button_optional(
                            "Previous Track",
                            TidalMenuAction::PreviousTrack,
                            is_playing,
                        ),
                        menu::Item::Divider,
                        menu::Item::Button(mode_label, None, TidalMenuAction::CyclePlaybackMode),
                        menu::Item::Divider,
                        menu_button_optional("Stop", TidalMenuAction::StopPlayback, is_playing),
                    ],
                ),
                // ── Account ─────────────────────────────────────
                (
                    "Account",
                    vec![
                        menu::Item::Button("Settings", None, TidalMenuAction::ShowSettings),
                        menu::Item::Divider,
                        menu::Item::Button("Log Out", None, TidalMenuAction::Logout),
                    ],
                ),
            ],
        )
}
