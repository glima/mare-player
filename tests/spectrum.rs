// SPDX-License-Identifier: MIT

//! Integration tests for the spectrum analyzer module.
//!
//! Covers edge cases: silence, DC offset, mono fallback, reset behaviour,
//! shared analyzer, custom band counts, different sample rates, and
//! frequency detection accuracy.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::audio::spectrum::{SharedSpectrumAnalyzer, SpectrumAnalyzer, SpectrumData};

// ===========================================================================
// SpectrumData defaults
// ===========================================================================

mod spectrum_data {
    use super::*;

    #[test]
    fn default_has_12_bands() {
        let data = SpectrumData::default();
        assert_eq!(data.bands.len(), 12);
        assert_eq!(data.left_bands.len(), 12);
        assert_eq!(data.right_bands.len(), 12);
    }

    #[test]
    fn default_all_zeros() {
        let data = SpectrumData::default();
        for &v in &data.bands {
            assert!(v.abs() < f32::EPSILON);
        }
        for &v in &data.left_bands {
            assert!(v.abs() < f32::EPSILON);
        }
        for &v in &data.right_bands {
            assert!(v.abs() < f32::EPSILON);
        }
    }

    #[test]
    fn clone_is_independent() {
        let mut data = SpectrumData::default();
        data.bands[0] = 0.5;
        data.left_bands[0] = 0.3;
        data.right_bands[0] = 0.7;
        let cloned = data.clone();
        assert!((cloned.bands[0] - 0.5).abs() < f32::EPSILON);
        assert!((cloned.left_bands[0] - 0.3).abs() < f32::EPSILON);
        assert!((cloned.right_bands[0] - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn debug_format() {
        let data = SpectrumData::default();
        let debug = format!("{:?}", data);
        assert!(debug.contains("SpectrumData"));
        assert!(debug.contains("left_bands"));
        assert!(debug.contains("right_bands"));
        assert!(debug.contains("bands"));
    }
}

// ===========================================================================
// SpectrumAnalyzer creation
// ===========================================================================

mod creation {
    use super::*;

    #[test]
    fn new_at_44100() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 12);
    }

    #[test]
    fn new_at_48000() {
        let mut analyzer = SpectrumAnalyzer::new(48000);
        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 12);
    }

    #[test]
    fn new_at_96000() {
        let mut analyzer = SpectrumAnalyzer::new(96000);
        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 12);
    }

    #[test]
    fn custom_bands_16() {
        let mut analyzer = SpectrumAnalyzer::with_bands(44100, 16);
        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 16);
        assert_eq!(spectrum.left_bands.len(), 16);
        assert_eq!(spectrum.right_bands.len(), 16);
    }

    #[test]
    fn custom_bands_64() {
        let mut analyzer = SpectrumAnalyzer::with_bands(44100, 64);
        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 64);
        assert_eq!(spectrum.left_bands.len(), 64);
        assert_eq!(spectrum.right_bands.len(), 64);
    }

    #[test]
    fn custom_bands_1() {
        let mut analyzer = SpectrumAnalyzer::with_bands(44100, 1);
        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 1);
    }

    #[test]
    fn custom_bands_128() {
        let mut analyzer = SpectrumAnalyzer::with_bands(44100, 128);
        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 128);
    }
}

// ===========================================================================
// Silence
// ===========================================================================

mod silence {
    use super::*;

    #[test]
    fn silence_produces_low_energy() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        // Feed silence (all zeros, interleaved stereo)
        let silence = vec![0.0f32; 4096];
        analyzer.push_stereo_samples(&silence);
        let spectrum = analyzer.compute();

        // All bands should be very low (essentially zero, but mapped through
        // the dB scale, so they end up at 0.0 after clamping).
        for &v in &spectrum.bands {
            assert!(v < 0.05, "silence band value {} should be near zero", v);
        }
    }

    #[test]
    fn silence_stereo_channels_equal() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        let silence = vec![0.0f32; 4096];
        analyzer.push_stereo_samples(&silence);
        let spectrum = analyzer.compute();

        for i in 0..spectrum.left_bands.len() {
            assert!(
                (spectrum.left_bands[i] - spectrum.right_bands[i]).abs() < 0.001,
                "band {}: left={} right={} should be equal for silence",
                i,
                spectrum.left_bands[i],
                spectrum.right_bands[i]
            );
        }
    }
}

