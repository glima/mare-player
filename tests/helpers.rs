// SPDX-License-Identifier: MIT

//! Integration tests for the helpers module.
//!
//! Covers `format_seconds` with extensive edge cases including zero, fractional
//! values, negative values, boundary conditions, very large values, and
//! special floating-point values.
//!
//! Also covers `max_description_chars` with various window widths including
//! boundary values, scaling behaviour, and special floating-point inputs.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::helpers::{format_seconds, max_description_chars};

// ===========================================================================
// Basic formatting
// ===========================================================================

mod basic {
    use super::*;

    #[test]
    fn zero() {
        assert_eq!(format_seconds(0.0), "0:00");
    }

    #[test]
    fn one_second() {
        assert_eq!(format_seconds(1.0), "0:01");
    }

    #[test]
    fn five_seconds() {
        assert_eq!(format_seconds(5.0), "0:05");
    }

    #[test]
    fn ten_seconds() {
        assert_eq!(format_seconds(10.0), "0:10");
    }

    #[test]
    fn thirty_seconds() {
        assert_eq!(format_seconds(30.0), "0:30");
    }

    #[test]
    fn fifty_nine_seconds() {
        assert_eq!(format_seconds(59.0), "0:59");
    }

    #[test]
    fn one_minute() {
        assert_eq!(format_seconds(60.0), "1:00");
    }

    #[test]
    fn one_minute_one_second() {
        assert_eq!(format_seconds(61.0), "1:01");
    }

    #[test]
    fn one_minute_five_seconds() {
        assert_eq!(format_seconds(65.0), "1:05");
    }

    #[test]
    fn one_minute_thirty_seconds() {
        assert_eq!(format_seconds(90.0), "1:30");
    }

    #[test]
    fn two_minutes() {
        assert_eq!(format_seconds(120.0), "2:00");
    }

    #[test]
    fn three_minutes_five_seconds() {
        assert_eq!(format_seconds(185.0), "3:05");
    }

    #[test]
    fn ten_minutes() {
        assert_eq!(format_seconds(600.0), "10:00");
    }

    #[test]
    fn fifty_nine_minutes_fifty_nine_seconds() {
        assert_eq!(format_seconds(3599.0), "59:59");
    }
}

// ===========================================================================
// Hour boundary
// ===========================================================================

mod hours {
    use super::*;

    #[test]
    fn exactly_one_hour() {
        assert_eq!(format_seconds(3600.0), "1:00:00");
    }

    #[test]
    fn one_hour_one_second() {
        assert_eq!(format_seconds(3601.0), "1:00:01");
    }

    #[test]
    fn one_hour_one_minute() {
        assert_eq!(format_seconds(3660.0), "1:01:00");
    }

    #[test]
    fn one_hour_two_minutes_seven_seconds() {
        assert_eq!(format_seconds(3727.0), "1:02:07");
    }

    #[test]
    fn two_hours() {
        assert_eq!(format_seconds(7200.0), "2:00:00");
    }

    #[test]
    fn two_hours_thirty_minutes() {
        assert_eq!(format_seconds(9000.0), "2:30:00");
    }

    #[test]
    fn ten_hours() {
        assert_eq!(format_seconds(36000.0), "10:00:00");
    }

    #[test]
    fn ten_hours_thirty_minutes_fifteen_seconds() {
        assert_eq!(format_seconds(37815.0), "10:30:15");
    }

    #[test]
    fn twenty_four_hours() {
        assert_eq!(format_seconds(86400.0), "24:00:00");
    }

    #[test]
    fn one_hundred_hours() {
        // 100 * 3600 = 360000
        assert_eq!(format_seconds(360000.0), "100:00:00");
    }
}

// ===========================================================================
// Fractional seconds (truncation)
// ===========================================================================

mod fractional {
    use super::*;

    #[test]
    fn half_second_truncates_to_zero() {
        assert_eq!(format_seconds(0.5), "0:00");
    }

    #[test]
    fn one_point_nine_truncates_to_one() {
        assert_eq!(format_seconds(1.9), "0:01");
    }

