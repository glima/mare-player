// SPDX-License-Identifier: GPL-3.0-only

//! TIDAL API client module for the COSMIC applet.
//!
//! This module wraps the `tidlers` crate and provides:
//! - OAuth PKCE authentication flow (the only flow TIDAL serves hi-res to)
//! - Secure credential storage via the system keyring
//! - Session persistence for long-lived authentication
//! - API methods for playlists, albums, tracks, and search
//! - Audio playback via symphonia + PulseAudio

pub mod auth;
pub mod client;
pub mod client_identity;
pub mod login_uri;
pub mod models;
pub mod mpris;
pub mod play_history;
pub mod play_reporter;
pub mod player;
