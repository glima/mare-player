// SPDX-License-Identifier: MIT

//! Integration tests for `audio::decoder` public API.
//!
//! These tests exercise the public surface of `AudioDecoder`, `StreamingDecoder`,
//! `AudioFormat`, `DecoderError`, and `DownloadHandle` **without** requiring a
//! running audio output server.  Where HTTP is needed a tiny in-process TCP
//! server is spun up to serve canned responses.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use cosmic_applet_mare::audio::decoder::{
    AudioDecoder, AudioFormat, DecoderError, DecoderResult, StreamingDecoder,
};

// ─── Tiny valid MP3 fixture (440 Hz sine, 50 ms, mono, 32 kbps) ────────────
// Generated with:
//   ffmpeg -f lavfi -i "sine=frequency=440:duration=0.05" \
//          -ar 44100 -ac 1 -c:a libmp3lame -b:a 32k tiny.mp3
const TINY_MP3: &[u8] = &[
    0x49, 0x44, 0x33, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x22, 0x54, 0x53, 0x53, 0x45, 0x00, 0x00,
    0x00, 0x0e, 0x00, 0x00, 0x03, 0x4c, 0x61, 0x76, 0x66, 0x36, 0x31, 0x2e, 0x37, 0x2e, 0x31, 0x30,
    0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xfb, 0x40, 0xc0,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x49, 0x6e, 0x66, 0x6f, 0x00, 0x00, 0x00, 0x0f, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x01,
    0xef, 0x00, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93,
    0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93,
    0x93, 0x93, 0x93, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca,
    0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca,
    0xca, 0xca, 0xca, 0xca, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x4c, 0x61, 0x76, 0x63, 0x36, 0x31, 0x2e,
    0x31, 0x39, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24, 0x02,
    0xa3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xef, 0x28, 0xb6, 0x68, 0xcc, 0x00, 0x00, 0x00,
    0x00, 0x00, 0xff, 0xfb, 0x10, 0xc4, 0x00, 0x00, 0x04, 0x74, 0x13, 0x55, 0x54, 0x90, 0x80, 0x30,
    0xa6, 0x09, 0xaf, 0x37, 0x1a, 0x20, 0x02, 0x00, 0x01, 0xad, 0x39, 0x40, 0x00, 0x01, 0x59, 0x3a,
    0x3d, 0x50, 0x50, 0x08, 0x06, 0x09, 0x01, 0xf0, 0x7c, 0x1f, 0x07, 0xca, 0x02, 0x00, 0x80, 0x61,
    0x10, 0x7c, 0x1f, 0xd4, 0x08, 0x3b, 0x13, 0x87, 0xf8, 0x83, 0x70, 0x04, 0x93, 0xf6, 0xc0, 0x60,
    0x38, 0x1c, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x28, 0x89, 0x2a, 0x99, 0x14, 0x64, 0x08, 0xe9,
    0x02, 0x48, 0x16, 0xa3, 0xf7, 0x85, 0x01, 0xf0, 0x13, 0x1b, 0xf0, 0x22, 0x94, 0x2f, 0xa8, 0x1a,
    0x12, 0xfc, 0x24, 0x0d, 0x2a, 0x0a, 0x00, 0x18, 0x30, 0x00, 0xff, 0xfb, 0x12, 0xc4, 0x02, 0x83,
    0xc5, 0x58, 0x1d, 0x20, 0x1d, 0xe0, 0x00, 0x28, 0xa4, 0x83, 0xa4, 0x82, 0xbc, 0x00, 0x05, 0xcc,
    0x09, 0x00, 0xbc, 0x40, 0x04, 0x86, 0x00, 0xe0, 0x78, 0x67, 0xee, 0xf6, 0xa6, 0x63, 0x03, 0x96,
    0x61, 0xc4, 0x11, 0x26, 0x0c, 0x00, 0x7e, 0x60, 0x42, 0x06, 0x06, 0x05, 0x20, 0x4c, 0x60, 0x5e,
    0x03, 0xc5, 0x9a, 0xb4, 0x95, 0xf9, 0x48, 0xf3, 0x01, 0x30, 0x11, 0x30, 0x00, 0x03, 0x63, 0x03,
    0x60, 0x84, 0x33, 0x6e, 0x50, 0xd3, 0x2e, 0xb1, 0x77, 0x30, 0xbf, 0x07, 0xd3, 0x05, 0x90, 0x1d,
    0x30, 0x0b, 0x02, 0xd3, 0x02, 0x50, 0x1f, 0x30, 0x23, 0x01, 0xb4, 0x4f, 0x9f, 0x49, 0x03, 0x92,
    0x48, 0x00, 0x0a, 0xff, 0xfb, 0x10, 0xc4, 0x02, 0x80, 0x04, 0xb4, 0x43, 0x52, 0xb9, 0x92, 0x80,
    0x10, 0x97, 0x06, 0xa6, 0xeb, 0x98, 0x30, 0x04, 0x61, 0x11, 0xd2, 0xa1, 0x4c, 0x16, 0xe9, 0x9a,
    0xd1, 0x5c, 0xfa, 0x22, 0xaa, 0xf9, 0x12, 0xcc, 0xbb, 0xbf, 0x7f, 0x37, 0x96, 0x4f, 0xe0, 0x61,
    0x5f, 0xc7, 0x8b, 0x17, 0xc0, 0xc7, 0x7e, 0x15, 0x50, 0x0c, 0x5d, 0x85, 0xc0, 0x00, 0x00, 0x26,
    0x12, 0x84, 0x62, 0x73, 0xcc, 0x92, 0x41, 0xa8, 0x1d, 0x5e, 0x49, 0x12, 0x42, 0x95, 0x2d, 0x3c,
    0x94, 0x49, 0x14, 0x14, 0x05, 0x63, 0x18, 0x53, 0xbc, 0x4b, 0x74, 0xa8, 0x2d, 0xc4, 0xaa, 0x4c,
    0x41, 0x4d, 0x45, 0x33, 0x2e, 0x31, 0x30, 0x30, 0xaa, 0xaa, 0xaa,
];