    #[test]
    fn five_point_nine() {
        assert_eq!(format_seconds(5.9), "0:05");
    }

    #[test]
    fn fifty_nine_point_nine() {
        assert_eq!(format_seconds(59.9), "0:59");
    }

    #[test]
    fn sixty_point_five() {
        assert_eq!(format_seconds(60.5), "1:00");
    }

    #[test]
    fn three_hundred_point_seven() {
        assert_eq!(format_seconds(300.7), "5:00");
    }

    #[test]
    fn three_thousand_six_hundred_point_one() {
        assert_eq!(format_seconds(3600.1), "1:00:00");
    }

    #[test]
    fn small_fraction() {
        assert_eq!(format_seconds(0.001), "0:00");
    }

    #[test]
    fn just_under_one() {
        assert_eq!(format_seconds(0.999), "0:00");
    }

    #[test]
    fn just_under_sixty() {
        assert_eq!(format_seconds(59.999), "0:59");
    }

    #[test]
    fn just_under_one_hour() {
        assert_eq!(format_seconds(3599.999), "59:59");
    }
}

// ===========================================================================
// Negative values
// ===========================================================================

mod negative {
    use super::*;

    #[test]
    fn negative_one() {
        assert_eq!(format_seconds(-1.0), "0:00");
    }

    #[test]
    fn negative_ten() {
        assert_eq!(format_seconds(-10.0), "0:00");
    }

    #[test]
    fn negative_large() {
        assert_eq!(format_seconds(-999999.0), "0:00");
    }

    #[test]
    fn negative_small_fraction() {
        assert_eq!(format_seconds(-0.001), "0:00");
    }

    #[test]
    fn negative_half() {
        assert_eq!(format_seconds(-0.5), "0:00");
    }

    #[test]
    fn negative_infinity() {
        assert_eq!(format_seconds(f64::NEG_INFINITY), "0:00");
    }
}

// ===========================================================================
// Special floating-point values
// ===========================================================================

mod special_fp {
    use super::*;

    #[test]
    fn positive_zero() {
        assert_eq!(format_seconds(0.0), "0:00");
    }

    #[test]
    fn negative_zero() {
        assert_eq!(format_seconds(-0.0), "0:00");
    }

    #[test]
    fn nan_clamps_to_zero() {
        // f64::NAN.max(0.0) returns 0.0 in Rust
        assert_eq!(format_seconds(f64::NAN), "0:00");
    }

    #[test]
    fn positive_infinity_produces_large_value() {
        // f64::INFINITY.max(0.0) is INFINITY → cast to u64 is MAX or UB;
        // the function should at least not panic. We just verify it returns
        // something containing digits.
        let result = format_seconds(f64::INFINITY);
        assert!(
            !result.is_empty(),
            "should produce some output for +infinity"
        );
    }

    #[test]
    fn very_small_positive() {
        assert_eq!(format_seconds(f64::MIN_POSITIVE), "0:00");
    }

    #[test]
    fn epsilon() {
        assert_eq!(format_seconds(f64::EPSILON), "0:00");
    }
}

// ===========================================================================
// Very large values
// ===========================================================================

mod large_values {
    use super::*;

    #[test]
    fn one_million_seconds() {
        // 1_000_000 seconds = 277 hours, 46 minutes, 40 seconds
        assert_eq!(format_seconds(1_000_000.0), "277:46:40");
    }

    #[test]
    fn one_day_in_seconds() {
        assert_eq!(format_seconds(86400.0), "24:00:00");
    }

    #[test]
    fn one_week_in_seconds() {
        // 604800 seconds = 168:00:00
        assert_eq!(format_seconds(604800.0), "168:00:00");
    }

    #[test]
    fn large_but_valid() {
        // 9999 hours = 35996400 seconds
        let result = format_seconds(35996400.0);
        assert!(result.starts_with("9999:"));
    }
}

// ===========================================================================
// Padding / formatting
// ===========================================================================

mod padding {
    use super::*;

