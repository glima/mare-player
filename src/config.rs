// SPDX-License-Identifier: GPL-3.0-only

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
    /// Label for the Settings dropdown: TIDAL's tier name plus what it actually
    /// delivers.
    ///
    /// Deliberately the same vocabulary the now-playing badge uses for the
    /// *served* stream, so "what I asked for" and "what I'm getting" can be
    /// compared directly, and phrased like TIDAL's own client ("16-bit,
    /// 44.1 kHz" / "Up to 24-bit, 192 kHz").
    ///
    /// Not localized, for the same reason the log-level dropdown isn't: the
    /// content is TIDAL's tier names and unit-bearing specs. The badge renders
    /// the tier straight from the API response, so translating this side alone
    /// would put the two halves of that comparison in different languages. The
    /// prose *underneath* the dropdown (`quality-description-*`) is localized —
    /// that's where anything explanatory belongs.
    pub fn display_name(&self) -> &'static str {
        match self {
            AudioQuality::Low => "Low — 96 kbps AAC",
            AudioQuality::High => "High — 320 kbps AAC",
            AudioQuality::Lossless => "Lossless — 16-bit, 44.1 kHz",
            AudioQuality::HiRes => "Hi-Res Lossless — up to 24-bit, 192 kHz",
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

    /// The string TIDAL's `audioquality` request parameter expects.
    ///
    /// Deliberately not derived from [`Self::to_tidlers`]: tidlers renders its
    /// `HiRes` as `HI_RES`, the MQA-era tier TIDAL retired, and asking for a
    /// tier that no longer exists earns a silent downgrade to `LOSSLESS`. The
    /// FLAC hi-res tier — "Max" in TIDAL's own UI, up to 24-bit / 192 kHz — is
    /// `HI_RES_LOSSLESS`.
    pub fn tidal_param(self) -> &'static str {
        match self {
            AudioQuality::Low => "LOW",
            AudioQuality::High => "HIGH",
            AudioQuality::Lossless => "LOSSLESS",
            AudioQuality::HiRes => "HI_RES_LOSSLESS",
        }
    }
}

impl AsRef<str> for AudioQuality {
    fn as_ref(&self) -> &str {
        self.display_name()
    }
}

/// Console/journal log verbosity.
///
/// Controls the base level of the terminal (journal) log layer only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    /// Only errors.
    Error,
    /// Warnings and errors.
    Warn,
    /// Informational messages and above (the default).
    #[default]
    Info,
    /// Debug messages and above.
    Debug,
    /// Everything, including trace-level spans.
    Trace,
}

impl LogLevel {
    /// The `EnvFilter` base directive string for this level.
    pub fn as_filter_str(&self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }

    /// Human-readable label for the settings dropdown.
    pub fn display_name(&self) -> &'static str {
        match self {
            LogLevel::Error => "Error",
            LogLevel::Warn => "Warning",
            LogLevel::Info => "Info (default)",
            LogLevel::Debug => "Debug",
            LogLevel::Trace => "Trace",
        }
    }
}

impl AsRef<str> for LogLevel {
    fn as_ref(&self) -> &str {
        self.display_name()
    }
}

/// Configuration for Maré Player
#[derive(Debug, Clone, CosmicConfigEntry, PartialEq)]
#[version = 2]
pub struct Config {
    /// Preferred audio quality for playback
    pub audio_quality: AudioQuality,
    /// Maximum image cache size in megabytes
    pub image_cache_max_mb: u32,
    /// Console/journal log verbosity
    pub log_level: LogLevel,
    /// Volume level (0.0 to 1.0), persisted across restarts
    pub volume_level: f32,
    /// Fixed loudness pre-amp applied to music **videos**, in decibels.
    ///
    /// TIDAL authors replay-gain for audio tracks but **not** for videos, so
    /// videos get a fixed pre-amp instead. TIDAL album gains cluster around
    /// -7..-11 dB, so the -8 dB default brings videos roughly in line with
    /// normalized tracks.
    pub video_preamp_db: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            audio_quality: AudioQuality::HiRes,
            image_cache_max_mb: 200,
            log_level: LogLevel::Info,
            volume_level: 1.0,
            video_preamp_db: -8.0,
        }
    }
}