// ─── Mock HTTP helpers ──────────────────────────────────────────────────────

/// Spawn a tiny single-request HTTP server on localhost that serves `body`
/// with the given `content_type`.  Returns `(url, port)`.
fn spawn_http_server(body: &'static [u8], content_type: &str) -> (String, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{}/audio", port);
    let ct = content_type.to_string();

    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut req = [0u8; 4096];
            let _ = stream.read(&mut req);
            let resp = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: {}\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n",
                ct,
                body.len(),
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });

    std::thread::sleep(Duration::from_millis(20));
    (url, port)
}

/// Spawn a tiny HTTP server that returns the given status code and no body.
fn spawn_http_error(status: u16) -> (String, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{}/audio", port);

    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut req = [0u8; 4096];
            let _ = stream.read(&mut req);
            let resp = format!(
                "HTTP/1.1 {} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                status,
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });

    std::thread::sleep(Duration::from_millis(20));
    (url, port)
}

/// Spawn a mock HTTP server that drip-feeds `body` one byte at a time
/// with a small delay between each byte, exercising the streaming buffer.
fn spawn_http_slow(body: &'static [u8], content_type: &str) -> (String, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{}/audio", port);
    let ct = content_type.to_string();

    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut req = [0u8; 4096];
            let _ = stream.read(&mut req);
            // Use chunked transfer encoding so we can drip-feed.
            let header = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: {}\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n",
                ct,
                body.len(),
            );
            let _ = stream.write_all(header.as_bytes());
            // Send in small chunks with a tiny delay to exercise streaming.
            let chunk_size = 64;
            for chunk in body.chunks(chunk_size) {
                let _ = stream.write_all(chunk);
                let _ = stream.flush();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    });

    std::thread::sleep(Duration::from_millis(20));
    (url, port)
}

// ===========================================================================
// AudioFormat
// ===========================================================================

mod audio_format {
    use super::*;

    #[test]
    fn construction_with_duration() {
        let f = AudioFormat {
            sample_rate: 44100,
            channels: 2,
            duration: Some(180.0),
        };
        assert_eq!(f.sample_rate, 44100);
        assert_eq!(f.channels, 2);
        assert_eq!(f.duration, Some(180.0));
    }

    #[test]
    fn construction_without_duration() {
        let f = AudioFormat {
            sample_rate: 96000,
            channels: 1,
            duration: None,
        };
        assert_eq!(f.sample_rate, 96000);
        assert_eq!(f.channels, 1);
        assert!(f.duration.is_none());
    }

    #[test]
    fn clone_preserves_all_fields() {
        let f = AudioFormat {
            sample_rate: 48000,
            channels: 6,
            duration: Some(300.5),
        };
        let f2 = f.clone();
        assert_eq!(f.sample_rate, f2.sample_rate);
        assert_eq!(f.channels, f2.channels);
        assert_eq!(f.duration, f2.duration);
    }

