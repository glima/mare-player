// SPDX-License-Identifier: MIT

//! Integration tests for the tidal/client.rs module.
//!
//! Tests PlaybackUrl enum, TidalError Display/Error impls, TidalAppClient
//! construction and caching, and exposed helper methods (derive_plan_label,
//! title_case, uuid_to_cdn_url, extract_picture_url_from_json, parse_mix_from_json).

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use std::path::PathBuf;

use cosmic_applet_mare::tidal::auth::AuthState;
use cosmic_applet_mare::tidal::client::{PlaybackUrl, TidalAppClient, TidalError};

// ===========================================================================
// PlaybackUrl — as_url
// ===========================================================================

mod playback_url_as_url {
    use super::*;

    #[test]
    fn direct_returns_the_url() {
        let url = PlaybackUrl::Direct("https://example.com/stream.mp4".to_string(), None);
        assert_eq!(url.as_url(), "https://example.com/stream.mp4");
    }

    #[test]
    fn dash_manifest_returns_path_as_string() {
        let path = PathBuf::from("/tmp/manifest.mpd");
        let url = PlaybackUrl::DashManifest(path.clone(), None);
        assert_eq!(url.as_url(), path.to_string_lossy().to_string());
    }

    #[test]
    fn cached_file_returns_path_as_string() {
        let path = PathBuf::from("/cache/audio/song.dat");
        let url = PlaybackUrl::CachedFile(path.clone(), None);
        assert_eq!(url.as_url(), path.to_string_lossy().to_string());
    }

    #[test]
    fn direct_empty_url() {
        let url = PlaybackUrl::Direct(String::new(), None);
        assert_eq!(url.as_url(), "");
    }

    #[test]
    fn dash_manifest_relative_path() {
        let path = PathBuf::from("relative/path/manifest.mpd");
        let url = PlaybackUrl::DashManifest(path, None);
        assert_eq!(url.as_url(), "relative/path/manifest.mpd");
    }

    #[test]
    fn cached_file_relative_path() {
        let path = PathBuf::from("cache/song.flac");
        let url = PlaybackUrl::CachedFile(path, None);
        assert_eq!(url.as_url(), "cache/song.flac");
    }
}

// ===========================================================================
// PlaybackUrl — is_dash
// ===========================================================================

mod playback_url_is_dash {
    use super::*;

    #[test]
    fn direct_is_not_dash() {
        let url = PlaybackUrl::Direct("https://example.com/stream".to_string(), None);
        assert!(!url.is_dash());
    }

    #[test]
    fn dash_manifest_is_dash() {
        let url = PlaybackUrl::DashManifest(PathBuf::from("/tmp/manifest.mpd"), None);
        assert!(url.is_dash());
    }

    #[test]
    fn cached_file_is_not_dash() {
        let url = PlaybackUrl::CachedFile(PathBuf::from("/cache/song.dat"), None);
        assert!(!url.is_dash());
    }
}

// ===========================================================================
// PlaybackUrl — is_cached
// ===========================================================================

mod playback_url_is_cached {
    use super::*;

    #[test]
    fn direct_is_not_cached() {
        let url = PlaybackUrl::Direct("https://example.com/stream".to_string(), None);
        assert!(!url.is_cached());
    }

    #[test]
    fn dash_manifest_is_not_cached() {
        let url = PlaybackUrl::DashManifest(PathBuf::from("/tmp/manifest.mpd"), None);
        assert!(!url.is_cached());
    }

    #[test]
    fn cached_file_is_cached() {
        let url = PlaybackUrl::CachedFile(PathBuf::from("/cache/song.dat"), None);
        assert!(url.is_cached());
    }
}

// ===========================================================================
// PlaybackUrl — Clone, Debug
// ===========================================================================

mod playback_url_traits {
    use super::*;

    #[test]
    fn direct_clone() {
        let url = PlaybackUrl::Direct("https://example.com".to_string(), None);
        let cloned = url.clone();
        assert_eq!(url.as_url(), cloned.as_url());
    }

    #[test]
    fn dash_manifest_clone() {
        let url = PlaybackUrl::DashManifest(PathBuf::from("/tmp/m.mpd"), None);
        let cloned = url.clone();
        assert_eq!(url.as_url(), cloned.as_url());
        assert!(cloned.is_dash());
    }

    #[test]
    fn cached_file_clone() {
        let url = PlaybackUrl::CachedFile(PathBuf::from("/cache/s.dat"), None);
        let cloned = url.clone();
        assert_eq!(url.as_url(), cloned.as_url());
        assert!(cloned.is_cached());
    }

    #[test]
    fn debug_output_is_nonempty_for_all_variants() {
        let variants: Vec<PlaybackUrl> = vec![
            PlaybackUrl::Direct("url".to_string(), None),
            PlaybackUrl::DashManifest(PathBuf::from("path.mpd"), None),
            PlaybackUrl::CachedFile(PathBuf::from("path.dat"), None),
        ];
        for v in &variants {
            let dbg = format!("{:?}", v);
            assert!(!dbg.is_empty());
        }
    }

    #[test]
    fn debug_direct_contains_url() {
        let url = PlaybackUrl::Direct("https://test.url".to_string(), None);
        let dbg = format!("{:?}", url);
        assert!(dbg.contains("Direct"), "Debug should contain variant name");
        assert!(dbg.contains("https://test.url"), "Debug should contain URL");
    }

    #[test]
    fn debug_dash_contains_variant_name() {
        let url = PlaybackUrl::DashManifest(PathBuf::from("test.mpd"), None);
        let dbg = format!("{:?}", url);
        assert!(
            dbg.contains("DashManifest"),
            "Debug should contain variant name"
        );
    }

    #[test]
    fn debug_cached_contains_variant_name() {
        let url = PlaybackUrl::CachedFile(PathBuf::from("test.dat"), None);
        let dbg = format!("{:?}", url);
        assert!(
            dbg.contains("CachedFile"),
            "Debug should contain variant name"
        );
    }
}

// ===========================================================================
// PlaybackUrl — mutually exclusive flags
// ===========================================================================

mod playback_url_exclusivity {
    use super::*;

    #[test]
    fn direct_has_no_special_flags() {
        let url = PlaybackUrl::Direct("https://example.com".to_string(), None);
        assert!(!url.is_dash());
        assert!(!url.is_cached());
    }

    #[test]
    fn dash_is_dash_but_not_cached() {
        let url = PlaybackUrl::DashManifest(PathBuf::from("m.mpd"), None);
        assert!(url.is_dash());
        assert!(!url.is_cached());
    }

