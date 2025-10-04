// SPDX-License-Identifier: MIT

//! Data loading message handlers for Maré Player.
//!
//! This module handles all data-fetching concerns, split by domain:
//!
//! - `library` - Playlists, albums, mixes, profiles, artist/album/track detail
//! - `search` - Search query debouncing and result handling
//! - `favorites` - Favorite track/album toggle and follow/unfollow artist
//! - `thumbnails` - 2×2 playlist grid thumbnail generation

pub mod favorites;
pub mod library;
pub mod search;
pub mod thumbnails;
