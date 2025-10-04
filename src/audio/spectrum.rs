// SPDX-License-Identifier: MIT

//! Real-time spectrum analyzer using FFT.
//!
//! This module provides frequency spectrum analysis of audio samples
//! using the rustfft library for real FFT computation.
//! Supports stereo analysis with separate left and right channel data.

use parking_lot::Mutex;
use rustfft::{FftPlanner, num_complex::Complex};
use std::sync::Arc;
use tracing::debug;

/// Size of the FFT window (must be power of 2)
const FFT_SIZE: usize = 2048;

/// Number of frequency bands to output for visualization.
///
/// Matches the visualizer's `NUM_BARS` (12) so the backend produces exactly
/// one band per visible bar — no wasteful oversampling.
const NUM_BANDS: usize = 12;

/// Smoothing factor for spectrum data (0.0 = no smoothing, 1.0 = full smoothing).
///
/// Kept low so the visualizer widget receives near-raw FFT magnitudes and
/// can apply its own asymmetric attack/decay smoothing for snappy response.
const SMOOTHING_FACTOR: f32 = 0.3;

/// Spectrum data for visualization (stereo)
#[derive(Debug, Clone)]
pub struct SpectrumData {
    /// Left channel frequency band magnitudes (0.0 to 1.0), from low to high frequency
    pub left_bands: Vec<f32>,
    /// Right channel frequency band magnitudes (0.0 to 1.0), from low to high frequency
    pub right_bands: Vec<f32>,
    /// Combined/mono bands for backwards compatibility (used in tests)
    #[allow(dead_code)]
    pub bands: Vec<f32>,
}

impl Default for SpectrumData {
    fn default() -> Self {
        Self {
            left_bands: vec![0.0; NUM_BANDS],
            right_bands: vec![0.0; NUM_BANDS],
            bands: vec![0.0; NUM_BANDS],
        }
    }
}

/// Real-time spectrum analyzer with stereo support
pub struct SpectrumAnalyzer {
    /// FFT planner for efficient FFT computation
    fft: Arc<dyn rustfft::Fft<f32>>,
    /// Left channel sample buffer for FFT input
    left_buffer: Vec<f32>,
    /// Right channel sample buffer for FFT input
    right_buffer: Vec<f32>,
    /// Write position in the sample buffers
    write_pos: usize,
    /// Hann window coefficients for reducing spectral leakage
    window: Vec<f32>,
    /// Previous left spectrum data for smoothing
    prev_left_spectrum: Vec<f32>,
    /// Previous right spectrum data for smoothing
    prev_right_spectrum: Vec<f32>,
    /// Sample rate of the audio
    sample_rate: u32,
    /// Number of output bands
    num_bands: usize,
}

impl SpectrumAnalyzer {
    /// Create a new spectrum analyzer
    pub fn new(sample_rate: u32) -> Self {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);