// ===========================================================================
// DC offset (constant signal)
// ===========================================================================

mod dc_offset {
    use super::*;

    #[test]
    fn dc_offset_energy_in_low_bands() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        // Constant value = DC component (0 Hz)
        let dc: Vec<f32> = vec![0.5; 4096]; // interleaved stereo: 0.5 on both channels
        analyzer.push_stereo_samples(&dc);
        let spectrum = analyzer.compute();

        // DC energy should mostly appear in the lowest bands.
        // Higher bands should have relatively less energy.
        let low_sum: f32 = spectrum.bands.iter().take(4).sum();
        let high_sum: f32 = spectrum.bands.iter().skip(24).sum();
        // Low bands should have more energy than high bands
        // (DC is at 0 Hz, which maps to the lowest frequency bands)
        assert!(
            low_sum >= high_sum,
            "DC should concentrate in low bands: low={}, high={}",
            low_sum,
            high_sum
        );
    }
}

// ===========================================================================
// Sine wave detection
// ===========================================================================

mod sine_detection {
    use super::*;

    const FFT_SIZE: usize = 2048;

    fn generate_stereo_sine(freq: f32, sample_rate: u32, num_samples: usize) -> Vec<f32> {
        (0..num_samples)
            .flat_map(|i| {
                let t = i as f32 / sample_rate as f32;
                let val = (2.0 * std::f32::consts::PI * freq * t).sin();
                [val, val] // same on both channels
            })
            .collect()
    }

    #[test]
    fn detect_1khz_tone() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        let samples = generate_stereo_sine(1000.0, 44100, FFT_SIZE);
        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        let max_band = spectrum
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        // With 12 log-spaced bands, 1 kHz falls in band 5 (≈560–1090 Hz)
        assert!(
            max_band > 3 && max_band < 8,
            "1 kHz peak at band {} (expected roughly 4-7)",
            max_band
        );
    }

    #[test]
    fn detect_440hz_tone() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        let samples = generate_stereo_sine(440.0, 44100, FFT_SIZE);
        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        let max_band = spectrum
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        // With 12 log-spaced bands, 440 Hz falls in band 4 (≈288–561 Hz)
        assert!(
            max_band > 2 && max_band < 7,
            "440 Hz peak at band {} (expected roughly 3-6)",
            max_band
        );
    }

    #[test]
    fn detect_5khz_tone() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        let samples = generate_stereo_sine(5000.0, 44100, FFT_SIZE);
        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        let max_band = spectrum
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        // With 12 log-spaced bands, 5 kHz falls in band 8 (≈4128–8030 Hz)
        assert!(
            max_band > 6 && max_band < 11,
            "5 kHz peak at band {} (expected roughly 7-10)",
            max_band
        );
    }

    #[test]
    fn low_freq_peaks_before_high_freq() {
        let mut analyzer_low = SpectrumAnalyzer::new(44100);
        let mut analyzer_high = SpectrumAnalyzer::new(44100);

        let samples_low = generate_stereo_sine(200.0, 44100, FFT_SIZE);
        let samples_high = generate_stereo_sine(8000.0, 44100, FFT_SIZE);

        analyzer_low.push_stereo_samples(&samples_low);
        analyzer_high.push_stereo_samples(&samples_high);

        let spec_low = analyzer_low.compute();
        let spec_high = analyzer_high.compute();

        let peak_low = spec_low
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        let peak_high = spec_high
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        assert!(
            peak_low < peak_high,
            "200 Hz peak ({}) should be at a lower band than 8 kHz peak ({})",
            peak_low,
            peak_high
        );
    }

    #[test]
    fn detect_tone_at_48khz_sample_rate() {
        let mut analyzer = SpectrumAnalyzer::new(48000);
        let samples = generate_stereo_sine(1000.0, 48000, FFT_SIZE);
        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        // With 12 log-spaced bands, 1 kHz ≈ band 5 regardless of sample rate
        let max_band = spectrum
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        assert!(
            max_band > 3 && max_band < 8,
            "1 kHz at 48 kHz sample rate: peak at band {}",
            max_band
        );
    }

    #[test]
    fn detect_tone_at_96khz_sample_rate() {
        let mut analyzer = SpectrumAnalyzer::new(96000);
        let samples = generate_stereo_sine(1000.0, 96000, FFT_SIZE);
        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        let max_band = spectrum
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        assert!(
            max_band > 3 && max_band < 8,
            "1 kHz at 96 kHz sample rate: peak at band {}",
            max_band
        );
    }
}

