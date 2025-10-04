// SPDX-License-Identifier: MIT

//! Integration tests for the DASH manifest parsing module.
//!
//! Covers various manifest formats, sanitization, error handling,
//! edge cases (empty timelines, missing fields, malformed XML), and
//! segment URL generation for both SegmentTimeline and duration-based
//! approaches.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::audio::dash::{DashError, DashManifest};

// ===========================================================================
// Well-formed manifests
// ===========================================================================

mod valid_manifests {
    use super::*;

    /// Standard TIDAL HiRes FLAC manifest with SegmentTimeline.
    const TIDAL_HIRES_MPD: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
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
    fn parse_tidal_hires_manifest() {
        let manifest = DashManifest::parse(TIDAL_HIRES_MPD).unwrap();
        assert_eq!(manifest.audio_info.codec, "flac");
        assert_eq!(manifest.audio_info.sample_rate, 44100);
        // 43 segments (r=42 → repeat 42 = 43 total) + 1 = 44
        assert_eq!(manifest.audio_info.segment_count, 44);
        assert_eq!(manifest.segment_urls.len(), 44);
    }

    #[test]
    fn init_url_extracted() {
        let manifest = DashManifest::parse(TIDAL_HIRES_MPD).unwrap();
        assert_eq!(manifest.init_url, "https://example.com/init.mp4");
    }

    #[test]
    fn segment_urls_numbered_sequentially() {
        let manifest = DashManifest::parse(TIDAL_HIRES_MPD).unwrap();
        assert_eq!(manifest.segment_urls[0], "https://example.com/1.mp4");
        assert_eq!(manifest.segment_urls[1], "https://example.com/2.mp4");
        assert_eq!(manifest.segment_urls[43], "https://example.com/44.mp4");
    }

    #[test]
    fn duration_is_positive() {
        let manifest = DashManifest::parse(TIDAL_HIRES_MPD).unwrap();
        assert!(
            manifest.audio_info.duration > 170.0,
            "duration should be ~173s, got {}",
            manifest.audio_info.duration
        );
    }

