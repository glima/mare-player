// SPDX-License-Identifier: MIT

//! DASH streaming support for HiRes audio.
//!
//! This module handles parsing DASH (Dynamic Adaptive Streaming over
//! HTTP) MPD (Media Presentation Description) manifests and resolving
//! segment URLs for playback. TIDAL uses DASH for HiRes (FLAC)
//! streaming. We use the `dash-mpd` crate for robust manifest parsing;
//! actual HTTP downloading is handled by the caller (see
//! [`super::decoder`]).

use dash_mpd::{parse, MPD};
use regex::Regex;
use tracing::{info, warn};

/// Result type for DASH operations
pub type DashResult<T> = Result<T, DashError>;

/// Errors that can occur during DASH streaming
#[derive(Debug)]
pub enum DashError {
    /// Failed to parse manifest
    ParseError(String),
    /// No audio representation found
    NoAudioTrack,
    /// Invalid manifest structure
    InvalidManifest(String),
}

impl std::fmt::Display for DashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParseError(msg) => write!(f, "Parse error: {}", msg),
            Self::NoAudioTrack => write!(f, "No audio track found in manifest"),
            Self::InvalidManifest(msg) => write!(f, "Invalid manifest: {}", msg),
        }
    }
}

impl std::error::Error for DashError {}

/// Information about a DASH audio stream
#[derive(Debug, Clone)]
pub struct DashAudioInfo {
    /// Codec (e.g., "flac")
    pub codec: String,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Duration in seconds
    pub duration: f64,
    /// Number of segments
    pub segment_count: usize,
}

/// A parsed DASH manifest ready for streaming
#[derive(Debug)]
pub struct DashManifest {
    /// Audio stream info
    pub audio_info: DashAudioInfo,
    /// Initialization segment URL
    pub init_url: String,
    /// Media segment URLs (resolved, ready to download)
    pub segment_urls: Vec<String>,
}