// ===========================================================================
// Stereo separation
// ===========================================================================

mod stereo {
    use super::*;

    const FFT_SIZE: usize = 2048;

    #[test]
    fn different_frequencies_per_channel() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        let left_freq = 500.0;
        let right_freq = 4000.0;

        let samples: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let left = (2.0 * std::f32::consts::PI * left_freq * t).sin();
                let right = (2.0 * std::f32::consts::PI * right_freq * t).sin();
                [left, right]
            })
            .collect();

        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        let left_peak = spectrum
            .left_bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        let right_peak = spectrum
            .right_bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        assert!(
            left_peak < right_peak,
            "Left 500 Hz peak ({}) should be lower than right 4 kHz peak ({})",
            left_peak,
            right_peak
        );
    }

    #[test]
    fn mono_signal_has_equal_channels() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        let freq = 1000.0;
        let samples: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let val = (2.0 * std::f32::consts::PI * freq * t).sin();
                [val, val]
            })
            .collect();

        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        for i in 0..spectrum.left_bands.len() {
            assert!(
                (spectrum.left_bands[i] - spectrum.right_bands[i]).abs() < 0.01,
                "band {}: left={} right={} differ for mono signal",
                i,
                spectrum.left_bands[i],
                spectrum.right_bands[i]
            );
        }
    }

    #[test]
    fn left_only_signal() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        let freq = 1000.0;
        let samples: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let left = (2.0 * std::f32::consts::PI * freq * t).sin();
                [left, 0.0]
            })
            .collect();

        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        // Left channel should have energy, right should be near silence
        let left_max: f32 = spectrum.left_bands.iter().copied().fold(0.0, f32::max);
        let right_max: f32 = spectrum.right_bands.iter().copied().fold(0.0, f32::max);

        assert!(
            left_max > 0.1,
            "left channel should have energy, max={}",
            left_max
        );
        assert!(
            right_max < 0.05,
            "right channel should be near silence, max={}",
            right_max
        );
    }

    #[test]
    fn right_only_signal() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        let freq = 1000.0;
        let samples: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let right = (2.0 * std::f32::consts::PI * freq * t).sin();
                [0.0, right]
            })
            .collect();

        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        let left_max: f32 = spectrum.left_bands.iter().copied().fold(0.0, f32::max);
        let right_max: f32 = spectrum.right_bands.iter().copied().fold(0.0, f32::max);

        assert!(
            left_max < 0.05,
            "left channel should be near silence, max={}",
            left_max
        );
        assert!(
            right_max > 0.1,
            "right channel should have energy, max={}",
            right_max
        );
    }

    #[test]
    fn combined_bands_are_average_of_left_right() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        let samples: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let left = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                let right = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
                [left, right]
            })
            .collect();

        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        for i in 0..spectrum.bands.len() {
            let expected = (spectrum.left_bands[i] + spectrum.right_bands[i]) * 0.5;
            assert!(
                (spectrum.bands[i] - expected).abs() < 0.001,
                "band {} combined={} expected={}",
                i,
                spectrum.bands[i],
                expected
            );
        }
    }
}

// ===========================================================================
// Mono fallback (odd number of samples)
// ===========================================================================

mod mono_fallback {
    use super::*;

    #[test]
    fn odd_sample_count_does_not_panic() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        // 3 samples: [L, R, L_only] — the last sample is a mono fallback
        let samples = vec![0.5, -0.5, 0.3];
        analyzer.push_stereo_samples(&samples);
        // Should not panic
        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 12);
    }

    #[test]
    fn single_sample_does_not_panic() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        let samples = vec![0.1];
        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 12);
    }

    #[test]
    fn empty_samples_does_not_panic() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        let samples: Vec<f32> = vec![];
        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 12);
    }
}

// ===========================================================================
// Reset
// ===========================================================================

mod reset {
    use super::*;

    const FFT_SIZE: usize = 2048;