    /// Manifest with a single segment (no repeat).
    #[test]
    fn single_segment() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT4S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="48000">
        <SegmentTemplate timescale="48000"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/seg-$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="192000"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.segment_urls.len(), 1);
        assert_eq!(
            manifest.segment_urls[0],
            "https://cdn.example.com/seg-1.mp4"
        );
        assert_eq!(manifest.audio_info.sample_rate, 48000);
        assert_eq!(manifest.audio_info.codec, "flac");
    }

    /// Manifest with multiple non-repeating S elements.
    #[test]
    fn multiple_s_elements_no_repeat() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT10S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="aac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/i.mp4"
          media="https://cdn.example.com/$Number$.m4s"
          startNumber="0">
          <SegmentTimeline>
            <S d="88200"/>
            <S d="88200"/>
            <S d="88200"/>
            <S d="44100"/>
            <S d="44100"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.segment_urls.len(), 5);
        assert_eq!(manifest.segment_urls[0], "https://cdn.example.com/0.m4s");
        assert_eq!(manifest.segment_urls[4], "https://cdn.example.com/4.m4s");
        assert_eq!(manifest.audio_info.codec, "aac");
    }

    /// Manifest with mixed repeat and non-repeat S elements.
    #[test]
    fn mixed_repeat_and_single_segments() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT30S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="mp3" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init"
          media="https://cdn.example.com/chunk_$Number$"
          startNumber="1">
          <SegmentTimeline>
            <S d="176400" r="4"/>
            <S d="88200"/>
            <S d="176400" r="1"/>
            <S d="44100"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        // r=4 → 5 segments, then 1, r=1 → 2 segments, then 1 = 5+1+2+1 = 9
        assert_eq!(manifest.segment_urls.len(), 9);
        assert_eq!(manifest.segment_urls[0], "https://cdn.example.com/chunk_1");
        assert_eq!(manifest.segment_urls[8], "https://cdn.example.com/chunk_9");
    }

    /// Manifest with startNumber > 1.
    #[test]
    fn custom_start_number() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="100">
          <SegmentTimeline>
            <S d="176128" r="2"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.segment_urls.len(), 3);
        assert_eq!(manifest.segment_urls[0], "https://cdn.example.com/100.mp4");
        assert_eq!(manifest.segment_urls[1], "https://cdn.example.com/101.mp4");
        assert_eq!(manifest.segment_urls[2], "https://cdn.example.com/102.mp4");
    }

    /// Manifest with codec info on the AdaptationSet instead of Representation.
    #[test]
    fn codec_on_adaptation_set() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT3S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4" codecs="flac" audioSamplingRate="96000">
      <Representation id="rep1">
        <SegmentTemplate timescale="96000"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="192000" r="0"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.audio_info.codec, "flac");
        assert_eq!(manifest.audio_info.sample_rate, 96000);
        assert_eq!(manifest.segment_urls.len(), 1);
    }

    /// Manifest where SegmentTemplate is on the AdaptationSet, not the Representation.
    #[test]
    fn segment_template_on_adaptation_set() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT10S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <SegmentTemplate timescale="44100"
        initialization="https://cdn.example.com/init.mp4"
        media="https://cdn.example.com/s$Number$.mp4"
        startNumber="1">
        <SegmentTimeline>
          <S d="176128" r="1"/>
        </SegmentTimeline>
      </SegmentTemplate>
      <Representation codecs="flac" audioSamplingRate="44100"/>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.segment_urls.len(), 2);
        assert_eq!(manifest.segment_urls[0], "https://cdn.example.com/s1.mp4");
        assert_eq!(manifest.segment_urls[1], "https://cdn.example.com/s2.mp4");
    }

    /// Manifest with no explicit mediaPresentationDuration.
    #[test]
    fn no_media_presentation_duration() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128" r="9"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        // Duration defaults to 0.0 when not specified
        assert_eq!(manifest.audio_info.duration, 0.0);
        // Segments should still parse fine
        assert_eq!(manifest.segment_urls.len(), 10);
    }

    /// Manifest with r="0" (explicit no-repeat, same as omitting r).
    #[test]
    fn explicit_r_zero() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT8S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128" r="0"/>
            <S d="176128" r="0"/>
            <S d="176128" r="0"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        // r=0 means play once (no repeat), so 3 S elements = 3 segments
        assert_eq!(manifest.segment_urls.len(), 3);
    }

    /// Very large repeat count.
    #[test]
    fn large_repeat_count() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT1H">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128" r="999"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        // r=999 → 1000 segments
        assert_eq!(manifest.segment_urls.len(), 1000);
        assert_eq!(manifest.segment_urls[0], "https://cdn.example.com/1.mp4");
        assert_eq!(
            manifest.segment_urls[999],
            "https://cdn.example.com/1000.mp4"
        );
    }
}

// ===========================================================================
// Sanitization
// ===========================================================================

mod sanitization {
    use super::*;

    /// Manifest with group="main" (string) — should be sanitized to group="0".
    #[test]
    fn sanitize_group_main() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet id="0" group="main" contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128" r="1"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        // Should not error — the sanitizer should fix group="main" → group="0"
        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.segment_urls.len(), 2);
    }

    /// Manifest with group="audio" (another string value).
    #[test]
    fn sanitize_group_audio() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT3S">
  <Period id="0">
    <AdaptationSet id="0" group="audio" contentType="audio" mimeType="audio/mp4">
      <Representation codecs="aac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="88200"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.segment_urls.len(), 1);
    }

    /// Manifest with bandwidth="unknown" (string) — should be sanitized.
    #[test]
    fn sanitize_bandwidth_string() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" bandwidth="unknown" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.segment_urls.len(), 1);
    }

    /// Manifest with numeric group (should NOT be sanitized).
    #[test]
    fn numeric_group_not_sanitized() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet id="0" group="1" contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128" r="0"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.segment_urls.len(), 1);
    }
}