impl DashManifest {
    /// Sanitize DASH manifest to fix compatibility issues with dash-mpd crate
    /// Some manifests have attributes that don't match expected types
    fn sanitize_manifest(content: &str) -> String {
        let mut sanitized = content.to_string();

        // Fix group="main" -> group="0" (dash-mpd expects integer)
        if let Ok(re) = Regex::new(r#"group\s*=\s*"[^"]*[a-zA-Z][^"]*""#) {
            sanitized = re.replace_all(&sanitized, r#"group="0""#).to_string();
        }

        // Fix any other string values for numeric attributes
        // contentType is fine as string, but group/bandwidth/etc should be numeric
        if let Ok(re) = Regex::new(r#"bandwidth\s*=\s*"[^"]*[a-zA-Z][^"]*""#) {
            sanitized = re.replace_all(&sanitized, r#"bandwidth="0""#).to_string();
        }

        if sanitized != content {
            warn!("DASH manifest was sanitized to fix compatibility issues");
        }

        sanitized
    }

    /// Parse a DASH manifest from string content
    pub fn parse(content: &str) -> DashResult<Self> {
        // Sanitize manifest before parsing to handle compatibility issues
        let sanitized_content = Self::sanitize_manifest(content);

        let mpd: MPD = parse(&sanitized_content)
            .map_err(|e| DashError::ParseError(format!("MPD parse error: {}", e)))?;

        // Get duration from MPD
        let duration = mpd
            .mediaPresentationDuration
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        // Find the first audio adaptation set
        let period = mpd
            .periods
            .first()
            .ok_or(DashError::InvalidManifest("No periods in manifest".into()))?;

        let audio_adaptation = period
            .adaptations
            .iter()
            .find(|a| {
                a.contentType.as_deref() == Some("audio")
                    || a.mimeType
                        .as_deref()
                        .map(|m| m.contains("audio"))
                        .unwrap_or(false)
            })
            .ok_or(DashError::NoAudioTrack)?;

        // Get the first (usually only/best) representation
        let representation = audio_adaptation
            .representations
            .first()
            .ok_or(DashError::InvalidManifest("No representation found".into()))?;

        // Extract codec info
        let codec = representation
            .codecs
            .clone()
            .or_else(|| audio_adaptation.codecs.clone())
            .unwrap_or_default();

        // Extract sample rate (it's a String in dash-mpd)
        let sample_rate: u32 = representation
            .audioSamplingRate
            .as_ref()
            .or(audio_adaptation.audioSamplingRate.as_ref())
            .and_then(|s| s.parse().ok())
            .unwrap_or(44100);

        // Get segment template - check representation first, then adaptation set
        let segment_template = representation
            .SegmentTemplate
            .as_ref()
            .or(audio_adaptation.SegmentTemplate.as_ref())
            .ok_or(DashError::InvalidManifest(
                "No SegmentTemplate found".into(),
            ))?;

        // Get initialization URL
        let init_url =
            segment_template
                .initialization
                .clone()
                .ok_or(DashError::InvalidManifest(
                    "No initialization URL in template".into(),
                ))?;

        // Get media URL template
        let media_template = segment_template
            .media
            .clone()
            .ok_or(DashError::InvalidManifest("No media URL template".into()))?;

        let start_number = segment_template.startNumber.unwrap_or(1);
        let timescale = segment_template.timescale.unwrap_or(1) as u64;

        // Build segment URLs from SegmentTimeline
        let mut segment_urls = Vec::new();

        if let Some(timeline) = &segment_template.SegmentTimeline {
            let mut segment_number = start_number;

            for s in &timeline.segments {
                // r attribute means repeat count (0 = play once, 1 = repeat once = play twice)
                let repeat_count = s.r.unwrap_or(0);

                for _ in 0..=repeat_count {
                    let url = media_template.replace("$Number$", &segment_number.to_string());
                    segment_urls.push(url);
                    segment_number += 1;
                }
            }
        } else if let Some(seg_duration) = segment_template.duration {
            // Calculate number of segments from duration
            let total_timescale_units = (duration * timescale as f64) as u64;
            let num_segments = (total_timescale_units / seg_duration as u64) + 1;

            for i in 0..num_segments {
                let segment_number = start_number + i;
                let url = media_template.replace("$Number$", &segment_number.to_string());
                segment_urls.push(url);
            }
        }

        if segment_urls.is_empty() {
            return Err(DashError::InvalidManifest(
                "Could not determine segment URLs".into(),
            ));
        }

        let audio_info = DashAudioInfo {
            codec,
            sample_rate,
            duration,
            segment_count: segment_urls.len(),
        };

        info!(
            "Parsed DASH manifest: {} segments, {:.2}s duration, {}Hz, codec={}",
            audio_info.segment_count, audio_info.duration, audio_info.sample_rate, audio_info.codec
        );

        Ok(Self {
            audio_info,
            init_url,
            segment_urls,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MPD: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2M53.588S">
  <Period id="0">
    <AdaptationSet id="0" contentType="audio" mimeType="audio/mp4">
      <Representation id="FLAC_HIRES,44100,24" codecs="flac" bandwidth="1641666" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://example.com/init.mp4"
          media="https://example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128" r="42"/>
            <S d="81728"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

    #[test]
    fn test_parse_manifest() {
        let manifest = DashManifest::parse(SAMPLE_MPD).unwrap();

        assert_eq!(manifest.audio_info.codec, "flac");
        assert_eq!(manifest.audio_info.sample_rate, 44100);

        // 43 segments with r=42 (repeat 42 times = 43 total) + 1 segment = 44
        assert_eq!(manifest.segment_urls.len(), 44);
    }

    #[test]
    fn test_segment_urls() {
        let manifest = DashManifest::parse(SAMPLE_MPD).unwrap();

        assert_eq!(manifest.segment_urls[0], "https://example.com/1.mp4");
        assert_eq!(manifest.segment_urls[43], "https://example.com/44.mp4");
    }

    #[test]
    fn test_init_url() {
        let manifest = DashManifest::parse(SAMPLE_MPD).unwrap();
        assert_eq!(manifest.init_url, "https://example.com/init.mp4");
    }
}
