// SPDX-License-Identifier: MIT

//! Integration tests for the views/components module.
//!
//! Tests TrackRowOptions (defaults, duration_column_width computation) and
//! the public constants exported from the components module.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use std::sync::Arc;

use cosmic_applet_mare::tidal::models::Track;
use cosmic_applet_mare::views::components::{
    MAX_POPUP_HEIGHT, NOW_PLAYING_ART_SIZE, PANEL_ART_SIZE, THUMBNAIL_SIZE, TrackRowOptions,
};

// ===========================================================================
// Public Constants
// ===========================================================================

mod constants {
    use super::*;

    #[test]
    fn thumbnail_size_is_positive() {
        assert!(THUMBNAIL_SIZE > 0);
    }

    #[test]
    fn thumbnail_size_value() {
        assert_eq!(THUMBNAIL_SIZE, 40);
    }

    #[test]
    fn now_playing_art_size_is_positive() {
        assert!(NOW_PLAYING_ART_SIZE > 0);
    }

    #[test]
    fn now_playing_art_size_value() {
        assert_eq!(NOW_PLAYING_ART_SIZE, 56);
    }

    #[test]
    fn panel_art_size_is_positive() {
        assert!(PANEL_ART_SIZE > 0);
    }

    #[test]
    fn panel_art_size_value() {
        assert_eq!(PANEL_ART_SIZE, 20);
    }

    #[test]
    fn max_popup_height_is_positive() {
        assert!(MAX_POPUP_HEIGHT > 0.0);
    }

    #[test]
    fn max_popup_height_value() {
        assert!((MAX_POPUP_HEIGHT - 500.0).abs() < f32::EPSILON);
    }

    #[test]
    fn thumbnail_smaller_than_now_playing_art() {
        assert!(
            THUMBNAIL_SIZE < NOW_PLAYING_ART_SIZE,
            "list thumbnails should be smaller than now-playing art"
        );
    }

    #[test]
    fn panel_art_smallest() {
        assert!(
            PANEL_ART_SIZE < THUMBNAIL_SIZE,
            "panel art should be the smallest size"
        );
    }

    #[test]
    fn size_ordering_panel_lt_thumb_lt_now_playing() {
        assert!(PANEL_ART_SIZE < THUMBNAIL_SIZE);
        assert!(THUMBNAIL_SIZE < NOW_PLAYING_ART_SIZE);
    }
}

// ===========================================================================
// TrackRowOptions — Default
// ===========================================================================

mod track_row_options_default {
    use super::*;

    #[test]
    fn default_tracks_is_empty() {
        let opts = TrackRowOptions::default();
        assert!(opts.tracks.is_empty());
    }

    #[test]
    fn default_context_is_none() {
        let opts = TrackRowOptions::default();
        assert!(opts.context.is_none());
    }

    #[test]
    fn default_fallback_icon_is_audio_generic() {
        let opts = TrackRowOptions::default();
        assert_eq!(opts.fallback_icon, "audio-x-generic-symbolic");
    }

    #[test]
    fn default_show_radio_button_is_true() {
        let opts = TrackRowOptions::default();
        assert!(opts.show_radio_button);
    }

    #[test]
    fn default_fallback_icon_is_nonempty() {
        let opts = TrackRowOptions::default();
        assert!(!opts.fallback_icon.is_empty());
    }

    #[test]
    fn default_fallback_icon_ends_with_symbolic() {
        let opts = TrackRowOptions::default();
        assert!(
            opts.fallback_icon.ends_with("-symbolic"),
            "COSMIC icon names should end with -symbolic, got: {}",
            opts.fallback_icon
        );
    }
}

// ===========================================================================
// TrackRowOptions — field assignment
// ===========================================================================

mod track_row_options_fields {
    use super::*;

    #[test]
    fn can_set_context() {
        let tracks: Vec<Track> = vec![];
        let opts = TrackRowOptions {
            context: Some("My Playlist".to_string()),
            tracks: tracks.into(),
            ..Default::default()
        };
        assert_eq!(opts.context, Some("My Playlist".to_string()));
    }

