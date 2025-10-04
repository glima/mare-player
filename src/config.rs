// SPDX-License-Identifier: MIT

//! Persisted configuration schema for Maré Player.
//!
//! Settings are stored via COSMIC's config system and survive restarts.
//! The [`Config`] struct is the single source of truth for user preferences
//! such as audio quality, cache limits, and notification toggles.

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};
use serde::{Deserialize, Serialize};

/// Audio quality settings for TIDAL playback
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioQuality {
    /// Low quality (96 kbps AAC)
    Low,
    /// High quality (320 kbps AAC)
    High,
    /// Lossless quality (FLAC 16-bit/44.1kHz)
    Lossless,
    /// Hi-Res quality (FLAC up to 24-bit/192kHz)
    #[default]
    HiRes,
}

impl AudioQuality {
    /// Get display name for the quality setting
    pub fn display_name(&self) -> &'static str {
        match self {
            AudioQuality::Low => "Low (96 kbps)",
            AudioQuality::High => "High (320 kbps)",
            AudioQuality::Lossless => "Lossless (CD Quality)",
            AudioQuality::HiRes => "Hi-Res (Master Quality)",
        }
    }

    /// Convert to tidlers AudioQuality
    pub fn to_tidlers(self) -> tidlers::client::models::playback::AudioQuality {
        match self {
            AudioQuality::Low => tidlers::client::models::playback::AudioQuality::Low,
            AudioQuality::High => tidlers::client::models::playback::AudioQuality::High,
            AudioQuality::Lossless => tidlers::client::models::playback::AudioQuality::Lossless,
            AudioQuality::HiRes => tidlers::client::models::playback::AudioQuality::HiRes,
        }
    }
}

impl AsRef<str> for AudioQuality {
    fn as_ref(&self) -> &str {
        self.display_name()
    }
}

/// Configuration for Maré Player
#[derive(Debug, Clone, CosmicConfigEntry, PartialEq)]
#[version = 1]
pub struct Config {
    /// Preferred audio quality for playback
    pub audio_quality: AudioQuality,
    /// Maximum image cache size in megabytes
    pub image_cache_max_mb: u32,
    /// Maximum audio/song cache size in megabytes (cached songs for offline-like replay)
    pub audio_cache_max_mb: u32,
    /// Volume level (0.0 to 1.0), persisted across restarts
    pub volume_level: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            audio_quality: AudioQuality::HiRes,
            image_cache_max_mb: 200,
            audio_cache_max_mb: 2000,
            volume_level: 1.0,
        }
    }
}
