// SPDX-License-Identifier: MIT

//! Message handlers for Maré Player.
//!
//! This module contains all the message handling logic, split by domain:
//!
//! - `auth` - Authentication handlers (login, OAuth, logout, session restore)
//! - `playback` - Playback control handlers (play, pause, seek, queue, shuffle)
//! - `navigation` - View state transition handlers
//! - `data` - Data loading handlers, further split into:
//!   - `library` - Playlists, albums, mixes, profiles, artist/album/track detail
//!   - `search` - Search query debouncing and result handling
//!   - `favorites` - Favorite track/album toggle and follow/unfollow artist
//!   - `thumbnails` - 2×2 playlist grid thumbnail generation
//! - `misc` - Miscellaneous handlers (config, errors, images, sharing, MPRIS)

pub mod auth;
pub mod data;
pub mod misc;
pub mod navigation;
pub mod playback;