        // Pre-compute Hann window
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let x = std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32;
                0.5 * (1.0 - (2.0 * x).cos())
            })
            .collect();

        debug!(
            "SpectrumAnalyzer initialized (stereo): FFT_SIZE={}, sample_rate={}, num_bands={}",
            FFT_SIZE, sample_rate, NUM_BANDS
        );

        Self {
            fft,
            left_buffer: vec![0.0; FFT_SIZE],
            right_buffer: vec![0.0; FFT_SIZE],
            write_pos: 0,
            window,
            prev_left_spectrum: vec![0.0; NUM_BANDS],
            prev_right_spectrum: vec![0.0; NUM_BANDS],
            sample_rate,
            num_bands: NUM_BANDS,
        }
    }

    /// Create a spectrum analyzer with custom number of bands
    pub fn with_bands(sample_rate: u32, num_bands: usize) -> Self {
        let mut analyzer = Self::new(sample_rate);
        analyzer.num_bands = num_bands;
        analyzer.prev_left_spectrum = vec![0.0; num_bands];
        analyzer.prev_right_spectrum = vec![0.0; num_bands];
        analyzer
    }

    /// Push interleaved stereo samples, keeping channels separate
    pub fn push_stereo_samples(&mut self, samples: &[f32]) {
        for chunk in samples.chunks(2) {
            let (left, right) = match (chunk.first(), chunk.get(1)) {
                (Some(&l), Some(&r)) => (l, r),
                (Some(&l), None) => (l, l), // Mono fallback
                _ => continue,
            };

            if let Some(slot) = self.left_buffer.get_mut(self.write_pos) {
                *slot = left;
            }
            if let Some(slot) = self.right_buffer.get_mut(self.write_pos) {
                *slot = right;
            }
            self.write_pos = (self.write_pos + 1) % FFT_SIZE;
        }
    }

    /// Compute FFT for a single channel buffer
    fn compute_channel_fft(&self, buffer: &[f32]) -> Vec<f32> {
        // Prepare FFT input with windowing
        let mut fft_input: Vec<Complex<f32>> = Vec::with_capacity(FFT_SIZE);

        // Read samples in order, starting from write_pos (oldest sample)
        for i in 0..FFT_SIZE {
            let idx = (self.write_pos + i) % FFT_SIZE;
            let sample = buffer.get(idx).copied().unwrap_or(0.0);
            let window_val = self.window.get(i).copied().unwrap_or(1.0);
            let windowed = sample * window_val;
            fft_input.push(Complex::new(windowed, 0.0));
        }

        // Perform FFT in-place
        self.fft.process(&mut fft_input);

        // Convert to magnitude spectrum (only first half is meaningful for real input)
        let half_size = FFT_SIZE / 2;
        fft_input.iter().take(half_size).map(|c| c.norm()).collect()
    }

    /// Compute the current spectrum and return band magnitudes for both channels
    pub fn compute(&mut self) -> SpectrumData {
        // Compute FFT for both channels
        let left_magnitudes = self.compute_channel_fft(&self.left_buffer);
        let right_magnitudes = self.compute_channel_fft(&self.right_buffer);

        // Map FFT bins to logarithmic frequency bands
        let left_bands = self.map_to_bands(&left_magnitudes);
        let right_bands = self.map_to_bands(&right_magnitudes);

        // Apply smoothing to left channel
        let mut smoothed_left = Vec::with_capacity(self.num_bands);
        for (&band_val, prev_val) in left_bands
            .iter()
            .zip(self.prev_left_spectrum.iter_mut())
            .take(self.num_bands)
        {
            let smoothed = SMOOTHING_FACTOR * *prev_val + (1.0 - SMOOTHING_FACTOR) * band_val;
            smoothed_left.push(smoothed);
            *prev_val = smoothed;
        }

        // Apply smoothing to right channel
        let mut smoothed_right = Vec::with_capacity(self.num_bands);
        for (&band_val, prev_val) in right_bands
            .iter()
            .zip(self.prev_right_spectrum.iter_mut())
            .take(self.num_bands)
        {
            let smoothed = SMOOTHING_FACTOR * *prev_val + (1.0 - SMOOTHING_FACTOR) * band_val;
            smoothed_right.push(smoothed);
            *prev_val = smoothed;
        }

        // Compute combined/mono bands for backwards compatibility
        let combined_bands: Vec<f32> = smoothed_left
            .iter()
            .zip(smoothed_right.iter())
            .map(|(&l, &r)| (l + r) * 0.5)
            .collect();

        SpectrumData {
            left_bands: smoothed_left,
            right_bands: smoothed_right,
            bands: combined_bands,
        }
    }

    /// Map linear FFT bins to logarithmic frequency bands
    fn map_to_bands(&self, magnitudes: &[f32]) -> Vec<f32> {
        let mut bands = vec![0.0f32; self.num_bands];
        let half_size = magnitudes.len();

        // Frequency range: ~20Hz to ~20kHz (or Nyquist, whichever is lower)
        let min_freq = 20.0f32;
        let max_freq = (self.sample_rate as f32 / 2.0).min(20000.0);

        // Calculate frequency for each FFT bin
        let bin_freq =
            |bin: usize| -> f32 { bin as f32 * self.sample_rate as f32 / FFT_SIZE as f32 };

        // Calculate band edges using logarithmic spacing
        let log_min = min_freq.ln();
        let log_max = max_freq.ln();

        for (band, band_val) in bands.iter_mut().enumerate().take(self.num_bands) {
            // Calculate frequency range for this band
            let t0 = band as f32 / self.num_bands as f32;
            let t1 = (band + 1) as f32 / self.num_bands as f32;

            let freq_low = (log_min + t0 * (log_max - log_min)).exp();
            let freq_high = (log_min + t1 * (log_max - log_min)).exp();

            // Find FFT bins that fall within this frequency range
            let mut sum = 0.0f32;
            let mut count = 0;

            for (bin, &mag) in magnitudes.iter().enumerate().take(half_size) {
                let freq = bin_freq(bin);
                if freq >= freq_low && freq < freq_high {
                    sum += mag;
                    count += 1;
                }
            }

            // Average the magnitudes in this band and store directly
            if count > 0 {
                *band_val = sum / count as f32;
            }
        }

        // Normalize bands to 0.0-1.0 range
        // Use a reference level based on FFT size for consistent scaling
        let reference = FFT_SIZE as f32 / 4.0;
        for band in &mut bands {
            // Apply logarithmic scaling for better visual response
            // Add small epsilon to avoid log(0)
            let db = 20.0 * (*band / reference + 1e-10).log10();
            // Map -60dB to 0dB range to 0.0 to 1.0
            *band = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        }

        bands
    }

    /// Reset the analyzer state
    pub fn reset(&mut self) {
        self.left_buffer.fill(0.0);
        self.right_buffer.fill(0.0);
        self.write_pos = 0;
        self.prev_left_spectrum.fill(0.0);
        self.prev_right_spectrum.fill(0.0);
    }
}

