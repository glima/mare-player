// SPDX-License-Identifier: MIT

//! TIDAL API client module for the COSMIC applet.
//!
//! This module wraps the `tidlers` crate and provides:
//! - OAuth device code authentication flow
//! - Secure credential storage via the system keyring
//! - Session persistence for long-lived authentication
//! - API methods for playlists, albums, tracks, and search
//! - Audio playback via symphonia + PulseAudio

pub mod auth;
pub mod client;
pub mod models;
pub mod mpris;
pub mod play_history;
pub mod player;