// ===========================================================================
// Error cases
// ===========================================================================

mod error_cases {
    use super::*;

    #[test]
    fn empty_string_is_parse_error() {
        let result = DashManifest::parse("");
        assert!(result.is_err());
        match result.unwrap_err() {
            DashError::ParseError(_) => {} // expected
            other => panic!("Expected ParseError, got {:?}", other),
        }
    }

    #[test]
    fn garbage_xml_is_parse_error() {
        let result = DashManifest::parse("not xml at all");
        assert!(result.is_err());
    }

    #[test]
    fn valid_xml_but_not_mpd() {
        let xml = r#"<?xml version='1.0'?><root><child/></root>"#;
        let result = DashManifest::parse(xml);
        assert!(result.is_err());
    }

    #[test]
    fn mpd_with_no_periods() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
</MPD>"#;

        let result = DashManifest::parse(mpd);
        assert!(result.is_err());
        match result.unwrap_err() {
            DashError::InvalidManifest(msg) => {
                assert!(
                    msg.contains("No periods"),
                    "Expected 'No periods' message, got: {}",
                    msg
                );
            }
            other => panic!("Expected InvalidManifest, got {:?}", other),
        }
    }

    #[test]
    fn period_with_no_audio_adaptation_set() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="video" mimeType="video/mp4">
      <Representation codecs="avc1" width="1920" height="1080">
        <SegmentTemplate timescale="90000"
          initialization="https://cdn.example.com/video_init.mp4"
          media="https://cdn.example.com/video_$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="180000" r="2"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let result = DashManifest::parse(mpd);
        assert!(result.is_err());
        match result.unwrap_err() {
            DashError::NoAudioTrack => {} // expected
            other => panic!("Expected NoAudioTrack, got {:?}", other),
        }
    }

    #[test]
    fn audio_adaptation_with_no_representation() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
    </AdaptationSet>
  </Period>
</MPD>"#;

        let result = DashManifest::parse(mpd);
        assert!(result.is_err());
        match result.unwrap_err() {
            DashError::InvalidManifest(msg) => {
                assert!(
                    msg.contains("No representation"),
                    "Expected 'No representation' message, got: {}",
                    msg
                );
            }
            other => panic!("Expected InvalidManifest, got {:?}", other),
        }
    }

    #[test]
    fn representation_with_no_segment_template() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100"/>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let result = DashManifest::parse(mpd);
        assert!(result.is_err());
        match result.unwrap_err() {
            DashError::InvalidManifest(msg) => {
                assert!(
                    msg.contains("SegmentTemplate"),
                    "Expected SegmentTemplate error, got: {}",
                    msg
                );
            }
            other => panic!("Expected InvalidManifest, got {:?}", other),
        }
    }
}

// ===========================================================================
// DashError Display / Error trait
// ===========================================================================

mod error_display {
    use super::*;

    #[test]
    fn parse_error_display() {
        let err = DashError::ParseError("bad xml".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("Parse error"));
        assert!(msg.contains("bad xml"));
    }

    #[test]
    fn no_audio_track_display() {
        let err = DashError::NoAudioTrack;
        let msg = format!("{}", err);
        assert!(msg.contains("No audio track"));
    }

    #[test]
    fn invalid_manifest_display() {
        let err = DashError::InvalidManifest("missing field".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("Invalid manifest"));
        assert!(msg.contains("missing field"));
    }

    #[test]
    fn error_trait_source_is_none() {
        let err = DashError::NoAudioTrack;
        // std::error::Error is implemented
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn debug_format() {
        let err = DashError::ParseError("test".to_string());
        let debug = format!("{:?}", err);
        assert!(debug.contains("ParseError"));
        assert!(debug.contains("test"));
    }
}

// ===========================================================================
// DashAudioInfo
// ===========================================================================

mod audio_info {
    use super::*;