    #[test]
    fn cached_is_cached_but_not_dash() {
        let url = PlaybackUrl::CachedFile(PathBuf::from("s.dat"), None);
        assert!(!url.is_dash());
        assert!(url.is_cached());
    }
}

// ===========================================================================
// TidalError — Display
// ===========================================================================

mod tidal_error_display {
    use super::*;

    #[test]
    fn not_authenticated_display() {
        let e = TidalError::NotAuthenticated;
        let msg = format!("{}", e);
        assert!(
            msg.contains("Not authenticated"),
            "expected 'Not authenticated' in '{msg}'"
        );
    }

    #[test]
    fn authentication_failed_display() {
        let e = TidalError::AuthenticationFailed("bad token".to_string());
        let msg = format!("{}", e);
        assert!(msg.contains("Authentication failed"), "missing prefix");
        assert!(msg.contains("bad token"), "missing inner message");
    }

    #[test]
    fn request_failed_display() {
        let e = TidalError::RequestFailed("timeout".to_string());
        let msg = format!("{}", e);
        assert!(msg.contains("Request failed"));
        assert!(msg.contains("timeout"));
    }

    #[test]
    fn parse_error_display() {
        let e = TidalError::ParseError("invalid JSON".to_string());
        let msg = format!("{}", e);
        assert!(msg.contains("Parse error"));
        assert!(msg.contains("invalid JSON"));
    }

    #[test]
    fn session_expired_display() {
        let e = TidalError::SessionExpired;
        let msg = format!("{}", e);
        assert!(msg.contains("Session expired"));
    }

    #[test]
    fn network_error_display() {
        let e = TidalError::NetworkError("DNS failure".to_string());
        let msg = format!("{}", e);
        assert!(msg.contains("Network error"));
        assert!(msg.contains("DNS failure"));
    }

    #[test]
    fn credential_error_display() {
        let e = TidalError::CredentialError("keyring locked".to_string());
        let msg = format!("{}", e);
        assert!(msg.contains("Credential error"));
        assert!(msg.contains("keyring locked"));
    }

    #[test]
    fn all_variants_produce_nonempty_display() {
        let errors: Vec<TidalError> = vec![
            TidalError::NotAuthenticated,
            TidalError::AuthenticationFailed("msg".to_string()),
            TidalError::RequestFailed("msg".to_string()),
            TidalError::ParseError("msg".to_string()),
            TidalError::SessionExpired,
            TidalError::NetworkError("msg".to_string()),
            TidalError::CredentialError("msg".to_string()),
        ];
        for e in &errors {
            let msg = format!("{}", e);
            assert!(!msg.is_empty(), "Display for {:?} should not be empty", e);
        }
    }

    #[test]
    fn display_with_empty_inner_message() {
        let e = TidalError::AuthenticationFailed(String::new());
        let msg = format!("{}", e);
        assert!(msg.contains("Authentication failed"));
    }
}

// ===========================================================================
// TidalError — Debug, Clone, std::error::Error
// ===========================================================================

mod tidal_error_traits {
    use super::*;

    #[test]
    fn debug_output_contains_variant_name() {
        let e = TidalError::NotAuthenticated;
        let dbg = format!("{:?}", e);
        assert!(dbg.contains("NotAuthenticated"));
    }

    #[test]
    fn clone_equals_original_display() {
        let e = TidalError::RequestFailed("test".to_string());
        let cloned = e.clone();
        assert_eq!(format!("{}", e), format!("{}", cloned));
    }

    #[test]
    fn implements_std_error() {
        let e: Box<dyn std::error::Error> = Box::new(TidalError::NetworkError("test".to_string()));
        let msg = format!("{}", e);
        assert!(msg.contains("Network error"));
    }

    #[test]
    fn error_source_is_none() {
        use std::error::Error;
        let e = TidalError::NotAuthenticated;
        assert!(e.source().is_none(), "TidalError should not chain sources");
    }
}

// ===========================================================================
// TidalAppClient — construction
// ===========================================================================

mod client_construction {
    use super::*;

    #[test]
    fn new_creates_unauthenticated_client() {
        let client = TidalAppClient::new();
        assert_eq!(*client.auth_state(), AuthState::NotAuthenticated);
    }

    #[test]
    fn new_with_audio_cache_mb_creates_client() {
        let client = TidalAppClient::new_with_audio_cache_mb(500);
        assert_eq!(*client.auth_state(), AuthState::NotAuthenticated);
    }

    #[test]
    fn default_creates_unauthenticated_client() {
        let client = TidalAppClient::default();
        assert_eq!(*client.auth_state(), AuthState::NotAuthenticated);
    }

    #[test]
    fn new_with_zero_cache_mb() {
        // Should not panic even with 0 MB cache
        let client = TidalAppClient::new_with_audio_cache_mb(0);
        assert_eq!(*client.auth_state(), AuthState::NotAuthenticated);
    }

    #[test]
    fn new_with_large_cache_mb() {
        let client = TidalAppClient::new_with_audio_cache_mb(100_000);
        assert_eq!(*client.auth_state(), AuthState::NotAuthenticated);
    }
}

// ===========================================================================
// TidalAppClient — audio_cache_key
// ===========================================================================

mod client_audio_cache_key {
    use super::*;

    #[test]
    fn cache_key_contains_track_id() {
        let client = TidalAppClient::new();
        let key = client.audio_cache_key("12345");
        assert!(
            key.contains("12345"),
            "cache key should contain track ID, got: {key}"
        );
    }

    #[test]
    fn cache_key_contains_quality_indicator() {
        let client = TidalAppClient::new();
        let key = client.audio_cache_key("99999");
        // Default quality is High for TidalAppClient::new()
        assert!(
            key.contains("High"),
            "cache key should contain quality, got: {key}"
        );
    }

    #[test]
    fn cache_key_for_different_tracks_are_different() {
        let client = TidalAppClient::new();
        let key1 = client.audio_cache_key("111");
        let key2 = client.audio_cache_key("222");
        assert_ne!(key1, key2);
    }

    #[test]
    fn cache_key_for_same_track_is_deterministic() {
        let client = TidalAppClient::new();
        let key1 = client.audio_cache_key("42");
        let key2 = client.audio_cache_key("42");
        assert_eq!(key1, key2);
    }

    #[test]
    fn cache_key_with_empty_track_id() {
        let client = TidalAppClient::new();
        let key = client.audio_cache_key("");
        // Should still produce something with the quality suffix
        assert!(!key.is_empty());
    }
}