    #[test]
    fn can_disable_radio_button() {
        let tracks: Vec<Track> = vec![];
        let opts = TrackRowOptions {
            show_radio_button: false,
            tracks: tracks.into(),
            ..Default::default()
        };
        assert!(!opts.show_radio_button);
    }

    #[test]
    fn can_set_custom_fallback_icon() {
        let tracks: Vec<Track> = vec![];
        let opts = TrackRowOptions {
            fallback_icon: "media-optical-symbolic",
            tracks: tracks.into(),
            ..Default::default()
        };
        assert_eq!(opts.fallback_icon, "media-optical-symbolic");
    }

    #[test]
    fn can_reference_external_tracks_slice() {
        let tracks = vec![
            Track {
                id: "1".to_string(),
                title: "Track One".to_string(),
                duration: 200,
                ..Default::default()
            },
            Track {
                id: "2".to_string(),
                title: "Track Two".to_string(),
                duration: 300,
                ..Default::default()
            },
        ];
        let tracks: Arc<[Track]> = tracks.into();
        let opts = TrackRowOptions {
            tracks: Arc::clone(&tracks),
            ..Default::default()
        };
        assert_eq!(opts.tracks.len(), 2);
        assert_eq!(opts.tracks[0].id, "1");
        assert_eq!(opts.tracks[1].id, "2");
    }
}

// ===========================================================================
// TrackRowOptions — duration_column_width
// ===========================================================================

mod duration_column_width {
    use super::*;

    #[test]
    fn empty_tracks_returns_fallback_width() {
        let opts = TrackRowOptions::default();
        let width = opts.duration_column_width();
        // Fallback is "0:00" → 1 digit + 2 digits + 1 colon = 3 digits * 6 + 1 colon * 3 + 1 = 22.0
        let expected = 3.0 * 6.0 + 1.0 * 3.0 + 1.0;
        assert!(
            (width - expected).abs() < f32::EPSILON,
            "expected {expected} for empty tracks, got {width}"
        );
    }

    #[test]
    fn single_short_track() {
        let tracks = vec![Track {
            duration: 65, // 1:05
            ..Default::default()
        }];
        let opts = TrackRowOptions {
            tracks: tracks.into(),
            ..Default::default()
        };
        let width = opts.duration_column_width();
        // "1:05" → 3 digits, 1 colon → 3*6 + 1*3 + 1 = 22.0
        let expected = 3.0 * 6.0 + 1.0 * 3.0 + 1.0;
        assert!(
            (width - expected).abs() < f32::EPSILON,
            "expected {expected} for 1:05, got {width}"
        );
    }

    #[test]
    fn track_with_longer_duration_produces_wider_column() {
        let short_tracks = vec![Track {
            duration: 30, // 0:30
            ..Default::default()
        }];
        let long_tracks = vec![Track {
            duration: 3661, // 61:01
            ..Default::default()
        }];
        let short_opts = TrackRowOptions {
            tracks: short_tracks.into(),
            ..Default::default()
        };
        let long_opts = TrackRowOptions {
            tracks: long_tracks.into(),
            ..Default::default()
        };
        let short_width = short_opts.duration_column_width();
        let long_width = long_opts.duration_column_width();
        assert!(
            long_width >= short_width,
            "longer duration should produce wider column: short={short_width}, long={long_width}"
        );
    }

    #[test]
    fn width_uses_longest_duration_from_list() {
        let tracks = vec![
            Track {
                duration: 30, // 0:30
                ..Default::default()
            },
            Track {
                duration: 600, // 10:00
                ..Default::default()
            },
            Track {
                duration: 120, // 2:00
                ..Default::default()
            },
        ];
        let opts = TrackRowOptions {
            tracks: tracks.into(),
            ..Default::default()
        };
        let width = opts.duration_column_width();
        // "10:00" is the longest string (5 chars: 4 digits + 1 colon)
        // 4 digits * 6 + 1 colon * 3 + 1 = 28.0
        let expected = 4.0 * 6.0 + 1.0 * 3.0 + 1.0;
        assert!(
            (width - expected).abs() < f32::EPSILON,
            "expected {expected} for max duration 10:00, got {width}"
        );
    }