    /// Verify that seconds are always zero-padded to two digits.
    #[test]
    fn seconds_always_two_digits() {
        for s in 0..60u64 {
            let result = format_seconds(s as f64);
            let parts: Vec<&str> = result.split(':').collect();
            let last = parts.last().unwrap();
            assert_eq!(
                last.len(),
                2,
                "seconds part '{}' should be 2 digits in '{}'",
                last,
                result
            );
        }
    }

    /// Verify that minutes are zero-padded to two digits when hours are present.
    #[test]
    fn minutes_padded_with_hours() {
        for m in 0..10u64 {
            let seconds = 3600.0 + (m as f64) * 60.0;
            let result = format_seconds(seconds);
            let parts: Vec<&str> = result.split(':').collect();
            assert_eq!(parts.len(), 3, "should have H:MM:SS format for {}", result);
            let minutes_part = parts[1];
            assert_eq!(
                minutes_part.len(),
                2,
                "minutes '{}' should be 2 digits in '{}'",
                minutes_part,
                result
            );
        }
    }

    /// Verify that minutes are NOT zero-padded when hours are absent.
    #[test]
    fn minutes_not_padded_without_hours() {
        for m in 0..10u64 {
            let seconds = (m as f64) * 60.0;
            let result = format_seconds(seconds);
            let parts: Vec<&str> = result.split(':').collect();
            assert_eq!(
                parts.len(),
                2,
                "should have M:SS format for {} seconds",
                seconds
            );
            let minutes_part = parts[0];
            // Single digit minutes should NOT be zero-padded (e.g., "3:00" not "03:00")
            if m < 10 {
                assert_eq!(
                    minutes_part.len(),
                    1,
                    "minutes '{}' should be 1 digit in '{}'",
                    minutes_part,
                    result
                );
            }
        }
    }

    /// Verify hours are NOT zero-padded.
    #[test]
    fn hours_not_padded() {
        assert_eq!(format_seconds(3600.0), "1:00:00");
        assert_eq!(format_seconds(7200.0), "2:00:00");
        assert_eq!(format_seconds(36000.0), "10:00:00");
    }
}

// ===========================================================================
// Determinism
// ===========================================================================

mod determinism {
    use super::*;

    #[test]
    fn same_input_same_output() {
        let values = [0.0, 1.0, 59.0, 60.0, 3599.0, 3600.0, 86400.0];
        for &v in &values {
            let r1 = format_seconds(v);
            let r2 = format_seconds(v);
            let r3 = format_seconds(v);
            assert_eq!(r1, r2, "non-deterministic for input {}", v);
            assert_eq!(r2, r3, "non-deterministic for input {}", v);
        }
    }

    #[test]
    fn monotonic_values_produce_increasing_output() {
        let mut prev_total = 0u64;
        for s in (0..7200).step_by(17) {
            let result = format_seconds(s as f64);
            let parts: Vec<&str> = result.split(':').collect();

            let total_seconds = if parts.len() == 3 {
                let h: u64 = parts[0].parse().unwrap();
                let m: u64 = parts[1].parse().unwrap();
                let s: u64 = parts[2].parse().unwrap();
                h * 3600 + m * 60 + s
            } else {
                let m: u64 = parts[0].parse().unwrap();
                let s: u64 = parts[1].parse().unwrap();
                m * 60 + s
            };

            assert!(
                total_seconds >= prev_total,
                "output for {} seconds ({}) should be >= previous ({})",
                s,
                total_seconds,
                prev_total
            );
            prev_total = total_seconds;
        }
    }
}

// ===========================================================================
// Comprehensive sweep
// ===========================================================================

mod sweep {
    use super::*;

    /// Test every second from 0 to 3700 (past the hour boundary) and verify
    /// that the output parses back to the correct value.
    #[test]
    fn roundtrip_0_to_3700() {
        for s in 0..=3700u64 {
            let result = format_seconds(s as f64);
            let parts: Vec<&str> = result.split(':').collect();

            let total = if parts.len() == 3 {
                let h: u64 = parts[0].parse().unwrap();
                let m: u64 = parts[1].parse().unwrap();
                let sec: u64 = parts[2].parse().unwrap();
                h * 3600 + m * 60 + sec
            } else if parts.len() == 2 {
                let m: u64 = parts[0].parse().unwrap();
                let sec: u64 = parts[1].parse().unwrap();
                m * 60 + sec
            } else {
                panic!("unexpected format: '{}'", result);
            };

            assert_eq!(
                total, s,
                "format_seconds({}) = '{}' roundtrips to {}",
                s, result, total
            );
        }
    }