    #[test]
    fn debug_format_contains_values() {
        let f = AudioFormat {
            sample_rate: 192000,
            channels: 2,
            duration: Some(42.5),
        };
        let dbg = format!("{:?}", f);
        assert!(dbg.contains("192000"));
        assert!(dbg.contains("42.5"));
    }

    #[test]
    fn common_sample_rates() {
        for &rate in &[
            8000, 11025, 22050, 44100, 48000, 88200, 96000, 176400, 192000,
        ] {
            let f = AudioFormat {
                sample_rate: rate,
                channels: 2,
                duration: None,
            };
            assert_eq!(f.sample_rate, rate);
        }
    }

    #[test]
    fn common_channel_counts() {
        for ch in 1..=8 {
            let f = AudioFormat {
                sample_rate: 44100,
                channels: ch,
                duration: None,
            };
            assert_eq!(f.channels, ch);
        }
    }
}

// ===========================================================================
// DecoderError
// ===========================================================================

mod decoder_error {
    use super::*;

    #[test]
    fn display_open_error() {
        let e = DecoderError::OpenError("bad file".into());
        assert_eq!(e.to_string(), "Failed to open media: bad file");
    }

    #[test]
    fn display_no_audio_track() {
        let e = DecoderError::NoAudioTrack;
        assert_eq!(e.to_string(), "No audio track found");
    }

    #[test]
    fn display_decoder_init() {
        let e = DecoderError::DecoderInit("codec fail".into());
        assert_eq!(e.to_string(), "Decoder initialization failed: codec fail");
    }

    #[test]
    fn display_decode_error() {
        let e = DecoderError::DecodeError("corrupt".into());
        assert_eq!(e.to_string(), "Decoding error: corrupt");
    }

    #[test]
    fn display_seek_error() {
        let e = DecoderError::SeekError("past end".into());
        assert_eq!(e.to_string(), "Seek error: past end");
    }

    #[test]
    fn display_io_error() {
        let e = DecoderError::IoError("disk full".into());
        assert_eq!(e.to_string(), "IO error: disk full");
    }

    #[test]
    fn display_unsupported_format() {
        let e = DecoderError::UnsupportedFormat("opus".into());
        assert_eq!(e.to_string(), "Unsupported format: opus");
    }

    #[test]
    fn implements_std_error() {
        let e: Box<dyn std::error::Error> = Box::new(DecoderError::NoAudioTrack);
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn debug_format_contains_variant() {
        let e = DecoderError::OpenError("x".into());
        let dbg = format!("{:?}", e);
        assert!(dbg.contains("OpenError"));
    }

    #[test]
    fn all_variants_display_non_empty() {
        let variants: Vec<DecoderError> = vec![
            DecoderError::OpenError("a".into()),
            DecoderError::NoAudioTrack,
            DecoderError::DecoderInit("b".into()),
            DecoderError::DecodeError("c".into()),
            DecoderError::SeekError("d".into()),
            DecoderError::IoError("e".into()),
            DecoderError::UnsupportedFormat("f".into()),
        ];
        for v in variants {
            assert!(!v.to_string().is_empty());
        }
    }
}

// ===========================================================================
// DecoderResult type alias
// ===========================================================================

mod decoder_result {
    use super::*;

    #[test]
    fn ok_variant() {
        let r: DecoderResult<i32> = Ok(42);
        assert_eq!(r.unwrap(), 42);
    }

    #[test]
    fn err_variant() {
        let r: DecoderResult<i32> = Err(DecoderError::NoAudioTrack);
        assert!(r.is_err());
    }
}

// ===========================================================================
// AudioDecoder::from_bytes
// ===========================================================================

mod from_bytes {
    use super::*;

    #[test]
    fn valid_mp3_with_hint() {
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        assert_eq!(dec.format_info.sample_rate, 44100);
        assert!(dec.format_info.channels >= 1);
    }

    #[test]
    fn valid_mp3_without_hint() {
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), None).unwrap();
        assert_eq!(dec.format_info.sample_rate, 44100);
    }

    #[test]
    fn empty_data_errors() {
        let r = AudioDecoder::from_bytes(vec![], Some("mp3"));
        assert!(r.is_err());
    }

    #[test]
    fn garbage_data_errors() {
        let r = AudioDecoder::from_bytes(vec![0xDE, 0xAD, 0xBE, 0xEF], Some("mp3"));
        assert!(r.is_err());
    }

    #[test]
    fn one_byte_errors() {
        let r = AudioDecoder::from_bytes(vec![0xFF], None);
        assert!(r.is_err());
    }

    #[test]
    fn wrong_hint_no_panic() {
        // FLAC hint for MP3 data — should not panic regardless of outcome.
        let _ = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("flac"));
    }

    #[test]
    fn m4a_hint_no_panic() {
        let _ = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("m4a"));
    }
}

