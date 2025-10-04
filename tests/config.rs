// SPDX-License-Identifier: MIT

//! Integration tests for the config module.
//!
//! Tests AudioQuality enum (display_name, to_tidlers, AsRef<str>, Default)
//! and Config struct (Default values, field ranges).

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::config::{AudioQuality, Config};

// ===========================================================================
// AudioQuality — Default
// ===========================================================================

mod audio_quality_default {
    use super::*;

    #[test]
    fn default_is_hires() {
        let q = AudioQuality::default();
        assert_eq!(q, AudioQuality::HiRes);
    }
}

// ===========================================================================
// AudioQuality — display_name
// ===========================================================================

mod audio_quality_display_name {
    use super::*;

    #[test]
    fn low_display_name() {
        assert_eq!(AudioQuality::Low.display_name(), "Low (96 kbps)");
    }

    #[test]
    fn high_display_name() {
        assert_eq!(AudioQuality::High.display_name(), "High (320 kbps)");
    }

    #[test]
    fn lossless_display_name() {
        assert_eq!(
            AudioQuality::Lossless.display_name(),
            "Lossless (CD Quality)"
        );
    }

    #[test]
    fn hires_display_name() {
        assert_eq!(
            AudioQuality::HiRes.display_name(),
            "Hi-Res (Master Quality)"
        );
    }

    #[test]
    fn display_names_are_all_distinct() {
        let names: Vec<&str> = [
            AudioQuality::Low,
            AudioQuality::High,
            AudioQuality::Lossless,
            AudioQuality::HiRes,
        ]
        .iter()
        .map(|q| q.display_name())
        .collect();

        for (i, a) in names.iter().enumerate() {
            for (j, b) in names.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "display_name collision between variants {i} and {j}");
                }
            }
        }
    }
}

// ===========================================================================
// AudioQuality — AsRef<str>
// ===========================================================================

mod audio_quality_as_ref {
    use super::*;

    #[test]
    fn as_ref_matches_display_name_for_low() {
        let q = AudioQuality::Low;
        let as_ref: &str = q.as_ref();
        assert_eq!(as_ref, q.display_name());
    }

    #[test]
    fn as_ref_matches_display_name_for_high() {
        let q = AudioQuality::High;
        let as_ref: &str = q.as_ref();
        assert_eq!(as_ref, q.display_name());
    }

    #[test]
    fn as_ref_matches_display_name_for_lossless() {
        let q = AudioQuality::Lossless;
        let as_ref: &str = q.as_ref();
        assert_eq!(as_ref, q.display_name());
    }

    #[test]
    fn as_ref_matches_display_name_for_hires() {
        let q = AudioQuality::HiRes;
        let as_ref: &str = q.as_ref();
        assert_eq!(as_ref, q.display_name());
    }

    #[test]
    fn as_ref_is_nonempty_for_all_variants() {
        for q in &[
            AudioQuality::Low,
            AudioQuality::High,
            AudioQuality::Lossless,
            AudioQuality::HiRes,
        ] {
            let s: &str = q.as_ref();
            assert!(!s.is_empty(), "as_ref() should not be empty for {:?}", q);
        }
    }
}

// ===========================================================================
// AudioQuality — to_tidlers
// ===========================================================================

mod audio_quality_to_tidlers {
    use super::*;
    use tidlers::client::models::playback::AudioQuality as TidlersQuality;

    #[test]
    fn low_converts_to_tidlers_low() {
        let result = AudioQuality::Low.to_tidlers();
        assert!(matches!(result, TidlersQuality::Low));
    }

    #[test]
    fn high_converts_to_tidlers_high() {
        let result = AudioQuality::High.to_tidlers();
        assert!(matches!(result, TidlersQuality::High));
    }

    #[test]
    fn lossless_converts_to_tidlers_lossless() {
        let result = AudioQuality::Lossless.to_tidlers();
        assert!(matches!(result, TidlersQuality::Lossless));
    }

    #[test]
    fn hires_converts_to_tidlers_hires() {
        let result = AudioQuality::HiRes.to_tidlers();
        assert!(matches!(result, TidlersQuality::HiRes));
    }

    #[test]
    fn roundtrip_all_variants() {
        // Ensure every local variant maps to a distinct tidlers variant
        let variants = [
            AudioQuality::Low,
            AudioQuality::High,
            AudioQuality::Lossless,
            AudioQuality::HiRes,
        ];
        let tidlers_debug: Vec<String> = variants
            .iter()
            .map(|q| format!("{:?}", q.to_tidlers()))
            .collect();

        // All should be distinct
        for (i, a) in tidlers_debug.iter().enumerate() {
            for (j, b) in tidlers_debug.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "to_tidlers collision between variants {i} and {j}");
                }
            }
        }
    }
}

// ===========================================================================
// AudioQuality — Clone, Copy, PartialEq, Eq
// ===========================================================================

mod audio_quality_traits {
    use super::*;

    #[test]
    fn clone_equals_original() {
        let q = AudioQuality::Lossless;
        let cloned = q.clone();
        assert_eq!(q, cloned);
    }

    #[test]
    fn copy_equals_original() {
        let q = AudioQuality::HiRes;
        let copied = q;
        assert_eq!(q, copied);
    }

    #[test]
    fn different_variants_are_not_equal() {
        assert_ne!(AudioQuality::Low, AudioQuality::High);
        assert_ne!(AudioQuality::High, AudioQuality::Lossless);
        assert_ne!(AudioQuality::Lossless, AudioQuality::HiRes);
        assert_ne!(AudioQuality::HiRes, AudioQuality::Low);
    }