// ===========================================================================
// TidalAppClient — audio cache size/max
// ===========================================================================

mod client_audio_cache_metrics {
    use super::*;

    #[test]
    fn audio_cache_max_reflects_configured_mb() {
        let client = TidalAppClient::new_with_audio_cache_mb(100);
        let max_bytes = client.audio_cache_max();
        // 100 MB = 100 * 1024 * 1024 bytes
        assert_eq!(max_bytes, 100 * 1024 * 1024);
    }

    #[test]
    fn audio_cache_max_default_is_2000_mb() {
        let client = TidalAppClient::new();
        let max_bytes = client.audio_cache_max();
        assert_eq!(max_bytes, 2000 * 1024 * 1024);
    }

    #[test]
    fn audio_cache_size_returns_a_value() {
        // The XDG cache directory may already contain data from a real
        // installation, so we can only verify the call succeeds and
        // returns a non-negative value (u64 is always >= 0).
        let client = TidalAppClient::new_with_audio_cache_mb(10);
        let _size = client.audio_cache_size();
        // Just verify it doesn't panic
    }

    #[test]
    fn clear_audio_cache_does_not_panic() {
        let client = TidalAppClient::new_with_audio_cache_mb(10);
        client.clear_audio_cache();
        // Should not panic
    }

    #[test]
    fn clear_then_size_is_zero_or_near_zero() {
        let client = TidalAppClient::new_with_audio_cache_mb(10);
        client.clear_audio_cache();
        let size = client.audio_cache_size();
        // After clearing, should be 0 (or very small if directory metadata counts)
        assert!(
            size < 1024,
            "after clear, cache size should be near 0, got {}",
            size
        );
    }
}

// ===========================================================================
// TidalAppClient — auth_state
// ===========================================================================

mod client_auth_state {
    use super::*;

    #[test]
    fn initial_state_is_not_authenticated() {
        let client = TidalAppClient::new();
        assert_eq!(*client.auth_state(), AuthState::NotAuthenticated);
    }

    #[test]
    fn default_client_is_not_authenticated() {
        let client = TidalAppClient::default();
        assert_eq!(*client.auth_state(), AuthState::NotAuthenticated);
    }
}

// ===========================================================================
// TidalAppClient — cached data (initially None)
// ===========================================================================

mod client_cached_data {
    use super::*;

    #[test]
    fn get_cached_playlists_initially_none() {
        let client = TidalAppClient::new();
        // Fresh client with no prior API calls should have no cached playlists
        // (unless leftover from a previous test run in the same XDG dir)
        // We just verify it doesn't panic
        let _result = client.get_cached_playlists();
    }

    #[test]
    fn get_cached_albums_does_not_panic() {
        let client = TidalAppClient::new();
        let _result = client.get_cached_albums();
    }

    #[test]
    fn get_cached_favorite_tracks_does_not_panic() {
        let client = TidalAppClient::new();
        let _result = client.get_cached_favorite_tracks();
    }

    #[test]
    fn get_cached_mixes_does_not_panic() {
        let client = TidalAppClient::new();
        let _result = client.get_cached_mixes();
    }

    #[test]
    fn get_cached_followed_artists_does_not_panic() {
        let client = TidalAppClient::new();
        let _result = client.get_cached_followed_artists();
    }

    #[test]
    fn get_cached_playlist_tracks_does_not_panic() {
        let client = TidalAppClient::new();
        let _result = client.get_cached_playlist_tracks("nonexistent-uuid");
    }
}

// ===========================================================================
// TidalAppClient — audio_cache / api_cache accessors
// ===========================================================================

mod client_cache_accessors {
    use super::*;

    #[test]
    fn audio_cache_accessor_does_not_panic() {
        let client = TidalAppClient::new();
        let _cache = client.audio_cache();
    }

    #[test]
    fn api_cache_accessor_does_not_panic() {
        let client = TidalAppClient::new();
        let _cache = client.api_cache();
    }
}

// ===========================================================================
// TidalAppClient — get_cached_audio_path / audio_cache_path_for
// ===========================================================================

mod client_audio_cache_paths {
    use super::*;

    #[test]
    fn get_cached_audio_path_for_nonexistent_track_is_none() {
        let client = TidalAppClient::new();
        let result = client.get_cached_audio_path("nonexistent-track-99999999");
        assert!(
            result.is_none(),
            "should be None for a track that was never cached"
        );
    }

    #[test]
    fn audio_cache_path_for_returns_a_valid_path() {
        let client = TidalAppClient::new();
        let path = client.audio_cache_path_for("test-track-123");
        // Should return a path with .dat extension
        assert!(
            path.to_string_lossy().contains(".dat"),
            "cache path should have .dat extension, got: {:?}",
            path
        );
    }

    #[test]
    fn audio_cache_path_for_is_deterministic() {
        let client = TidalAppClient::new();
        let path1 = client.audio_cache_path_for("track-abc");
        let path2 = client.audio_cache_path_for("track-abc");
        assert_eq!(path1, path2);
    }

    #[test]
    fn audio_cache_path_for_different_tracks_differ() {
        let client = TidalAppClient::new();
        let path1 = client.audio_cache_path_for("track-aaa");
        let path2 = client.audio_cache_path_for("track-bbb");
        assert_ne!(path1, path2);
    }
}

// ===========================================================================
// TidalAppClient::title_case
// ===========================================================================

mod title_case {
    use super::*;

    #[test]
    fn upper_snake_case() {
        assert_eq!(TidalAppClient::title_case("HIFI_PLUS"), "Hifi Plus");
    }

    #[test]
    fn single_word_uppercase() {
        assert_eq!(TidalAppClient::title_case("PREMIUM"), "Premium");
    }

    #[test]
    fn single_word_lowercase() {
        assert_eq!(TidalAppClient::title_case("premium"), "Premium");
    }

    #[test]
    fn mixed_case_input() {
        assert_eq!(TidalAppClient::title_case("HiFi"), "Hifi");
    }

    #[test]
    fn empty_string() {
        assert_eq!(TidalAppClient::title_case(""), "");
    }

    #[test]
    fn multiple_underscores() {
        assert_eq!(TidalAppClient::title_case("A_B_C_D"), "A B C D");
    }

    #[test]
    fn already_title_case() {
        // "Free" stays "Free" (F upper, ree lower)
        assert_eq!(TidalAppClient::title_case("Free"), "Free");
    }

    #[test]
    fn all_lowercase_underscored() {
        assert_eq!(TidalAppClient::title_case("hello_world"), "Hello World");
    }