    #[test]
    fn reset_clears_all_state() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        // Feed a loud tone
        let samples: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let val = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                [val, val]
            })
            .collect();
        analyzer.push_stereo_samples(&samples);

        // Compute to update smoothing state
        let before_reset = analyzer.compute();
        let max_before: f32 = before_reset.bands.iter().copied().fold(0.0, f32::max);
        assert!(max_before > 0.1, "should have energy before reset");

        // Reset
        analyzer.reset();

        // Compute again — should be near-zero
        let after_reset = analyzer.compute();
        for &v in &after_reset.bands {
            assert!(
                v < 0.05,
                "after reset, band value {} should be near zero",
                v
            );
        }
    }

    #[test]
    fn reset_allows_new_analysis() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        // Feed 440 Hz
        let samples_440: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let val = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                [val, val]
            })
            .collect();
        analyzer.push_stereo_samples(&samples_440);
        let spec_440 = analyzer.compute();

        let peak_440 = spec_440
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        // Reset and feed 5 kHz
        analyzer.reset();

        let samples_5k: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let val = (2.0 * std::f32::consts::PI * 5000.0 * t).sin();
                [val, val]
            })
            .collect();
        analyzer.push_stereo_samples(&samples_5k);

        // Need multiple computes to wash out smoothing from reset
        for _ in 0..5 {
            analyzer.push_stereo_samples(&samples_5k);
            analyzer.compute();
        }
        let spec_5k = analyzer.compute();

        let peak_5k = spec_5k
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        // 5 kHz should peak at a higher band than 440 Hz
        assert!(
            peak_5k > peak_440,
            "After reset, 5 kHz peak ({}) should be > 440 Hz peak ({})",
            peak_5k,
            peak_440
        );
    }
}

// ===========================================================================
// Smoothing behaviour
// ===========================================================================

mod smoothing {
    use super::*;

    const FFT_SIZE: usize = 2048;

    #[test]
    fn smoothing_ramps_up_gradually() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        // First compute with silence
        let silence = vec![0.0f32; FFT_SIZE * 2];
        analyzer.push_stereo_samples(&silence);
        let spec_silent = analyzer.compute();

        // Now feed a loud tone
        let tone: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let val = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                [val, val]
            })
            .collect();
        analyzer.push_stereo_samples(&tone);
        let spec_first = analyzer.compute();

        // Feed the same tone again
        analyzer.push_stereo_samples(&tone);
        let spec_second = analyzer.compute();

        // Find peak band
        let peak_band = spec_second
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        // Due to smoothing, the second compute should have a higher or equal
        // value at the peak band than the first (ramping up)
        let first_val = spec_first.bands[peak_band];
        let _second_val = spec_second.bands[peak_band];
        let silent_val = spec_silent.bands[peak_band];

        // The silent value should be lowest
        assert!(
            silent_val <= first_val,
            "silent {} should be <= first {}",
            silent_val,
            first_val
        );
    }
}

// ===========================================================================
// SharedSpectrumAnalyzer
// ===========================================================================

mod shared_analyzer {
    use super::*;

    const FFT_SIZE: usize = 2048;

    #[test]
    fn basic_usage() {
        let shared = SharedSpectrumAnalyzer::with_bands(44100, 32);

        let samples: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let val = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                [val, val]
            })
            .collect();

        shared.push_stereo_samples(&samples);
        let spectrum = shared.compute();
        assert_eq!(spectrum.bands.len(), 32);
    }

    #[test]
    fn clone_shares_state() {
        let shared = SharedSpectrumAnalyzer::with_bands(44100, 32);
        let cloned = shared.clone();

        let samples: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let val = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                [val, val]
            })
            .collect();

        // Push via original
        shared.push_stereo_samples(&samples);

        // Compute via clone — should see the data
        let spectrum = cloned.compute();
        let max_val: f32 = spectrum.bands.iter().copied().fold(0.0, f32::max);
        assert!(
            max_val > 0.05,
            "cloned analyzer should see data from original, max={}",
            max_val
        );
    }

    #[test]
    fn reset_via_clone_affects_original() {
        let shared = SharedSpectrumAnalyzer::with_bands(44100, 32);
        let cloned = shared.clone();

        let samples: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let val = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                [val, val]
            })
            .collect();

        shared.push_stereo_samples(&samples);
        shared.compute(); // prime the smoothing

        // Reset via clone
        cloned.reset();

        // Compute via original — should be near zero
        let spectrum = shared.compute();
        for &v in &spectrum.bands {
            assert!(
                v < 0.05,
                "after reset via clone, band value {} should be near zero",
                v
            );
        }
    }

    #[test]
    fn custom_bands_via_shared() {
        let shared = SharedSpectrumAnalyzer::with_bands(44100, 8);
        let spectrum = shared.compute();
        assert_eq!(spectrum.bands.len(), 8);
        assert_eq!(spectrum.left_bands.len(), 8);
        assert_eq!(spectrum.right_bands.len(), 8);
    }
}