    #[test]
    fn very_long_track_duration() {
        let tracks = vec![Track {
            duration: 36000, // 600:00 (10 hours)
            ..Default::default()
        }];
        let opts = TrackRowOptions {
            tracks: tracks.into(),
            ..Default::default()
        };
        let width = opts.duration_column_width();
        // "600:00" → 5 digits, 1 colon → 5*6 + 1*3 + 1 = 34.0
        let expected = 5.0 * 6.0 + 1.0 * 3.0 + 1.0;
        assert!(
            (width - expected).abs() < f32::EPSILON,
            "expected {expected} for 600:00, got {width}"
        );
    }

    #[test]
    fn zero_duration_track() {
        let tracks = vec![Track {
            duration: 0, // 0:00
            ..Default::default()
        }];
        let opts = TrackRowOptions {
            tracks: tracks.into(),
            ..Default::default()
        };
        let width = opts.duration_column_width();
        // "0:00" → 3 digits, 1 colon → 3*6 + 1*3 + 1 = 22.0
        let expected = 3.0 * 6.0 + 1.0 * 3.0 + 1.0;
        assert!(
            (width - expected).abs() < f32::EPSILON,
            "expected {expected} for 0:00, got {width}"
        );
    }

    #[test]
    fn one_second_track() {
        let tracks = vec![Track {
            duration: 1, // 0:01
            ..Default::default()
        }];
        let opts = TrackRowOptions {
            tracks: tracks.into(),
            ..Default::default()
        };
        let width = opts.duration_column_width();
        // "0:01" → 3 digits, 1 colon → 3*6 + 1*3 + 1 = 22.0
        let expected = 3.0 * 6.0 + 1.0 * 3.0 + 1.0;
        assert!(
            (width - expected).abs() < f32::EPSILON,
            "expected {expected} for 0:01, got {width}"
        );
    }

    #[test]
    fn exactly_one_minute() {
        let tracks = vec![Track {
            duration: 60, // 1:00
            ..Default::default()
        }];
        let opts = TrackRowOptions {
            tracks: tracks.into(),
            ..Default::default()
        };
        let width = opts.duration_column_width();
        // "1:00" → 3 digits, 1 colon
        let expected = 3.0 * 6.0 + 1.0 * 3.0 + 1.0;
        assert!(
            (width - expected).abs() < f32::EPSILON,
            "expected {expected} for 1:00, got {width}"
        );
    }

    #[test]
    fn width_is_always_positive() {
        let tracks = vec![Track {
            duration: 0,
            ..Default::default()
        }];
        let opts = TrackRowOptions {
            tracks: tracks.into(),
            ..Default::default()
        };
        assert!(
            opts.duration_column_width() > 0.0,
            "duration column width should always be positive"
        );
    }

    #[test]
    fn width_is_positive_even_for_empty_tracks() {
        let opts = TrackRowOptions::default();
        assert!(
            opts.duration_column_width() > 0.0,
            "duration column width should be positive even with no tracks"
        );
    }

    #[test]
    fn deterministic_for_same_input() {
        let tracks = vec![
            Track {
                duration: 180,
                ..Default::default()
            },
            Track {
                duration: 240,
                ..Default::default()
            },
        ];
        let opts = TrackRowOptions {
            tracks: tracks.into(),
            ..Default::default()
        };
        let w1 = opts.duration_column_width();
        let w2 = opts.duration_column_width();
        assert!(
            (w1 - w2).abs() < f32::EPSILON,
            "should be deterministic: {w1} vs {w2}"
        );
    }