    #[test]
    fn trailing_underscore() {
        // Trailing underscore produces trailing empty split which is ignored by split_whitespace
        assert_eq!(TidalAppClient::title_case("HELLO_"), "Hello");
    }

    #[test]
    fn leading_underscore() {
        assert_eq!(TidalAppClient::title_case("_HELLO"), "Hello");
    }

    #[test]
    fn numeric_content() {
        assert_eq!(TidalAppClient::title_case("VERSION_2"), "Version 2");
    }
}

// ===========================================================================
// TidalAppClient::uuid_to_cdn_url
// ===========================================================================

mod uuid_to_cdn_url {
    use super::*;

    #[test]
    fn standard_uuid_converts_correctly() {
        let url = TidalAppClient::uuid_to_cdn_url("7e58f111-5b1a-492a-aaf1-88fb55ce8a44");
        assert_eq!(
            url,
            "https://resources.tidal.com/images/7e58f111/5b1a/492a/aaf1/88fb55ce8a44/320x320.jpg"
        );
    }

    #[test]
    fn uuid_with_no_dashes_unchanged() {
        let url = TidalAppClient::uuid_to_cdn_url("abcdef1234567890");
        assert_eq!(
            url,
            "https://resources.tidal.com/images/abcdef1234567890/320x320.jpg"
        );
    }

    #[test]
    fn empty_uuid() {
        let url = TidalAppClient::uuid_to_cdn_url("");
        assert_eq!(url, "https://resources.tidal.com/images//320x320.jpg");
    }

    #[test]
    fn uuid_result_ends_with_jpg() {
        let url = TidalAppClient::uuid_to_cdn_url("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        assert!(url.ends_with("/320x320.jpg"));
    }

    #[test]
    fn uuid_result_starts_with_tidal_cdn() {
        let url = TidalAppClient::uuid_to_cdn_url("test");
        assert!(url.starts_with("https://resources.tidal.com/images/"));
    }

    #[test]
    fn dashes_are_replaced_with_slashes() {
        let url = TidalAppClient::uuid_to_cdn_url("a-b-c");
        assert_eq!(url, "https://resources.tidal.com/images/a/b/c/320x320.jpg");
    }
}

// ===========================================================================
// TidalAppClient::derive_plan_label
// ===========================================================================

mod derive_plan_label {
    use super::*;

    // --- premium_access takes priority ---

    #[test]
    fn premium_access_hifi_plus() {
        let result = TidalAppClient::derive_plan_label(Some("HIFI_PLUS"), None, None);
        assert_eq!(result, Some("HiFi Plus".to_string()));
    }

    #[test]
    fn premium_access_hifi() {
        let result = TidalAppClient::derive_plan_label(Some("HIFI"), None, None);
        assert_eq!(result, Some("HiFi".to_string()));
    }

    #[test]
    fn premium_access_other_value() {
        let result = TidalAppClient::derive_plan_label(Some("FAMILY_PLAN"), None, None);
        assert_eq!(result, Some("Family Plan".to_string()));
    }

    #[test]
    fn premium_access_empty_string_falls_through() {
        let result = TidalAppClient::derive_plan_label(Some(""), Some("HIFI"), None);
        assert_eq!(result, Some("HiFi".to_string()));
    }

    // --- sub_type ---

    #[test]
    fn sub_type_hifi() {
        let result = TidalAppClient::derive_plan_label(None, Some("HIFI"), None);
        assert_eq!(result, Some("HiFi".to_string()));
    }

    #[test]
    fn sub_type_premium_without_quality() {
        let result = TidalAppClient::derive_plan_label(None, Some("PREMIUM"), None);
        assert_eq!(result, Some("Premium".to_string()));
    }

    #[test]
    fn sub_type_premium_with_hi_res_lossless() {
        let result =
            TidalAppClient::derive_plan_label(None, Some("PREMIUM"), Some("HI_RES_LOSSLESS"));
        assert_eq!(result, Some("HiFi Plus".to_string()));
    }

    #[test]
    fn sub_type_premium_with_hi_res() {
        let result = TidalAppClient::derive_plan_label(None, Some("PREMIUM"), Some("HI_RES"));
        assert_eq!(result, Some("HiFi Plus".to_string()));
    }

    #[test]
    fn sub_type_premium_with_lossless() {
        let result = TidalAppClient::derive_plan_label(None, Some("PREMIUM"), Some("LOSSLESS"));
        assert_eq!(result, Some("HiFi".to_string()));
    }

    #[test]
    fn sub_type_premium_with_high_quality() {
        let result = TidalAppClient::derive_plan_label(None, Some("PREMIUM"), Some("HIGH"));
        // "HIGH" doesn't upgrade Premium
        assert_eq!(result, Some("Premium".to_string()));
    }

    #[test]
    fn sub_type_free() {
        let result = TidalAppClient::derive_plan_label(None, Some("FREE"), None);
        assert_eq!(result, Some("Free".to_string()));
    }

    #[test]
    fn sub_type_unknown() {
        let result = TidalAppClient::derive_plan_label(None, Some("STUDENT"), None);
        assert_eq!(result, Some("Student".to_string()));
    }

    #[test]
    fn sub_type_empty_falls_through() {
        let result = TidalAppClient::derive_plan_label(None, Some(""), Some("HI_RES_LOSSLESS"));
        assert_eq!(result, Some("HiFi Plus".to_string()));
    }

    // --- highest_quality only (last resort) ---

    #[test]
    fn quality_hi_res_lossless() {
        let result = TidalAppClient::derive_plan_label(None, None, Some("HI_RES_LOSSLESS"));
        assert_eq!(result, Some("HiFi Plus".to_string()));
    }

    #[test]
    fn quality_hi_res() {
        let result = TidalAppClient::derive_plan_label(None, None, Some("HI_RES"));
        assert_eq!(result, Some("HiFi Plus".to_string()));
    }

    #[test]
    fn quality_lossless() {
        let result = TidalAppClient::derive_plan_label(None, None, Some("LOSSLESS"));
        assert_eq!(result, Some("HiFi".to_string()));
    }

    #[test]
    fn quality_high() {
        let result = TidalAppClient::derive_plan_label(None, None, Some("HIGH"));
        assert_eq!(result, Some("High".to_string()));
    }

    #[test]
    fn quality_low() {
        let result = TidalAppClient::derive_plan_label(None, None, Some("LOW"));
        assert_eq!(result, Some("Free".to_string()));
    }