    #[test]
    fn codec_extraction_flac() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.audio_info.codec, "flac");
    }

    #[test]
    fn codec_extraction_aac() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="mp4a.40.2" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="88200"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.audio_info.codec, "mp4a.40.2");
    }

    #[test]
    fn sample_rate_fallback_to_default() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        // No audioSamplingRate specified → defaults to 44100
        assert_eq!(manifest.audio_info.sample_rate, 44100);
    }

    #[test]
    fn sample_rate_96khz() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="96000">
        <SegmentTemplate timescale="96000"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="192000"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.audio_info.sample_rate, 96000);
    }

    #[test]
    fn segment_count_matches_urls() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT30S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128" r="6"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(
            manifest.audio_info.segment_count,
            manifest.segment_urls.len()
        );
        assert_eq!(manifest.audio_info.segment_count, 7);
    }
}

// ===========================================================================
// Audio detection via mimeType (not contentType)
// ===========================================================================

mod mime_type_detection {
    use super::*;

    /// Detect audio via mimeType when contentType is absent.
    #[test]
    fn detect_audio_by_mimetype_only() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.segment_urls.len(), 1);
    }

    /// Audio/webm mimeType.
    #[test]
    fn detect_audio_webm() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet mimeType="audio/webm">
      <Representation codecs="opus" audioSamplingRate="48000">
        <SegmentTemplate timescale="48000"
          initialization="https://cdn.example.com/init.webm"
          media="https://cdn.example.com/$Number$.webm"
          startNumber="1">
          <SegmentTimeline>
            <S d="96000"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.audio_info.codec, "opus");
        assert_eq!(manifest.audio_info.sample_rate, 48000);
    }
}

// ===========================================================================
// Determinism / idempotency
// ===========================================================================

mod determinism {
    use super::*;

    const MPD: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT1M">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128" r="14"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

    #[test]
    fn parsing_is_deterministic() {
        let m1 = DashManifest::parse(MPD).unwrap();
        let m2 = DashManifest::parse(MPD).unwrap();

        assert_eq!(m1.audio_info.codec, m2.audio_info.codec);
        assert_eq!(m1.audio_info.sample_rate, m2.audio_info.sample_rate);
        assert_eq!(m1.audio_info.segment_count, m2.audio_info.segment_count);
        assert_eq!(m1.init_url, m2.init_url);
        assert_eq!(m1.segment_urls, m2.segment_urls);
    }

    #[test]
    fn parse_many_times() {
        for _ in 0..100 {
            let manifest = DashManifest::parse(MPD).unwrap();
            assert_eq!(manifest.segment_urls.len(), 15);
        }
    }
}

// ===========================================================================
// DashManifest struct fields
// ===========================================================================

mod manifest_struct {
    use super::*;

    #[test]
    fn debug_format() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        let debug = format!("{:?}", manifest);
        assert!(debug.contains("DashManifest"));
        assert!(debug.contains("audio_info"));
        assert!(debug.contains("init_url"));
        assert!(debug.contains("segment_urls"));
    }

    #[test]
    fn audio_info_debug_format() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        let info_debug = format!("{:?}", manifest.audio_info);
        assert!(info_debug.contains("DashAudioInfo"));
        assert!(info_debug.contains("flac"));
        assert!(info_debug.contains("44100"));
    }

    #[test]
    fn audio_info_clone() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="48000">
        <SegmentTemplate timescale="48000"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="192000" r="2"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        let info = manifest.audio_info.clone();
        assert_eq!(info.codec, "flac");
        assert_eq!(info.sample_rate, 48000);
        assert_eq!(info.segment_count, 3);
    }
}

// ===========================================================================
// Edge cases with whitespace and encoding
// ===========================================================================