    #[test]
    fn same_variant_is_equal() {
        assert_eq!(AudioQuality::Low, AudioQuality::Low);
        assert_eq!(AudioQuality::High, AudioQuality::High);
        assert_eq!(AudioQuality::Lossless, AudioQuality::Lossless);
        assert_eq!(AudioQuality::HiRes, AudioQuality::HiRes);
    }

    #[test]
    fn debug_output_is_nonempty() {
        for q in &[
            AudioQuality::Low,
            AudioQuality::High,
            AudioQuality::Lossless,
            AudioQuality::HiRes,
        ] {
            let dbg = format!("{:?}", q);
            assert!(!dbg.is_empty());
        }
    }
}

// ===========================================================================
// AudioQuality — Serialize / Deserialize
// ===========================================================================

mod audio_quality_serde {
    use super::*;

    #[test]
    fn serialize_low() {
        let json = serde_json::to_string(&AudioQuality::Low).unwrap();
        assert!(json.contains("Low"), "expected 'Low' in {json}");
    }

    #[test]
    fn serialize_high() {
        let json = serde_json::to_string(&AudioQuality::High).unwrap();
        assert!(json.contains("High"), "expected 'High' in {json}");
    }

    #[test]
    fn serialize_lossless() {
        let json = serde_json::to_string(&AudioQuality::Lossless).unwrap();
        assert!(json.contains("Lossless"), "expected 'Lossless' in {json}");
    }

    #[test]
    fn serialize_hires() {
        let json = serde_json::to_string(&AudioQuality::HiRes).unwrap();
        assert!(json.contains("HiRes"), "expected 'HiRes' in {json}");
    }

    #[test]
    fn deserialize_roundtrip_all_variants() {
        for q in &[
            AudioQuality::Low,
            AudioQuality::High,
            AudioQuality::Lossless,
            AudioQuality::HiRes,
        ] {
            let json = serde_json::to_string(q).unwrap();
            let deserialized: AudioQuality = serde_json::from_str(&json).unwrap();
            assert_eq!(*q, deserialized, "roundtrip failed for {:?}", q);
        }
    }

    #[test]
    fn deserialize_invalid_value_fails() {
        let result: Result<AudioQuality, _> = serde_json::from_str("\"SuperHD\"");
        assert!(result.is_err());
    }
}

// ===========================================================================
// Config — Default
// ===========================================================================

mod config_default {
    use super::*;

    #[test]
    fn default_audio_quality_is_hires() {
        let cfg = Config::default();
        assert_eq!(cfg.audio_quality, AudioQuality::HiRes);
    }

    #[test]
    fn default_image_cache_max_mb_is_200() {
        let cfg = Config::default();
        assert_eq!(cfg.image_cache_max_mb, 200);
    }

    #[test]
    fn default_audio_cache_max_mb_is_2000() {
        let cfg = Config::default();
        assert_eq!(cfg.audio_cache_max_mb, 2000);
    }

    #[test]
    fn default_volume_level_is_1() {
        let cfg = Config::default();
        assert!((cfg.volume_level - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn default_volume_level_is_in_valid_range() {
        let cfg = Config::default();
        assert!(
            (0.0..=1.0).contains(&cfg.volume_level),
            "volume_level should be between 0.0 and 1.0, got {}",
            cfg.volume_level
        );
    }

    #[test]
    fn default_cache_sizes_are_positive() {
        let cfg = Config::default();
        assert!(cfg.image_cache_max_mb > 0);
        assert!(cfg.audio_cache_max_mb > 0);
    }
}

// ===========================================================================
// Config — Clone, PartialEq
// ===========================================================================

mod config_traits {
    use super::*;

    #[test]
    fn clone_equals_original() {
        let cfg = Config::default();
        let cloned = cfg.clone();
        assert_eq!(cfg, cloned);
    }

    #[test]
    fn modified_clone_differs() {
        let cfg = Config::default();
        let mut modified = cfg.clone();
        modified.audio_quality = AudioQuality::Low;
        assert_ne!(cfg, modified);
    }

    #[test]
    fn debug_output_is_nonempty() {
        let cfg = Config::default();
        let dbg = format!("{:?}", cfg);
        assert!(!dbg.is_empty());
        // Should contain field names
        assert!(dbg.contains("audio_quality"));
        assert!(dbg.contains("volume_level"));
    }
}

// ===========================================================================
// Config — field mutations
// ===========================================================================

mod config_fields {
    use super::*;

    #[test]
    fn audio_quality_can_be_changed() {
        let mut cfg = Config::default();
        cfg.audio_quality = AudioQuality::Low;
        assert_eq!(cfg.audio_quality, AudioQuality::Low);
    }

    #[test]
    fn volume_level_can_be_set_to_zero() {
        let mut cfg = Config::default();
        cfg.volume_level = 0.0;
        assert!((cfg.volume_level).abs() < f32::EPSILON);
    }

    #[test]
    fn image_cache_max_mb_can_be_set_to_zero() {
        let mut cfg = Config::default();
        cfg.image_cache_max_mb = 0;
        assert_eq!(cfg.image_cache_max_mb, 0);
    }

    #[test]
    fn audio_cache_max_mb_can_be_set_to_large_value() {
        let mut cfg = Config::default();
        cfg.audio_cache_max_mb = 10000;
        assert_eq!(cfg.audio_cache_max_mb, 10000);
    }

    #[test]
    fn all_quality_variants_can_be_assigned() {
        let mut cfg = Config::default();
        for q in [
            AudioQuality::Low,
            AudioQuality::High,
            AudioQuality::Lossless,
            AudioQuality::HiRes,
        ] {
            cfg.audio_quality = q;
            assert_eq!(cfg.audio_quality, q);
        }
    }
}