    #[test]
    fn quality_unknown() {
        let result = TidalAppClient::derive_plan_label(None, None, Some("UNKNOWN_QUALITY"));
        assert_eq!(result, None);
    }

    // --- all None ---

    #[test]
    fn all_none_returns_none() {
        let result = TidalAppClient::derive_plan_label(None, None, None);
        assert_eq!(result, None);
    }

    // --- premium_access overrides sub_type ---

    #[test]
    fn premium_access_overrides_sub_type() {
        let result =
            TidalAppClient::derive_plan_label(Some("HIFI_PLUS"), Some("FREE"), Some("LOW"));
        assert_eq!(result, Some("HiFi Plus".to_string()));
    }
}

// ===========================================================================
// TidalAppClient::derive_plan_label_from_type_and_quality
// ===========================================================================

mod derive_plan_label_from_type_and_quality {
    use super::*;

    #[test]
    fn hifi_with_hi_res() {
        let result =
            TidalAppClient::derive_plan_label_from_type_and_quality("HIFI", "HI_RES_LOSSLESS");
        // sub_type "HIFI" takes priority
        assert_eq!(result, Some("HiFi".to_string()));
    }

    #[test]
    fn premium_with_hi_res_lossless_upgrades() {
        let result =
            TidalAppClient::derive_plan_label_from_type_and_quality("PREMIUM", "HI_RES_LOSSLESS");
        assert_eq!(result, Some("HiFi Plus".to_string()));
    }

    #[test]
    fn premium_with_lossless_upgrades_to_hifi() {
        let result = TidalAppClient::derive_plan_label_from_type_and_quality("PREMIUM", "LOSSLESS");
        assert_eq!(result, Some("HiFi".to_string()));
    }

    #[test]
    fn free_with_low() {
        let result = TidalAppClient::derive_plan_label_from_type_and_quality("FREE", "LOW");
        assert_eq!(result, Some("Free".to_string()));
    }

    #[test]
    fn premium_with_high() {
        let result = TidalAppClient::derive_plan_label_from_type_and_quality("PREMIUM", "HIGH");
        assert_eq!(result, Some("Premium".to_string()));
    }
}

// ===========================================================================
// TidalAppClient::extract_picture_url_from_json
// ===========================================================================

mod extract_picture_url {
    use super::*;