// ===========================================================================
// AudioDecoder::from_file
// ===========================================================================

mod from_file {
    use super::*;

    #[test]
    fn nonexistent_file() {
        let r = AudioDecoder::from_file("/tmp/__nonexistent_audio_12345.mp3");
        assert!(r.is_err());
    }

    #[test]
    fn empty_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let r = AudioDecoder::from_file(tmp.path());
        assert!(r.is_err());
    }

    #[test]
    fn valid_mp3_file() {
        let mut tmp = tempfile::NamedTempFile::with_suffix(".mp3").unwrap();
        tmp.write_all(TINY_MP3).unwrap();
        tmp.flush().unwrap();
        let dec = AudioDecoder::from_file(tmp.path()).unwrap();
        assert_eq!(dec.format_info.sample_rate, 44100);
    }

    #[test]
    fn file_with_no_extension() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(TINY_MP3).unwrap();
        tmp.flush().unwrap();
        // Probing should still work even without an extension hint.
        let r = AudioDecoder::from_file(tmp.path());
        // May or may not succeed — the important thing is no panic.
        let _ = r;
    }

    #[test]
    fn file_with_wrong_extension() {
        let mut tmp = tempfile::NamedTempFile::with_suffix(".flac").unwrap();
        tmp.write_all(TINY_MP3).unwrap();
        tmp.flush().unwrap();
        // Wrong extension, but probe should still detect MP3.
        let _ = AudioDecoder::from_file(tmp.path());
    }
}

// ===========================================================================
// AudioDecoder — decode_next / position
// ===========================================================================

mod decoding {
    use super::*;

    #[test]
    fn decode_next_returns_some() {
        let mut dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let first = dec.decode_next().unwrap();
        assert!(first.is_some());
        let samples = first.unwrap();
        assert!(!samples.is_empty());
    }

    #[test]
    fn decode_next_eventually_none() {
        let mut dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let mut packets = 0u32;
        loop {
            match dec.decode_next().unwrap() {
                Some(_) => packets += 1,
                None => break,
            }
            assert!(packets < 10_000, "too many packets — likely infinite loop");
        }
        assert!(packets >= 1);
    }

    #[test]
    fn format_info_accessible() {
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let fi = &dec.format_info;
        assert!(fi.sample_rate > 0);
        assert!(fi.channels >= 1);
    }
}

// ===========================================================================
// StreamingDecoder
// ===========================================================================

mod streaming_decoder {
    use super::*;

    #[test]
    fn from_decoder() {
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let mut sd = StreamingDecoder::from_decoder(dec);
        // Verify it constructed successfully and can decode samples.
        let mut buf = vec![0.0f32; 1024];
        let n = sd.fill_buffer(&mut buf).unwrap();
        assert!(n > 0);
    }

    #[test]
    fn fill_buffer_returns_samples() {
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let mut sd = StreamingDecoder::from_decoder(dec);
        let mut buf = vec![0.0f32; 4096];
        let n = sd.fill_buffer(&mut buf).unwrap();
        assert!(n > 0);
    }

    #[test]
    fn fill_buffer_to_eof() {
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let mut sd = StreamingDecoder::from_decoder(dec);
        let mut total = 0usize;
        loop {
            let mut buf = vec![0.0f32; 1024];
            let n = sd.fill_buffer(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            total += n;
            assert!(total < 10_000_000, "too many samples");
        }
        assert!(total > 0);
    }

    #[test]
    fn fill_buffer_small_output() {
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let mut sd = StreamingDecoder::from_decoder(dec);
        // Request only 4 samples at a time.
        let mut buf = vec![0.0f32; 4];
        let n = sd.fill_buffer(&mut buf).unwrap();
        assert_eq!(n, 4);
    }
}

// ===========================================================================
// AudioDecoder::from_url_streaming — with mock HTTP
// ===========================================================================

mod from_url_streaming {
    use super::*;

    #[test]
    fn valid_mp3() {
        let (url, _) = spawn_http_server(TINY_MP3, "audio/mpeg");
        let (dec, mut handle) = AudioDecoder::from_url_streaming(&url, None).unwrap();
        assert_eq!(dec.format_info.sample_rate, 44100);
        handle.abort();
    }

    #[test]
    fn http_error() {
        let (url, _) = spawn_http_error(404);
        let r = AudioDecoder::from_url_streaming(&url, None);
        assert!(r.is_err());
    }