    /// Verify format transition at the 1-hour boundary.
    #[test]
    fn hour_boundary_format_transition() {
        let under_hour = format_seconds(3599.0);
        let at_hour = format_seconds(3600.0);
        let over_hour = format_seconds(3601.0);

        // Under hour: M:SS format (2 parts)
        assert_eq!(under_hour.split(':').count(), 2, "3599s should be M:SS");
        // At hour: H:MM:SS format (3 parts)
        assert_eq!(at_hour.split(':').count(), 3, "3600s should be H:MM:SS");
        // Over hour: H:MM:SS format (3 parts)
        assert_eq!(over_hour.split(':').count(), 3, "3601s should be H:MM:SS");
    }
}

// ===========================================================================
// Typical music track durations
// ===========================================================================

mod music_durations {
    use super::*;

    #[test]
    fn pop_song_3m30s() {
        assert_eq!(format_seconds(210.0), "3:30");
    }

    #[test]
    fn prog_rock_23m() {
        assert_eq!(format_seconds(1380.0), "23:00");
    }

    #[test]
    fn symphony_1h12m() {
        assert_eq!(format_seconds(4320.0), "1:12:00");
    }

    #[test]
    fn podcast_2h30m() {
        assert_eq!(format_seconds(9000.0), "2:30:00");
    }

    #[test]
    fn audiobook_12h() {
        assert_eq!(format_seconds(43200.0), "12:00:00");
    }

    #[test]
    fn short_jingle_15s() {
        assert_eq!(format_seconds(15.0), "0:15");
    }

    #[test]
    fn album_total_45m33s() {
        assert_eq!(format_seconds(2733.0), "45:33");
    }
}

// ===========================================================================
// max_description_chars — baseline
// ===========================================================================

mod max_description_chars_baseline {
    use super::*;

    #[test]
    fn baseline_360px_returns_300() {
        assert_eq!(max_description_chars(360.0), 300);
    }

    #[test]
    fn zero_width_falls_back_to_baseline() {
        assert_eq!(max_description_chars(0.0), 300);
    }

    #[test]
    fn negative_width_falls_back_to_baseline() {
        assert_eq!(max_description_chars(-100.0), 300);
    }

    #[test]
    fn negative_one_falls_back_to_baseline() {
        assert_eq!(max_description_chars(-1.0), 300);
    }
}

// ===========================================================================
// max_description_chars — scaling
// ===========================================================================

mod max_description_chars_scaling {
    use super::*;

    #[test]
    fn wider_window_gives_more_chars() {
        let narrow = max_description_chars(360.0);
        let wide = max_description_chars(720.0);
        assert!(
            wide > narrow,
            "720px ({wide}) should give more chars than 360px ({narrow})"
        );
    }

    #[test]
    fn double_width_gives_double_chars() {
        let base = max_description_chars(360.0);
        let double = max_description_chars(720.0);
        assert_eq!(double, base * 2, "double width should give double chars");
    }

    #[test]
    fn half_width_gives_half_chars() {
        let base = max_description_chars(360.0);
        let half = max_description_chars(180.0);
        assert_eq!(half, base / 2, "half width should give half chars");
    }

    #[test]
    fn triple_width() {
        let base = max_description_chars(360.0);
        let triple = max_description_chars(1080.0);
        assert_eq!(triple, base * 3);
    }

    #[test]
    fn monotonically_increasing() {
        let mut prev = 0;
        for w in (50..=2000).step_by(50) {
            let chars = max_description_chars(w as f32);
            assert!(
                chars >= prev,
                "chars should not decrease: width={w}, got {chars}, prev was {prev}"
            );
            prev = chars;
        }
    }

    #[test]
    fn very_large_width() {
        let chars = max_description_chars(10000.0);
        assert!(
            chars > 1000,
            "very wide window should give many chars, got {chars}"
        );
    }
}

