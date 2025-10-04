// SPDX-License-Identifier: MIT

//! Integration tests for the i18n (internationalization) module.
//!
//! Tests the localizer factory, the static LANGUAGE_LOADER, and the init()
//! function to ensure the localization infrastructure works correctly.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::i18n;
use i18n_embed::LanguageLoader;
use i18n_embed::unic_langid::LanguageIdentifier;

// ===========================================================================
// LANGUAGE_LOADER static
// ===========================================================================

mod language_loader {
    use super::*;

    #[test]
    fn language_loader_is_accessible() {
        // Accessing the lazy static should not panic
        let _loader = &*i18n::LANGUAGE_LOADER;
    }

    #[test]
    fn language_loader_has_fallback_language() {
        let loader = &*i18n::LANGUAGE_LOADER;
        let current = loader.current_language();
        // The fallback language should be set (typically "en")
        assert!(
            !current.to_string().is_empty(),
            "current language should not be empty after fallback loading"
        );
    }

    #[test]
    fn language_loader_current_language_is_english_fallback() {
        let loader = &*i18n::LANGUAGE_LOADER;
        let lang = loader.current_language();
        // The fallback in the i18n/en/ directory should make "en" the default
        assert_eq!(
            lang.language.as_str(),
            "en",
            "fallback language should be English, got {:?}",
            lang
        );
    }

    #[test]
    fn language_loader_domain_is_nonempty() {
        let loader = &*i18n::LANGUAGE_LOADER;
        let domain = loader.domain();
        assert!(
            !domain.is_empty(),
            "language loader domain should not be empty"
        );
    }
}

// ===========================================================================
// localizer()
// ===========================================================================

mod localizer_fn {
    use super::*;

    #[test]
    fn localizer_returns_valid_boxed_localizer() {
        // Should not panic
        let _localizer = i18n::localizer();
    }

    #[test]
    fn localizer_can_be_called_multiple_times() {
        // Ensures no single-use or move semantics issues
        let _l1 = i18n::localizer();
        let _l2 = i18n::localizer();
        let _l3 = i18n::localizer();
    }

    #[test]
    fn localizer_available_languages_includes_english() {
        let localizer = i18n::localizer();
        let available = localizer.available_languages().unwrap_or_default();
        // Should have at least one language (en)
        assert!(
            !available.is_empty(),
            "available_languages should include at least the fallback"
        );

        let has_english = available.iter().any(|l| l.language.as_str() == "en");
        assert!(
            has_english,
            "available languages should include English, got: {:?}",
            available
        );
    }
}

// ===========================================================================
// init()
// ===========================================================================

mod init_fn {
    use super::*;

    fn lang(s: &str) -> LanguageIdentifier {
        s.parse().expect("valid language identifier")
    }

    #[test]
    fn init_with_empty_languages_does_not_panic() {
        i18n::init(&[]);
    }

    #[test]
    fn init_with_english_does_not_panic() {
        let langs = vec![lang("en")];
        i18n::init(&langs);
    }

    #[test]
    fn init_with_nonexistent_language_does_not_panic() {
        // Requesting a language that doesn't exist should fall back gracefully
        let langs = vec![lang("xx")];
        i18n::init(&langs);
    }

    #[test]
    fn init_with_multiple_languages_does_not_panic() {
        let langs = vec![lang("fr"), lang("de"), lang("en")];
        i18n::init(&langs);
    }

    #[test]
    fn init_can_be_called_multiple_times() {
        // Re-initialization should not panic or corrupt state
        let en = vec![lang("en")];
        i18n::init(&en);
        i18n::init(&en);
        i18n::init(&[]);
        i18n::init(&en);
    }

    #[test]
    fn init_with_english_preserves_english_as_current() {
        let langs = vec![lang("en")];
        i18n::init(&langs);
        let loader = &*i18n::LANGUAGE_LOADER;
        let current = loader.current_language();
        assert_eq!(
            current.language.as_str(),
            "en",
            "after init with English, current language should be English"
        );
    }
}

// ===========================================================================
// fl! macro
// ===========================================================================

mod fl_macro {
    #[test]
    fn fl_sign_in_returns_nonempty() {
        cosmic_applet_mare::i18n::init(&[]);
        let s = cosmic_applet_mare::fl!("sign-in");
        assert!(!s.is_empty());
    }

    #[test]
    fn fl_search_returns_nonempty() {
        cosmic_applet_mare::i18n::init(&[]);
        let s = cosmic_applet_mare::fl!("search");
        assert!(!s.is_empty());
    }

    #[test]
    fn fl_settings_returns_nonempty() {
        cosmic_applet_mare::i18n::init(&[]);
        let s = cosmic_applet_mare::fl!("settings");
        assert!(!s.is_empty());
    }

    #[test]
    fn fl_loading_returns_nonempty() {
        cosmic_applet_mare::i18n::init(&[]);
        let s = cosmic_applet_mare::fl!("loading");
        assert!(!s.is_empty());
    }

    #[test]
    fn fl_known_keys_are_all_nonempty() {
        cosmic_applet_mare::i18n::init(&[]);
        assert!(!cosmic_applet_mare::fl!("sign-in").is_empty());
        assert!(!cosmic_applet_mare::fl!("sign-out").is_empty());
        assert!(!cosmic_applet_mare::fl!("search").is_empty());
        assert!(!cosmic_applet_mare::fl!("settings").is_empty());
        assert!(!cosmic_applet_mare::fl!("back").is_empty());
        assert!(!cosmic_applet_mare::fl!("loading").is_empty());
        assert!(!cosmic_applet_mare::fl!("cancel").is_empty());
    }

    #[test]
    fn fl_navigation_keys_all_resolve() {
        cosmic_applet_mare::i18n::init(&[]);
        assert!(!cosmic_applet_mare::fl!("search").is_empty());
        assert!(!cosmic_applet_mare::fl!("settings").is_empty());
        assert!(!cosmic_applet_mare::fl!("back").is_empty());
    }

    #[test]
    fn fl_quality_keys_all_resolve() {
        cosmic_applet_mare::i18n::init(&[]);
        assert!(!cosmic_applet_mare::fl!("quality-low").is_empty());
        assert!(!cosmic_applet_mare::fl!("quality-high").is_empty());
        assert!(!cosmic_applet_mare::fl!("quality-lossless").is_empty());
        assert!(!cosmic_applet_mare::fl!("quality-hires").is_empty());
    }
}