    #[test]
    fn garbage_audio() {
        let (url, _) = spawn_http_server(
            b"this is not audio data at all, just garbage bytes for testing purposes here",
            "audio/mpeg",
        );
        let r = AudioDecoder::from_url_streaming(&url, None);
        assert!(r.is_err());
    }

    #[test]
    fn with_cache_path() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let cache = tmp_dir
            .path()
            .join("cached.mp3")
            .to_string_lossy()
            .to_string();
        let (url, _) = spawn_http_server(TINY_MP3, "audio/mpeg");
        let (_dec, mut handle) =
            AudioDecoder::from_url_streaming(&url, Some(cache.clone())).unwrap();

        // Wait for download to finish so the cache file is written.
        for _ in 0..100 {
            if handle.is_complete() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        handle.abort();
        assert!(
            std::path::Path::new(&cache).exists(),
            "cache file should have been written",
        );
        // Verify the cached file has the right size.
        let meta = std::fs::metadata(&cache).unwrap();
        assert_eq!(meta.len(), TINY_MP3.len() as u64);
    }

    #[test]
    fn download_handle_is_complete() {
        let (url, _) = spawn_http_server(TINY_MP3, "audio/mpeg");
        let (_dec, handle) = AudioDecoder::from_url_streaming(&url, None).unwrap();
        // The file is tiny, so download should complete very quickly.
        for _ in 0..100 {
            if handle.is_complete() {
                return; // pass
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("download did not complete in time");
    }

    #[test]
    fn download_handle_abort() {
        let (url, _) = spawn_http_server(TINY_MP3, "audio/mpeg");
        let (_dec, mut handle) = AudioDecoder::from_url_streaming(&url, None).unwrap();
        handle.abort();
        // Abort should be idempotent.
        handle.abort();
    }

    #[test]
    fn content_type_flac() {
        let (url, _) = spawn_http_server(TINY_MP3, "audio/flac");
        let _ = AudioDecoder::from_url_streaming(&url, None);
    }

    #[test]
    fn content_type_m4a() {
        let (url, _) = spawn_http_server(TINY_MP3, "audio/x-m4a");
        let _ = AudioDecoder::from_url_streaming(&url, None);
    }

    #[test]
    fn slow_drip_feed() {
        let (url, _) = spawn_http_slow(TINY_MP3, "audio/mpeg");
        let result = AudioDecoder::from_url_streaming(&url, None);
        // The slow server should still work because the streaming source
        // waits for data.
        assert!(result.is_ok(), "slow drip-feed failed: {:?}", result.err());
        let (_dec, mut handle) = result.unwrap();
        handle.abort();
    }

    #[test]
    fn decode_after_streaming() {
        let (url, _) = spawn_http_server(TINY_MP3, "audio/mpeg");
        let (mut dec, mut handle) = AudioDecoder::from_url_streaming(&url, None).unwrap();

        // Wait for full download.
        for _ in 0..100 {
            if handle.is_complete() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        // Decode a few packets to verify streaming works.
        let first = dec.decode_next().unwrap();
        assert!(
            first.is_some(),
            "expected some decoded audio from streaming source"
        );
        handle.abort();
    }
}

// ===========================================================================
// End-to-end: from_bytes → StreamingDecoder → fill_buffer
// ===========================================================================

mod end_to_end {
    use super::*;

    #[test]
    fn bytes_to_streaming_decoder_to_samples() {
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let mut sd = StreamingDecoder::from_decoder(dec);

        let mut all_samples = Vec::new();
        loop {
            let mut buf = vec![0.0f32; 512];
            let n = sd.fill_buffer(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            all_samples.extend_from_slice(&buf[..n]);
        }
        assert!(!all_samples.is_empty());
        // Verify samples are in a reasonable range (f32 audio is typically -1.0..1.0).
        for &s in &all_samples {
            assert!(s.abs() <= 2.0, "sample {} is out of expected range", s,);
        }
    }

    #[test]
    fn streaming_url_to_streaming_decoder() {
        let (url, _) = spawn_http_server(TINY_MP3, "audio/mpeg");
        let (dec, mut handle) = AudioDecoder::from_url_streaming(&url, None).unwrap();

        // Wait for download.
        for _ in 0..100 {
            if handle.is_complete() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let mut sd = StreamingDecoder::from_decoder(dec);
        let mut buf = vec![0.0f32; 4096];
        let n = sd.fill_buffer(&mut buf).unwrap();
        assert!(n > 0);
        handle.abort();
    }
}