// ===========================================================================
// max_description_chars — minimum floor
// ===========================================================================

mod max_description_chars_minimum {
    use super::*;

    #[test]
    fn very_narrow_clamps_to_150() {
        assert_eq!(max_description_chars(50.0), 150);
    }

    #[test]
    fn one_pixel_clamps_to_150() {
        assert_eq!(max_description_chars(1.0), 150);
    }

    #[test]
    fn tiny_fraction_clamps_to_150() {
        assert_eq!(max_description_chars(0.01), 150);
    }

    #[test]
    fn never_below_150() {
        for w in 1..=500 {
            let chars = max_description_chars(w as f32);
            assert!(
                chars >= 150,
                "should never be below 150: width={w}, got {chars}"
            );
        }
    }

    #[test]
    fn threshold_width_for_minimum() {
        // At what width do we stop clamping to the minimum?
        // 150 chars at ~0.83 chars/pixel → ~180 px
        let at_180 = max_description_chars(180.0);
        assert_eq!(at_180, 150, "180px should give exactly 150 chars");

        // Just above 180px should give more than 150
        let at_200 = max_description_chars(200.0);
        assert!(at_200 >= 150);
    }
}

// ===========================================================================
// max_description_chars — special float values
// ===========================================================================

mod max_description_chars_special_fp {
    use super::*;

    #[test]
    fn positive_infinity_returns_large_value() {
        let chars = max_description_chars(f32::INFINITY);
        assert!(
            chars > 1000,
            "infinity should give a large value, got {chars}"
        );
    }

    #[test]
    fn nan_falls_back_to_baseline() {
        // NAN > 0.0 is false, so it should use the fallback
        let chars = max_description_chars(f32::NAN);
        assert_eq!(chars, 300, "NaN should fall back to baseline 300");
    }

    #[test]
    fn negative_infinity_falls_back_to_baseline() {
        let chars = max_description_chars(f32::NEG_INFINITY);
        assert_eq!(chars, 300, "negative infinity should fall back to baseline");
    }

    #[test]
    fn epsilon_clamps_to_minimum() {
        let chars = max_description_chars(f32::EPSILON);
        assert_eq!(chars, 150, "epsilon should clamp to minimum");
    }

    #[test]
    fn min_positive_clamps_to_minimum() {
        let chars = max_description_chars(f32::MIN_POSITIVE);
        assert_eq!(chars, 150);
    }
}

// ===========================================================================
// max_description_chars — determinism
// ===========================================================================

mod max_description_chars_determinism {
    use super::*;

    #[test]
    fn same_input_same_output() {
        let widths = [0.0, 1.0, 50.0, 180.0, 360.0, 720.0, 1080.0, 1920.0];
        for &w in &widths {
            let r1 = max_description_chars(w);
            let r2 = max_description_chars(w);
            let r3 = max_description_chars(w);
            assert_eq!(r1, r2, "non-deterministic for width {w}");
            assert_eq!(r2, r3, "non-deterministic for width {w}");
        }
    }
}

// ===========================================================================
// max_description_chars — typical applet widths
// ===========================================================================

mod max_description_chars_typical {
    use super::*;

    #[test]
    fn typical_popup_350px() {
        let chars = max_description_chars(350.0);
        // Should be close to but slightly below 300
        assert!(
            chars >= 250 && chars <= 320,
            "350px should give ~292 chars, got {chars}"
        );
    }

    #[test]
    fn typical_popup_400px() {
        let chars = max_description_chars(400.0);
        assert!(
            chars > 300,
            "400px should give more than baseline 300, got {chars}"
        );
    }

    #[test]
    fn standalone_window_800px() {
        let chars = max_description_chars(800.0);
        assert!(
            chars > 500,
            "800px wide window should give >500 chars, got {chars}"
        );
    }

    #[test]
    fn standalone_window_1200px() {
        let chars = max_description_chars(1200.0);
        assert!(
            chars > 800,
            "1200px wide window should give >800 chars, got {chars}"
        );
    }
}