/// Thread-safe spectrum analyzer wrapper
impl std::fmt::Debug for SharedSpectrumAnalyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedSpectrumAnalyzer")
            .finish_non_exhaustive()
    }
}

pub struct SharedSpectrumAnalyzer {
    inner: Arc<Mutex<SpectrumAnalyzer>>,
}

impl SharedSpectrumAnalyzer {
    /// Create with custom number of bands
    pub fn with_bands(sample_rate: u32, num_bands: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SpectrumAnalyzer::with_bands(
                sample_rate,
                num_bands,
            ))),
        }
    }

    /// Push stereo samples
    pub fn push_stereo_samples(&self, samples: &[f32]) {
        self.inner.lock().push_stereo_samples(samples);
    }

    /// Compute current spectrum
    pub fn compute(&self) -> SpectrumData {
        self.inner.lock().compute()
    }

    /// Reset the analyzer
    pub fn reset(&self) {
        self.inner.lock().reset();
    }
}

impl Clone for SharedSpectrumAnalyzer {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spectrum_analyzer_creation() {
        let analyzer = SpectrumAnalyzer::new(44100);
        assert_eq!(analyzer.num_bands, NUM_BANDS);
    }

    #[test]
    fn test_spectrum_data_default() {
        let data = SpectrumData::default();
        assert_eq!(data.bands.len(), NUM_BANDS);
        assert_eq!(data.left_bands.len(), NUM_BANDS);
        assert_eq!(data.right_bands.len(), NUM_BANDS);
    }

    #[test]
    fn test_push_stereo_samples() {
        let mut analyzer = SpectrumAnalyzer::new(44100);
        // Stereo samples (interleaved L/R)
        let samples: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.01).sin()).collect();
        analyzer.push_stereo_samples(&samples);

        let spectrum = analyzer.compute();
        assert_eq!(spectrum.bands.len(), NUM_BANDS);
        assert_eq!(spectrum.left_bands.len(), NUM_BANDS);
        assert_eq!(spectrum.right_bands.len(), NUM_BANDS);
    }

    #[test]
    fn test_stereo_separation() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        // Generate different frequencies on left and right channels
        let left_freq = 500.0;
        let right_freq = 2000.0;

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

        // Find peak bands for each channel
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

        // The peaks should be at different bands (lower freq = lower band index)
        assert!(
            left_peak < right_peak,
            "Left peak {} should be lower than right peak {} (500Hz vs 2000Hz)",
            left_peak,
            right_peak
        );
    }

    #[test]
    fn test_sine_wave_detection() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        // Generate a 1kHz sine wave (stereo, same on both channels)
        let freq = 1000.0;
        let mono_samples: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let t = i as f32 / 44100.0;
                (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect();

        // Convert to stereo
        let samples: Vec<f32> = mono_samples.iter().flat_map(|&s| [s, s]).collect();

        analyzer.push_stereo_samples(&samples);
        let spectrum = analyzer.compute();

        // The 1kHz band should have significant energy.
        // With 12 bands logarithmically spaced from 20 Hz to 20 kHz,
        // 1 kHz falls in band 5 (≈560–1090 Hz).
        let max_band = spectrum
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        // 1 kHz should be detected in the mid bands
        assert!(max_band > 3 && max_band < 8, "Max band was {}", max_band);
    }

    #[test]
    fn test_combined_bands_average() {
        let mut analyzer = SpectrumAnalyzer::new(44100);

        // Generate stereo samples
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

        // Combined bands should be average of left and right
        for i in 0..spectrum.bands.len() {
            let expected = (spectrum.left_bands[i] + spectrum.right_bands[i]) * 0.5;
            assert!(
                (spectrum.bands[i] - expected).abs() < 0.001,
                "Band {} mismatch: {} vs expected {}",
                i,
                spectrum.bands[i],
                expected
            );
        }
    }
}