mod whitespace_and_encoding {
    use super::*;

    /// Manifest with extra whitespace and newlines.
    #[test]
    fn extra_whitespace() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>

<MPD   xmlns="urn:mpeg:dash:schema:mpd:2011"
       type="static"
       mediaPresentationDuration="PT5S"  >

  <Period   id="0"  >
    <AdaptationSet   contentType="audio"   mimeType="audio/mp4"  >
      <Representation   codecs="flac"   audioSamplingRate="44100"  >
        <SegmentTemplate   timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1"  >
          <SegmentTimeline>
            <S   d="176128"   r="1"  />
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>

</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.segment_urls.len(), 2);
    }

    /// Manifest with UTF-8 BOM (byte order mark).
    #[test]
    fn utf8_bom_prefix() {
        // Prepend UTF-8 BOM: \xEF\xBB\xBF
        let mpd_body = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period id="0">
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation codecs="flac" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://cdn.example.com/init.mp4"
          media="https://cdn.example.com/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let mut mpd_with_bom = String::from("\u{FEFF}");
        mpd_with_bom.push_str(mpd_body);

        // This may or may not parse successfully depending on the XML parser's
        // BOM handling. We just assert it doesn't panic.
        let _result = DashManifest::parse(&mpd_with_bom);
    }
}

// ===========================================================================
// Realistic TIDAL manifests
// ===========================================================================

mod realistic {
    use super::*;

    /// Simulate a realistic short TIDAL track (~ 30 seconds, FLAC).
    #[test]
    fn short_tidal_track() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT30.5S">
  <Period id="0">
    <AdaptationSet id="0" contentType="audio" mimeType="audio/mp4">
      <Representation id="FLAC,44100,16" codecs="flac" bandwidth="850000" audioSamplingRate="44100">
        <SegmentTemplate timescale="44100"
          initialization="https://sp-pr-cf.audio.tidal.com/mediatracks/CAEaJAgBEAIYASoEZmxhYzIECAAQADoCd8/init.mp4"
          media="https://sp-pr-cf.audio.tidal.com/mediatracks/CAEaJAgBEAIYASoEZmxhYzIECAAQADoCd8/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="176128" r="6"/>
            <S d="114688"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.audio_info.codec, "flac");
        assert_eq!(manifest.audio_info.sample_rate, 44100);
        // 7 segments (r=6) + 1 = 8
        assert_eq!(manifest.segment_urls.len(), 8);
        assert!(manifest.audio_info.duration > 30.0);
        assert!(manifest.init_url.contains("tidal.com"));
        assert!(manifest.segment_urls[0].contains("tidal.com"));
    }

    /// Simulate a realistic long TIDAL track (~ 7 minutes, HiRes FLAC).
    #[test]
    fn long_hires_track() {
        let mpd = r#"<?xml version='1.0' encoding='UTF-8'?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT7M12.345S">
  <Period id="0">
    <AdaptationSet id="0" contentType="audio" mimeType="audio/mp4">
      <Representation id="FLAC_HIRES,96000,24" codecs="flac" bandwidth="3200000" audioSamplingRate="96000">
        <SegmentTemplate timescale="96000"
          initialization="https://sp-pr-cf.audio.tidal.com/hires/init.mp4"
          media="https://sp-pr-cf.audio.tidal.com/hires/$Number$.mp4"
          startNumber="1">
          <SegmentTimeline>
            <S d="384000" r="107"/>
            <S d="192000"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

        let manifest = DashManifest::parse(mpd).unwrap();
        assert_eq!(manifest.audio_info.codec, "flac");
        assert_eq!(manifest.audio_info.sample_rate, 96000);
        // 108 segments (r=107) + 1 = 109
        assert_eq!(manifest.segment_urls.len(), 109);
        assert!(manifest.audio_info.duration > 430.0); // 7*60 + 12 = 432
    }
}