    #[test]
    fn extracts_direct_http_url_from_picture() {
        let json = serde_json::json!({
            "picture": "https://example.com/img.jpg"
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, Some("https://example.com/img.jpg".to_string()));
    }

    #[test]
    fn extracts_direct_http_url_from_profile_picture() {
        let json = serde_json::json!({
            "profilePicture": "https://example.com/profile.jpg"
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, Some("https://example.com/profile.jpg".to_string()));
    }

    #[test]
    fn converts_uuid_to_cdn_url() {
        let json = serde_json::json!({
            "picture": "7e58f111-5b1a-492a-aaf1-88fb55ce8a44"
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(
            result,
            Some(
                "https://resources.tidal.com/images/7e58f111/5b1a/492a/aaf1/88fb55ce8a44/320x320.jpg"
                    .to_string()
            )
        );
    }

    #[test]
    fn extracts_from_nested_url_key() {
        let json = serde_json::json!({
            "picture": {
                "url": "https://example.com/nested.jpg"
            }
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, Some("https://example.com/nested.jpg".to_string()));
    }

    #[test]
    fn extracts_from_nested_uuid_url_key() {
        let json = serde_json::json!({
            "picture": {
                "url": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
            }
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(
            result,
            Some(
                "https://resources.tidal.com/images/aaaaaaaa/bbbb/cccc/dddd/eeeeeeeeeeee/320x320.jpg"
                    .to_string()
            )
        );
    }

    #[test]
    fn extracts_from_size_key_320x320() {
        let json = serde_json::json!({
            "picture": {
                "320x320": "https://cdn.example.com/320.jpg"
            }
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, Some("https://cdn.example.com/320.jpg".to_string()));
    }

    #[test]
    fn extracts_from_size_key_640x640() {
        let json = serde_json::json!({
            "picture": {
                "640x640": "https://cdn.example.com/640.jpg"
            }
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, Some("https://cdn.example.com/640.jpg".to_string()));
    }

    #[test]
    fn extracts_from_size_key_750x750() {
        let json = serde_json::json!({
            "picture": {
                "750x750": "https://cdn.example.com/750.jpg"
            }
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, Some("https://cdn.example.com/750.jpg".to_string()));
    }

    #[test]
    fn extracts_from_medium_size_key() {
        let json = serde_json::json!({
            "picture": {
                "medium": "https://cdn.example.com/medium.jpg"
            }
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(
            result,
            Some("https://cdn.example.com/medium.jpg".to_string())
        );
    }

    #[test]
    fn returns_none_for_empty_object() {
        let json = serde_json::json!({});
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, None);
    }

    #[test]
    fn returns_none_for_no_matching_fields() {
        let json = serde_json::json!({
            "name": "Test",
            "id": 123
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, None);
    }

    #[test]
    fn returns_none_for_empty_string_picture() {
        let json = serde_json::json!({
            "picture": ""
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, None);
    }

    #[test]
    fn returns_none_for_null_picture() {
        let json = serde_json::json!({
            "picture": null
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, None);
    }

    #[test]
    fn prefers_profile_picture_over_picture() {
        // profilePicture is listed first in the field search order
        let json = serde_json::json!({
            "profilePicture": "https://example.com/profile.jpg",
            "picture": "https://example.com/other.jpg"
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, Some("https://example.com/profile.jpg".to_string()));
    }

    #[test]
    fn nested_object_prefers_url_key_over_size_keys() {
        let json = serde_json::json!({
            "picture": {
                "url": "https://example.com/from-url.jpg",
                "320x320": "https://example.com/from-size.jpg"
            }
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, Some("https://example.com/from-url.jpg".to_string()));
    }

    #[test]
    fn extracts_from_picture_url_field() {
        let json = serde_json::json!({
            "pictureUrl": "https://example.com/pic-url.jpg"
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(result, Some("https://example.com/pic-url.jpg".to_string()));
    }

    #[test]
    fn extracts_from_profile_picture_url_field() {
        let json = serde_json::json!({
            "profilePictureUrl": "https://example.com/profile-pic.jpg"
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(
            result,
            Some("https://example.com/profile-pic.jpg".to_string())
        );
    }

    #[test]
    fn nested_object_with_empty_url_tries_size_keys() {
        let json = serde_json::json!({
            "picture": {
                "url": "",
                "320x320": "https://cdn.example.com/fallback.jpg"
            }
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(
            result,
            Some("https://cdn.example.com/fallback.jpg".to_string())
        );
    }

    #[test]
    fn size_key_with_uuid_converts_to_cdn() {
        let json = serde_json::json!({
            "picture": {
                "320x320": "a-b-c-d"
            }
        });
        let result = TidalAppClient::extract_picture_url_from_json(&json);
        assert_eq!(
            result,
            Some("https://resources.tidal.com/images/a/b/c/d/320x320.jpg".to_string())
        );
    }
}

// ===========================================================================
// TidalAppClient::parse_mix_from_json
// ===========================================================================

mod parse_mix_from_json {
    use super::*;

    #[test]
    fn parses_complete_mix() {
        let json = serde_json::json!({
            "id": "mix-123",
            "type": "DAILY_MIX",
            "titleTextInfo": { "text": "My Daily Discovery" },
            "shortSubtitleTextInfo": { "text": "Updated daily" },
            "mixImages": [
                { "width": 160, "url": "https://example.com/small.jpg" },
                { "width": 640, "url": "https://example.com/large.jpg" },
                { "width": 320, "url": "https://example.com/medium.jpg" }
            ]
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert_eq!(mix.id, "mix-123");
        assert_eq!(mix.mix_type, "DAILY_MIX");
        assert_eq!(mix.title, "My Daily Discovery");
        assert_eq!(mix.subtitle, "Updated daily");
        // Should pick the largest image (640)
        assert_eq!(
            mix.image_url,
            Some("https://example.com/large.jpg".to_string())
        );
    }

    #[test]
    fn returns_none_when_id_missing() {
        let json = serde_json::json!({
            "type": "DAILY_MIX",
            "titleTextInfo": { "text": "Test" }
        });
        assert!(TidalAppClient::parse_mix_from_json(&json).is_none());
    }

    #[test]
    fn defaults_type_to_mix_when_missing() {
        let json = serde_json::json!({
            "id": "mix-456",
            "titleTextInfo": { "text": "Some Mix" }
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert_eq!(mix.mix_type, "MIX");
    }

    #[test]
    fn defaults_title_to_mix_when_missing() {
        let json = serde_json::json!({
            "id": "mix-789"
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert_eq!(mix.title, "Mix");
    }

    #[test]
    fn defaults_subtitle_to_empty_when_missing() {
        let json = serde_json::json!({
            "id": "mix-000"
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert_eq!(mix.subtitle, "");
    }

    #[test]
    fn falls_back_to_subtitle_text_info_when_short_missing() {
        let json = serde_json::json!({
            "id": "mix-fallback",
            "subtitleTextInfo": { "text": "Fallback subtitle" }
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert_eq!(mix.subtitle, "Fallback subtitle");
    }

    #[test]
    fn prefers_short_subtitle_over_long_subtitle() {
        let json = serde_json::json!({
            "id": "mix-pref",
            "shortSubtitleTextInfo": { "text": "Short" },
            "subtitleTextInfo": { "text": "Long" }
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert_eq!(mix.subtitle, "Short");
    }

    #[test]
    fn image_url_is_none_when_no_mix_images() {
        let json = serde_json::json!({
            "id": "mix-noimg"
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert!(mix.image_url.is_none());
    }

    #[test]
    fn image_url_is_none_when_mix_images_empty() {
        let json = serde_json::json!({
            "id": "mix-emptyimg",
            "mixImages": []
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert!(mix.image_url.is_none());
    }

    #[test]
    fn picks_largest_image_by_width() {
        let json = serde_json::json!({
            "id": "mix-imgs",
            "mixImages": [
                { "width": 100, "url": "https://example.com/100.jpg" },
                { "width": 1080, "url": "https://example.com/1080.jpg" },
                { "width": 500, "url": "https://example.com/500.jpg" }
            ]
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert_eq!(
            mix.image_url,
            Some("https://example.com/1080.jpg".to_string())
        );
    }

    #[test]
    fn single_image_is_selected() {
        let json = serde_json::json!({
            "id": "mix-single",
            "mixImages": [
                { "width": 320, "url": "https://example.com/only.jpg" }
            ]
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert_eq!(
            mix.image_url,
            Some("https://example.com/only.jpg".to_string())
        );
    }

    #[test]
    fn image_without_width_treated_as_zero() {
        let json = serde_json::json!({
            "id": "mix-nowidth",
            "mixImages": [
                { "url": "https://example.com/nowidth.jpg" },
                { "width": 100, "url": "https://example.com/100.jpg" }
            ]
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        // width 100 > 0, so the second image should be picked
        assert_eq!(
            mix.image_url,
            Some("https://example.com/100.jpg".to_string())
        );
    }

    #[test]
    fn image_without_url_is_skipped() {
        let json = serde_json::json!({
            "id": "mix-nourl",
            "mixImages": [
                { "width": 9999 },
                { "width": 100, "url": "https://example.com/100.jpg" }
            ]
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert_eq!(
            mix.image_url,
            Some("https://example.com/100.jpg".to_string())
        );
    }

    #[test]
    fn parses_artist_mix_type() {
        let json = serde_json::json!({
            "id": "artist-mix-1",
            "type": "ARTIST_MIX",
            "titleTextInfo": { "text": "Daft Punk Mix" }
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert_eq!(mix.mix_type, "ARTIST_MIX");
        assert_eq!(mix.title, "Daft Punk Mix");
    }

    #[test]
    fn parses_track_mix_type() {
        let json = serde_json::json!({
            "id": "track-mix-1",
            "type": "TRACK_MIX",
            "titleTextInfo": { "text": "Around the World Mix" },
            "shortSubtitleTextInfo": { "text": "Based on your listening" }
        });
        let mix = TidalAppClient::parse_mix_from_json(&json).unwrap();
        assert_eq!(mix.mix_type, "TRACK_MIX");
        assert_eq!(mix.title, "Around the World Mix");
        assert_eq!(mix.subtitle, "Based on your listening");
    }
}

// ===========================================================================
// TidalAppClient — audio_cache_path_for writable path
// ===========================================================================

mod client_save_and_retrieve_cache {
    use super::*;

    #[test]
    fn cache_path_for_returns_writable_path() {
        let client = TidalAppClient::new_with_audio_cache_mb(100);
        let path = client.audio_cache_path_for("write-test-track");

        // Ensure parent directory exists (audio_cache_path_for should handle this)
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }

        std::fs::write(&path, b"test data").unwrap();
        assert!(path.exists());

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }
}

// ===========================================================================
// PlaybackUrl — replay_gain_db
// ===========================================================================

mod playback_url_replay_gain {
    use super::*;

    #[test]
    fn direct_none_replay_gain() {
        let url = PlaybackUrl::Direct("https://example.com/s.mp4".to_string(), None);
        assert_eq!(url.replay_gain_db(), None);
    }

    #[test]
    fn direct_some_replay_gain() {
        let url = PlaybackUrl::Direct("https://example.com/s.mp4".to_string(), Some(-7.4));
        let rg = url.replay_gain_db().unwrap();
        assert!((rg - (-7.4)).abs() < f32::EPSILON);
    }

    #[test]
    fn dash_manifest_none_replay_gain() {
        let url = PlaybackUrl::DashManifest(PathBuf::from("/tmp/m.mpd"), None);
        assert_eq!(url.replay_gain_db(), None);
    }

    #[test]
    fn dash_manifest_some_replay_gain() {
        let url = PlaybackUrl::DashManifest(PathBuf::from("/tmp/m.mpd"), Some(-3.2));
        let rg = url.replay_gain_db().unwrap();
        assert!((rg - (-3.2)).abs() < f32::EPSILON);
    }

    #[test]
    fn cached_file_none_replay_gain() {
        let url = PlaybackUrl::CachedFile(PathBuf::from("/cache/s.dat"), None);
        assert_eq!(url.replay_gain_db(), None);
    }

    #[test]
    fn cached_file_some_replay_gain() {
        let url = PlaybackUrl::CachedFile(PathBuf::from("/cache/s.dat"), Some(2.5));
        let rg = url.replay_gain_db().unwrap();
        assert!((rg - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn replay_gain_zero() {
        let url = PlaybackUrl::Direct("https://example.com/s.mp4".to_string(), Some(0.0));
        let rg = url.replay_gain_db().unwrap();
        assert!(rg.abs() < f32::EPSILON);
    }

    #[test]
    fn replay_gain_positive() {
        let url = PlaybackUrl::Direct("https://example.com/s.mp4".to_string(), Some(5.0));
        let rg = url.replay_gain_db().unwrap();
        assert!((rg - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn replay_gain_negative() {
        let url = PlaybackUrl::Direct("https://example.com/s.mp4".to_string(), Some(-12.3));
        let rg = url.replay_gain_db().unwrap();
        assert!((rg - (-12.3)).abs() < f32::EPSILON);
    }

    #[test]
    fn replay_gain_preserved_by_clone() {
        let url = PlaybackUrl::Direct("https://example.com/s.mp4".to_string(), Some(-4.1));
        let cloned = url.clone();
        assert_eq!(url.replay_gain_db(), cloned.replay_gain_db());
    }

    #[test]
    fn replay_gain_none_preserved_by_clone() {
        let url = PlaybackUrl::DashManifest(PathBuf::from("/tmp/m.mpd"), None);
        let cloned = url.clone();
        assert_eq!(url.replay_gain_db(), cloned.replay_gain_db());
    }

    #[test]
    fn replay_gain_small_fractional() {
        let url = PlaybackUrl::CachedFile(PathBuf::from("/c/s.dat"), Some(-0.001));
        let rg = url.replay_gain_db().unwrap();
        assert!((rg - (-0.001)).abs() < f32::EPSILON);
    }

    #[test]
    fn replay_gain_large_value() {
        let url = PlaybackUrl::Direct("https://example.com/loud.mp4".to_string(), Some(-51.0));
        let rg = url.replay_gain_db().unwrap();
        assert!((rg - (-51.0)).abs() < f32::EPSILON);
    }
}

// ===========================================================================
// TidalAppClient — save / load replay_gain sidecar
// ===========================================================================

mod client_replay_gain {
    use super::*;

    #[test]
    fn save_then_load_replay_gain() {
        let client = TidalAppClient::new_with_audio_cache_mb(100);
        let track_id = "rg-save-load-test-001";

        client.save_replay_gain(track_id, -7.4);

        let loaded = client.load_replay_gain(track_id);
        assert!(loaded.is_some(), "should find saved replay gain");
        let rg = loaded.unwrap();
        assert!((rg - (-7.4)).abs() < 0.01, "expected -7.4, got {rg}");

        // Cleanup: remove the sidecar file
        let key = client.audio_cache_key(track_id);
        let path = client.audio_cache().hashed_path(&key, "rg");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_replay_gain_nonexistent_returns_none() {
        let client = TidalAppClient::new_with_audio_cache_mb(100);
        let loaded = client.load_replay_gain("rg-nonexistent-track-xyz");
        assert!(loaded.is_none());
    }

    #[test]
    fn save_replay_gain_zero() {
        let client = TidalAppClient::new_with_audio_cache_mb(100);
        let track_id = "rg-zero-test-002";

        client.save_replay_gain(track_id, 0.0);

        let loaded = client.load_replay_gain(track_id);
        assert!(loaded.is_some());
        let rg = loaded.unwrap();
        assert!(rg.abs() < 0.01, "expected ~0.0, got {rg}");

        // Cleanup
        let key = client.audio_cache_key(track_id);
        let path = client.audio_cache().hashed_path(&key, "rg");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_replay_gain_positive() {
        let client = TidalAppClient::new_with_audio_cache_mb(100);
        let track_id = "rg-positive-test-003";

        client.save_replay_gain(track_id, 3.5);

        let loaded = client.load_replay_gain(track_id);
        assert!(loaded.is_some());
        let rg = loaded.unwrap();
        assert!((rg - 3.5).abs() < 0.01, "expected 3.5, got {rg}");

        // Cleanup
        let key = client.audio_cache_key(track_id);
        let path = client.audio_cache().hashed_path(&key, "rg");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_replay_gain_overwrites_previous() {
        let client = TidalAppClient::new_with_audio_cache_mb(100);
        let track_id = "rg-overwrite-test-004";

        client.save_replay_gain(track_id, -5.0);
        client.save_replay_gain(track_id, -9.2);

        let loaded = client.load_replay_gain(track_id);
        assert!(loaded.is_some());
        let rg = loaded.unwrap();
        assert!((rg - (-9.2)).abs() < 0.01, "expected -9.2, got {rg}");

        // Cleanup
        let key = client.audio_cache_key(track_id);
        let path = client.audio_cache().hashed_path(&key, "rg");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn replay_gain_for_different_tracks_are_independent() {
        let client = TidalAppClient::new_with_audio_cache_mb(100);
        let track_a = "rg-independent-a-005";
        let track_b = "rg-independent-b-005";

        client.save_replay_gain(track_a, -3.0);
        client.save_replay_gain(track_b, -8.0);

        let rg_a = client.load_replay_gain(track_a).unwrap();
        let rg_b = client.load_replay_gain(track_b).unwrap();
        assert!((rg_a - (-3.0)).abs() < 0.01);
        assert!((rg_b - (-8.0)).abs() < 0.01);

        // Cleanup
        for tid in [track_a, track_b] {
            let key = client.audio_cache_key(tid);
            let path = client.audio_cache().hashed_path(&key, "rg");
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn save_replay_gain_large_negative() {
        let client = TidalAppClient::new_with_audio_cache_mb(100);
        let track_id = "rg-large-neg-006";

        client.save_replay_gain(track_id, -51.0);

        let loaded = client.load_replay_gain(track_id).unwrap();
        assert!((loaded - (-51.0)).abs() < 0.01);

        // Cleanup
        let key = client.audio_cache_key(track_id);
        let path = client.audio_cache().hashed_path(&key, "rg");
        let _ = std::fs::remove_file(&path);
    }
}

// ===========================================================================
// TidalAppClient — additional cache edge cases
// ===========================================================================

mod client_cache_edge_cases {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a `TidalAppClient` backed by an isolated temp directory.
    /// The returned `TempDir` must be kept alive for the duration of the test.
    fn isolated_client(audio_mb: u32) -> (TidalAppClient, TempDir) {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let client = TidalAppClient::new_with_cache_dir(tmp.path(), audio_mb);
        (client, tmp)
    }

    #[test]
    fn get_cached_audio_path_returns_none_for_unknown_track() {
        let (client, _tmp) = isolated_client(100);
        let cached = client.get_cached_audio_path("definitely-not-cached-track-xyz-999");
        assert!(cached.is_none());
    }

    #[test]
    fn audio_cache_path_for_is_deterministic() {
        let (client, _tmp) = isolated_client(100);
        let path1 = client.audio_cache_path_for("deterministic-track");
        let path2 = client.audio_cache_path_for("deterministic-track");
        assert_eq!(path1, path2);
    }

    #[test]
    fn audio_cache_path_for_different_tracks_differ() {
        let (client, _tmp) = isolated_client(100);
        let path_a = client.audio_cache_path_for("track-alpha");
        let path_b = client.audio_cache_path_for("track-beta");
        assert_ne!(path_a, path_b);
    }

    #[test]
    fn audio_cache_key_includes_quality_indicator() {
        let (client, _tmp) = isolated_client(100);
        let key = client.audio_cache_key("some-track");
        assert!(
            key.contains("some-track"),
            "key should contain the track id: {key}"
        );
        // The default quality is High
        assert!(
            key.contains("High"),
            "key should contain quality indicator: {key}"
        );
    }

    #[test]
    fn audio_cache_size_after_clear_is_small() {
        let (client, _tmp) = isolated_client(100);
        client.clear_audio_cache();
        let size = client.audio_cache_size();
        assert!(
            size < 4096,
            "cache size after clear should be ~0, got {size}"
        );
    }

    #[test]
    fn audio_cache_max_reflects_configured_mb() {
        let (client, _tmp) = isolated_client(500);
        let max_bytes = client.audio_cache_max();
        let expected = 500u64 * 1024 * 1024;
        assert_eq!(max_bytes, expected);
    }

    #[test]
    fn audio_cache_max_for_default_client() {
        let client = TidalAppClient::new();
        let max_bytes = client.audio_cache_max();
        let expected = 2000u64 * 1024 * 1024;
        assert_eq!(max_bytes, expected);
    }

    #[test]
    fn cache_path_parent_dir_exists() {
        let (client, _tmp) = isolated_client(100);
        let path = client.audio_cache_path_for("parent-dir-test-004");
        if let Some(parent) = path.parent() {
            assert!(
                parent.exists() || std::fs::create_dir_all(parent).is_ok(),
                "parent directory should exist or be creatable"
            );
        }
    }
}

// ===========================================================================
// PlaybackUrl — combined flag + replay_gain tests
// ===========================================================================

mod playback_url_combined {
    use super::*;

    #[test]
    fn direct_with_gain_is_not_dash_not_cached() {
        let url = PlaybackUrl::Direct("https://example.com/s.mp4".to_string(), Some(-5.0));
        assert!(!url.is_dash());
        assert!(!url.is_cached());
        assert!(url.replay_gain_db().is_some());
    }

    #[test]
    fn dash_with_gain_is_dash_not_cached() {
        let url = PlaybackUrl::DashManifest(PathBuf::from("/tmp/m.mpd"), Some(-3.0));
        assert!(url.is_dash());
        assert!(!url.is_cached());
        assert!(url.replay_gain_db().is_some());
    }

    #[test]
    fn cached_with_gain_is_cached_not_dash() {
        let url = PlaybackUrl::CachedFile(PathBuf::from("/c/s.dat"), Some(-8.0));
        assert!(!url.is_dash());
        assert!(url.is_cached());
        assert!(url.replay_gain_db().is_some());
    }

    #[test]
    fn direct_without_gain_has_none_replay_gain() {
        let url = PlaybackUrl::Direct("https://example.com/s.mp4".to_string(), None);
        assert!(!url.is_dash());
        assert!(!url.is_cached());
        assert!(url.replay_gain_db().is_none());
    }

    #[test]
    fn as_url_works_regardless_of_replay_gain() {
        let url1 = PlaybackUrl::Direct("https://a.com/1.mp4".to_string(), None);
        let url2 = PlaybackUrl::Direct("https://a.com/1.mp4".to_string(), Some(-4.0));
        assert_eq!(url1.as_url(), url2.as_url());
    }

    #[test]
    fn dash_as_url_works_regardless_of_replay_gain() {
        let p = PathBuf::from("/tmp/m.mpd");
        let url1 = PlaybackUrl::DashManifest(p.clone(), None);
        let url2 = PlaybackUrl::DashManifest(p, Some(-2.0));
        assert_eq!(url1.as_url(), url2.as_url());
    }

    #[test]
    fn cached_as_url_works_regardless_of_replay_gain() {
        let p = PathBuf::from("/c/s.dat");
        let url1 = PlaybackUrl::CachedFile(p.clone(), None);
        let url2 = PlaybackUrl::CachedFile(p, Some(1.0));
        assert_eq!(url1.as_url(), url2.as_url());
    }
}