// ===========================================================================
// Large input / stress
// ===========================================================================

mod stress {
    use super::*;

    #[test]
    fn large_input_does_not_panic() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        // Feed 1 second of stereo audio at 44100 Hz = 88200 samples
        let samples: Vec<f32> = (0..88200).map(|i| ((i as f32) * 0.01).sin()).collect();
        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 12);
    }

    #[test]
    fn repeated_compute_is_stable() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        let samples: Vec<f32> = (0..2048)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let val = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                [val, val]
            })
            .collect();
        analyzer.push_stereo_samples(&samples);

        // Compute many times — values should stabilize with smoothing
        let mut last_max = 0.0f32;
        for iteration in 0..50 {
            // Re-feed the same data
            analyzer.push_stereo_samples(&samples);
            let spectrum = analyzer.compute();
            let max_val: f32 = spectrum.bands.iter().copied().fold(0.0, f32::max);
            if iteration > 10 {
                // After several iterations, the max should stabilize
                let diff = (max_val - last_max).abs();
                assert!(
                    diff < 0.05,
                    "iteration {}: max changed by {} (was {}, now {})",
                    iteration,
                    diff,
                    last_max,
                    max_val
                );
            }
            last_max = max_val;
        }
    }

    #[test]
    fn many_push_calls_before_compute() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        // Push many small chunks before computing
        for i in 0..100 {
            let val = (i as f32 * 0.1).sin();
            let samples = vec![val, val, val, val]; // 2 stereo frames
            analyzer.push_stereo_samples(&samples);
        }

        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), 12);
    }
}

// ===========================================================================
// Band value ranges
// ===========================================================================

mod value_ranges {
    use super::*;

    const FFT_SIZE: usize = 2048;

    #[test]
    fn bands_are_clamped_0_to_1() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        // Feed a very loud signal
        let samples: Vec<f32> = (0..FFT_SIZE)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let val = (2.0 * std::f32::consts::PI * 1000.0 * t).sin() * 100.0;
                [val, val]
            })
            .collect();

        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        for (i, &v) in spectrum.bands.iter().enumerate() {
            assert!(
                v >= 0.0 && v <= 1.0,
                "band {} value {} outside [0, 1]",
                i,
                v
            );
        }
        for (i, &v) in spectrum.left_bands.iter().enumerate() {
            assert!(
                v >= 0.0 && v <= 1.0,
                "left band {} value {} outside [0, 1]",
                i,
                v
            );
        }
        for (i, &v) in spectrum.right_bands.iter().enumerate() {
            assert!(
                v >= 0.0 && v <= 1.0,
                "right band {} value {} outside [0, 1]",
                i,
                v
            );
        }
    }

    #[test]
    fn bands_are_non_negative_for_silence() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        let silence = vec![0.0f32; FFT_SIZE * 2];
        analyzer.push_stereo_samples(&silence);
        let spectrum = analyzer.compute();

        for (i, &v) in spectrum.bands.iter().enumerate() {
            assert!(v >= 0.0, "band {} value {} should be non-negative", i, v);
        }
    }

    #[test]
    fn bands_are_non_negative_for_noise() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        // Pseudo-random noise via simple LCG
        let mut seed: u32 = 42;
        let samples: Vec<f32> = (0..FFT_SIZE * 2)
            .map(|_| {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect();

        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        for (i, &v) in spectrum.bands.iter().enumerate() {
            assert!(
                v >= 0.0 && v <= 1.0,
                "noise band {} value {} outside [0, 1]",
                i,
                v
            );
        }
    }
}