    #[test]
    fn all_same_duration_uses_that_duration() {
        let tracks = vec![
            Track {
                duration: 90,
                ..Default::default()
            },
            Track {
                duration: 90,
                ..Default::default()
            },
            Track {
                duration: 90,
                ..Default::default()
            },
        ];
        let opts = TrackRowOptions {
            tracks: tracks.into(),
            ..Default::default()
        };
        let width = opts.duration_column_width();
        // "1:30" → 3 digits, 1 colon → 22.0
        let expected = 3.0 * 6.0 + 1.0 * 3.0 + 1.0;
        assert!(
            (width - expected).abs() < f32::EPSILON,
            "expected {expected} for uniform 1:30, got {width}"
        );
    }

    #[test]
    fn hour_long_track_width() {
        let tracks = vec![Track {
            duration: 3600, // 60:00
            ..Default::default()
        }];
        let opts = TrackRowOptions {
            tracks: tracks.into(),
            ..Default::default()
        };
        let width = opts.duration_column_width();
        // "60:00" → 4 digits, 1 colon → 4*6 + 1*3 + 1 = 28.0
        let expected = 4.0 * 6.0 + 1.0 * 3.0 + 1.0;
        assert!(
            (width - expected).abs() < f32::EPSILON,
            "expected {expected} for 60:00, got {width}"
        );
    }
}

// ===========================================================================
// RADIO_SVG constant
// ===========================================================================

mod radio_svg {
    use cosmic_applet_mare::views::components::RADIO_SVG;

    #[test]
    fn radio_svg_is_nonempty() {
        assert!(!RADIO_SVG.is_empty());
    }

    #[test]
    fn radio_svg_starts_with_svg_tag() {
        let s = std::str::from_utf8(RADIO_SVG).expect("RADIO_SVG should be valid UTF-8");
        assert!(
            s.starts_with("<svg"),
            "RADIO_SVG should start with <svg, got: {}",
            &s[..s.len().min(20)]
        );
    }

    #[test]
    fn radio_svg_ends_with_closing_tag() {
        let s = std::str::from_utf8(RADIO_SVG).expect("RADIO_SVG should be valid UTF-8");
        assert!(
            s.trim_end().ends_with("</svg>"),
            "RADIO_SVG should end with </svg>"
        );
    }

    #[test]
    fn radio_svg_is_16x16() {
        let s = std::str::from_utf8(RADIO_SVG).expect("RADIO_SVG should be valid UTF-8");
        assert!(
            s.contains("width=\"16\"") && s.contains("height=\"16\""),
            "RADIO_SVG should be 16x16"
        );
    }

    #[test]
    fn radio_svg_is_valid_utf8() {
        assert!(
            std::str::from_utf8(RADIO_SVG).is_ok(),
            "RADIO_SVG should be valid UTF-8"
        );
    }

    #[test]
    fn radio_svg_contains_path_elements() {
        let s = std::str::from_utf8(RADIO_SVG).unwrap();
        // A radio icon should have some SVG drawing primitives
        let has_drawing = s.contains("<path")
            || s.contains("<circle")
            || s.contains("<rect")
            || s.contains("<line");
        assert!(has_drawing, "RADIO_SVG should contain SVG drawing elements");
    }
}

// ===========================================================================
// favorite_icon_handle
// ===========================================================================

mod favorite_icon {
    use cosmic_applet_mare::views::components::favorite_icon_handle;

    #[test]
    fn favorite_true_returns_handle() {
        // Should not panic
        let _handle = favorite_icon_handle(true);
    }

    #[test]
    fn favorite_false_returns_handle() {
        // Should not panic
        let _handle = favorite_icon_handle(false);
    }

    #[test]
    fn both_variants_callable_back_to_back() {
        let _h1 = favorite_icon_handle(true);
        let _h2 = favorite_icon_handle(false);
        let _h3 = favorite_icon_handle(true);
    }
}
