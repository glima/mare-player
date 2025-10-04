// SPDX-License-Identifier: MIT

//! Integration tests for the authentication module.
//!
//! Covers UserProfile display_name logic (all branches), initials generation,
//! AuthManager state machine, AuthState transitions, and DeviceCodeInfo.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::tidal::auth::{AuthManager, AuthState, UserProfile};

// ===========================================================================
// UserProfile::display_name — all branches
// ===========================================================================

mod display_name {
    use super::*;

    /// Branch 1: "First Last" when both first_name and last_name are non-empty.
    #[test]
    fn first_and_last_name() {
        let p = UserProfile {
            first_name: Some("John".to_string()),
            last_name: Some("Doe".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "John Doe");
    }

    /// Branch 1 (negative): first_name present but last_name empty string.
    #[test]
    fn first_name_with_empty_last_name_skips_to_next() {
        let p = UserProfile {
            first_name: Some("John".to_string()),
            last_name: Some(String::new()),
            ..Default::default()
        };
        // Should NOT produce "John " — falls through to next branch
        assert_ne!(p.display_name(), "John ");
    }

    /// Branch 1 (negative): last_name present but first_name empty string.
    #[test]
    fn empty_first_name_with_last_name_skips_to_next() {
        let p = UserProfile {
            first_name: Some(String::new()),
            last_name: Some("Doe".to_string()),
            ..Default::default()
        };
        assert_ne!(p.display_name(), " Doe");
    }

    /// Branch 1 (negative): first_name is None, last_name is Some.
    #[test]
    fn none_first_name_with_last_name() {
        let p = UserProfile {
            first_name: None,
            last_name: Some("Doe".to_string()),
            ..Default::default()
        };
        // Falls through — should not panic
        let name = p.display_name();
        assert!(!name.is_empty());
    }

    /// Branch 2: full_name (TIDAL's fullName field) when first+last unavailable.
    #[test]
    fn full_name_fallback() {
        let p = UserProfile {
            full_name: Some("Jane Smith".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "Jane Smith");
    }

    /// Branch 2 (negative): full_name is empty string — skip to next.
    #[test]
    fn empty_full_name_skips_to_next() {
        let p = UserProfile {
            full_name: Some(String::new()),
            nickname: Some("jsmith".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "jsmith");
    }

    /// Branch 3: nickname.
    #[test]
    fn nickname_fallback() {
        let p = UserProfile {
            nickname: Some("cooluser42".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "cooluser42");
    }

    /// Branch 3 (negative): nickname is empty string — skip to next.
    #[test]
    fn empty_nickname_skips_to_next() {
        let p = UserProfile {
            nickname: Some(String::new()),
            first_name: Some("Alice".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "Alice");
    }

    /// Branch 4: first_name alone (when last_name is absent or empty).
    #[test]
    fn first_name_alone() {
        let p = UserProfile {
            first_name: Some("Alice".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "Alice");
    }

    /// Branch 4 (negative): first_name is empty string — skip to next.
    #[test]
    fn empty_first_name_alone_skips_to_next() {
        let p = UserProfile {
            first_name: Some(String::new()),
            username: Some("alice99".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "alice99");
    }

    /// Branch 5: username (when it doesn't look like an email).
    #[test]
    fn username_fallback() {
        let p = UserProfile {
            username: Some("bob_music".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "bob_music");
    }

    /// Branch 5 (negative): username that looks like an email — skip to next.
    #[test]
    fn email_like_username_skips_to_email() {
        let p = UserProfile {
            username: Some("bob@example.com".to_string()),
            email: Some("bob@example.com".to_string()),
            ..Default::default()
        };
        // Should use the email branch, not the username branch
        assert_eq!(p.display_name(), "bob@example.com");
    }

    /// Branch 5 (negative): empty username — skip to next.
    #[test]
    fn empty_username_skips_to_next() {
        let p = UserProfile {
            username: Some(String::new()),
            email: Some("user@mail.com".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "user@mail.com");
    }

    /// Branch 6: email as last resort before fallback.
    #[test]
    fn email_fallback() {
        let p = UserProfile {
            email: Some("user@example.org".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "user@example.org");
    }

    /// Branch 6 (negative): empty email — skip to final fallback.
    #[test]
    fn empty_email_skips_to_signed_in() {
        let p = UserProfile {
            email: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "Signed in");
    }

    /// Branch 7: Final fallback — all fields are None/empty.
    #[test]
    fn all_none_returns_signed_in() {
        let p = UserProfile::default();
        assert_eq!(p.display_name(), "Signed in");
    }

    /// Branch 7: all fields are Some but empty strings.
    #[test]
    fn all_empty_strings_returns_signed_in() {
        let p = UserProfile {
            username: Some(String::new()),
            first_name: Some(String::new()),
            last_name: Some(String::new()),
            full_name: Some(String::new()),
            nickname: Some(String::new()),
            email: Some(String::new()),
            picture_url: None,
            subscription_plan: None,
        };
        assert_eq!(p.display_name(), "Signed in");
    }

    /// Priority: first+last beats full_name.
    #[test]
    fn first_last_takes_priority_over_full_name() {
        let p = UserProfile {
            first_name: Some("John".to_string()),
            last_name: Some("Doe".to_string()),
            full_name: Some("Jonathan Doefield".to_string()),
            nickname: Some("jdoe".to_string()),
            username: Some("johnd".to_string()),
            email: Some("john@example.com".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "John Doe");
    }

    /// Priority: full_name beats nickname.
    #[test]
    fn full_name_takes_priority_over_nickname() {
        let p = UserProfile {
            full_name: Some("Jane Smith".to_string()),
            nickname: Some("jsmith".to_string()),
            username: Some("janesmith".to_string()),
            email: Some("jane@example.com".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "Jane Smith");
    }

    /// Priority: nickname beats first_name alone.
    #[test]
    fn nickname_takes_priority_over_first_name_alone() {
        let p = UserProfile {
            first_name: Some(String::new()),
            last_name: Some(String::new()),
            nickname: Some("coolnick".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "coolnick");
    }

    /// Unicode names.
    #[test]
    fn unicode_names() {
        let p = UserProfile {
            first_name: Some("José".to_string()),
            last_name: Some("García".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "José García");
    }

    /// CJK names.
    #[test]
    fn cjk_names() {
        let p = UserProfile {
            first_name: Some("太郎".to_string()),
            last_name: Some("山田".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "太郎 山田");
    }

    /// Emoji in nickname.
    #[test]
    fn emoji_nickname() {
        let p = UserProfile {
            nickname: Some("🎵musiclover🎵".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "🎵musiclover🎵");
    }

    /// Username with @ is treated as email-like.
    #[test]
    fn username_with_at_sign_is_email_like() {
        let p = UserProfile {
            username: Some("user@domain".to_string()),
            ..Default::default()
        };
        // Contains '@' so username branch is skipped, falls through to email (None) then "Signed in"
        assert_eq!(p.display_name(), "Signed in");
    }

    /// Whitespace-only fields should still count as non-empty
    /// (the code checks `!name.is_empty()`, not trimming).
    #[test]
    fn whitespace_only_first_name_is_non_empty() {
        let p = UserProfile {
            first_name: Some("  ".to_string()),
            last_name: Some("  ".to_string()),
            ..Default::default()
        };
        // "  " is not empty, so "First Last" branch fires: "  " + " " + "  " = "     "
        assert_eq!(p.display_name(), "     ");
    }
}

// ===========================================================================
// UserProfile::initials
// ===========================================================================

mod initials {
    use super::*;

    #[test]
    fn initials_from_first_last() {
        let p = UserProfile {
            first_name: Some("John".to_string()),
            last_name: Some("Doe".to_string()),
            ..Default::default()
        };
        assert_eq!(p.initials(), "J");
    }

    #[test]
    fn initials_from_nickname() {
        let p = UserProfile {
            nickname: Some("cooluser".to_string()),
            ..Default::default()
        };
        assert_eq!(p.initials(), "C");
    }

    #[test]
    fn initials_from_email() {
        let p = UserProfile {
            email: Some("alice@example.com".to_string()),
            ..Default::default()
        };
        assert_eq!(p.initials(), "A");
    }

    #[test]
    fn initials_fallback_signed_in() {
        let p = UserProfile::default();
        assert_eq!(p.initials(), "S"); // "Signed in" → 'S'
    }

    #[test]
    fn initials_lowercase_gets_uppercased() {
        let p = UserProfile {
            first_name: Some("alice".to_string()),
            last_name: Some("bob".to_string()),
            ..Default::default()
        };
        assert_eq!(p.initials(), "A");
    }

    #[test]
    fn initials_unicode() {
        let p = UserProfile {
            first_name: Some("José".to_string()),
            last_name: Some("García".to_string()),
            ..Default::default()
        };
        assert_eq!(p.initials(), "J");
    }

    #[test]
    fn initials_cjk() {
        let p = UserProfile {
            first_name: Some("太郎".to_string()),
            last_name: Some("山田".to_string()),
            ..Default::default()
        };
        assert_eq!(p.initials(), "太");
    }

    #[test]
    fn initials_emoji() {
        let p = UserProfile {
            nickname: Some("🎵music".to_string()),
            ..Default::default()
        };
        assert_eq!(p.initials(), "🎵");
    }

    #[test]
    fn initials_number() {
        let p = UserProfile {
            username: Some("42user".to_string()),
            ..Default::default()
        };
        assert_eq!(p.initials(), "4");
    }
}

// ===========================================================================
// UserProfile — Default and PartialEq
// ===========================================================================

mod user_profile_traits {
    use super::*;

    #[test]
    fn default_is_all_none() {
        let p = UserProfile::default();
        assert_eq!(p.username, None);
        assert_eq!(p.first_name, None);
        assert_eq!(p.last_name, None);
        assert_eq!(p.full_name, None);
        assert_eq!(p.nickname, None);
        assert_eq!(p.email, None);
        assert_eq!(p.picture_url, None);
        assert_eq!(p.subscription_plan, None);
    }

    #[test]
    fn partial_eq_identical() {
        let p1 = UserProfile {
            first_name: Some("Alice".to_string()),
            ..Default::default()
        };
        let p2 = UserProfile {
            first_name: Some("Alice".to_string()),
            ..Default::default()
        };
        assert_eq!(p1, p2);
    }

    #[test]
    fn partial_eq_different() {
        let p1 = UserProfile {
            first_name: Some("Alice".to_string()),
            ..Default::default()
        };
        let p2 = UserProfile {
            first_name: Some("Bob".to_string()),
            ..Default::default()
        };
        assert_ne!(p1, p2);
    }

    #[test]
    fn partial_eq_none_vs_some() {
        let p1 = UserProfile::default();
        let p2 = UserProfile {
            email: Some("a@b.com".to_string()),
            ..Default::default()
        };
        assert_ne!(p1, p2);
    }

    #[test]
    fn clone_is_equal() {
        let p = UserProfile {
            username: Some("test".to_string()),
            first_name: Some("First".to_string()),
            last_name: Some("Last".to_string()),
            full_name: Some("First Last".to_string()),
            nickname: Some("nick".to_string()),
            email: Some("test@example.com".to_string()),
            picture_url: Some("https://example.com/pic.jpg".to_string()),
            subscription_plan: Some("HiFi Plus".to_string()),
        };
        let q = p.clone();
        assert_eq!(p, q);
    }

    #[test]
    fn debug_format() {
        let p = UserProfile {
            username: Some("dbguser".to_string()),
            ..Default::default()
        };
        let debug = format!("{:?}", p);
        assert!(debug.contains("dbguser"));
    }

    #[test]
    fn eq_is_reflexive() {
        let p = UserProfile {
            first_name: Some("Test".to_string()),
            ..Default::default()
        };
        assert_eq!(p, p);
    }
}

// ===========================================================================
// UserProfile — subscription_plan and picture_url (non-display fields)
// ===========================================================================

mod user_profile_extra_fields {
    use super::*;

    #[test]
    fn picture_url_does_not_affect_display_name() {
        let p = UserProfile {
            picture_url: Some("https://example.com/avatar.jpg".to_string()),
            ..Default::default()
        };
        // picture_url is not used in display_name
        assert_eq!(p.display_name(), "Signed in");
    }

    #[test]
    fn subscription_plan_does_not_affect_display_name() {
        let p = UserProfile {
            subscription_plan: Some("HiFi Plus".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "Signed in");
    }

    #[test]
    fn full_profile_display_name_uses_first_last() {
        let p = UserProfile {
            username: Some("jdoe".to_string()),
            first_name: Some("John".to_string()),
            last_name: Some("Doe".to_string()),
            full_name: Some("John Doe".to_string()),
            nickname: Some("Johnny".to_string()),
            email: Some("john@example.com".to_string()),
            picture_url: Some("https://pic.example.com/j.jpg".to_string()),
            subscription_plan: Some("HiFi Plus".to_string()),
        };
        // First + Last takes highest priority
        assert_eq!(p.display_name(), "John Doe");
        assert_eq!(p.initials(), "J");
    }
}

// ===========================================================================
// AuthManager
// ===========================================================================

mod auth_manager {
    use super::*;

    #[test]
    fn new_starts_not_authenticated() {
        let m = AuthManager::new();
        assert_eq!(*m.state(), AuthState::NotAuthenticated);
    }

    #[test]
    fn default_starts_not_authenticated() {
        let m = AuthManager::default();
        assert_eq!(*m.state(), AuthState::NotAuthenticated);
    }

    #[test]
    fn set_state_to_awaiting_auth() {
        let mut m = AuthManager::new();
        m.set_state(AuthState::AwaitingUserAuth {
            verification_uri: "https://link.tidal.com/ABCDE".to_string(),
            user_code: "ABCDE".to_string(),
        });
        match m.state() {
            AuthState::AwaitingUserAuth {
                verification_uri,
                user_code,
            } => {
                assert_eq!(verification_uri, "https://link.tidal.com/ABCDE");
                assert_eq!(user_code, "ABCDE");
            }
            other => panic!("Expected AwaitingUserAuth, got {:?}", other),
        }
    }

    #[test]
    fn set_state_to_authenticated() {
        let mut m = AuthManager::new();
        let profile = UserProfile {
            username: Some("testuser".to_string()),
            first_name: Some("Test".to_string()),
            last_name: Some("User".to_string()),
            ..Default::default()
        };
        m.set_state(AuthState::Authenticated {
            profile: profile.clone(),
        });
        match m.state() {
            AuthState::Authenticated { profile: p } => {
                assert_eq!(p.display_name(), "Test User");
            }
            other => panic!("Expected Authenticated, got {:?}", other),
        }
    }

    #[test]
    fn set_state_to_failed() {
        let mut m = AuthManager::new();
        m.set_state(AuthState::Failed("network timeout".to_string()));
        match m.state() {
            AuthState::Failed(msg) => {
                assert_eq!(msg, "network timeout");
            }
            other => panic!("Expected Failed, got {:?}", other),
        }
    }

    #[test]
    fn state_transitions_full_cycle() {
        let mut m = AuthManager::new();

        // Start: NotAuthenticated
        assert_eq!(*m.state(), AuthState::NotAuthenticated);

        // Initiate OAuth
        m.set_state(AuthState::AwaitingUserAuth {
            verification_uri: "https://link.tidal.com/XYZ".to_string(),
            user_code: "XYZ".to_string(),
        });
        assert!(matches!(m.state(), AuthState::AwaitingUserAuth { .. }));

        // OAuth completes
        m.set_state(AuthState::Authenticated {
            profile: UserProfile {
                username: Some("user".to_string()),
                ..Default::default()
            },
        });
        assert!(matches!(m.state(), AuthState::Authenticated { .. }));

        // Logout
        m.set_state(AuthState::NotAuthenticated);
        assert_eq!(*m.state(), AuthState::NotAuthenticated);
    }

    #[test]
    fn state_transitions_failure_and_retry() {
        let mut m = AuthManager::new();

        // Start OAuth
        m.set_state(AuthState::AwaitingUserAuth {
            verification_uri: "https://link.tidal.com/ABC".to_string(),
            user_code: "ABC".to_string(),
        });

        // OAuth fails
        m.set_state(AuthState::Failed("User denied access".to_string()));
        assert!(matches!(m.state(), AuthState::Failed(_)));

        // Retry
        m.set_state(AuthState::NotAuthenticated);
        assert_eq!(*m.state(), AuthState::NotAuthenticated);

        // Start OAuth again
        m.set_state(AuthState::AwaitingUserAuth {
            verification_uri: "https://link.tidal.com/DEF".to_string(),
            user_code: "DEF".to_string(),
        });
        assert!(matches!(m.state(), AuthState::AwaitingUserAuth { .. }));

        // This time it succeeds
        m.set_state(AuthState::Authenticated {
            profile: UserProfile::default(),
        });
        assert!(matches!(m.state(), AuthState::Authenticated { .. }));
    }

    #[test]
    fn overwrite_authenticated_with_different_profile() {
        let mut m = AuthManager::new();

        m.set_state(AuthState::Authenticated {
            profile: UserProfile {
                first_name: Some("First".to_string()),
                ..Default::default()
            },
        });

        // Re-authenticate as a different user
        m.set_state(AuthState::Authenticated {
            profile: UserProfile {
                first_name: Some("Second".to_string()),
                ..Default::default()
            },
        });

        match m.state() {
            AuthState::Authenticated { profile } => {
                assert_eq!(profile.display_name(), "Second");
            }
            other => panic!("Expected Authenticated, got {:?}", other),
        }
    }
}

// ===========================================================================
// AuthState — PartialEq and Clone
// ===========================================================================

mod auth_state_traits {
    use super::*;

    #[test]
    fn not_authenticated_eq() {
        assert_eq!(AuthState::NotAuthenticated, AuthState::NotAuthenticated);
    }

    #[test]
    fn awaiting_user_auth_eq() {
        let a = AuthState::AwaitingUserAuth {
            verification_uri: "https://example.com".to_string(),
            user_code: "CODE".to_string(),
        };
        let b = AuthState::AwaitingUserAuth {
            verification_uri: "https://example.com".to_string(),
            user_code: "CODE".to_string(),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn awaiting_user_auth_ne_different_code() {
        let a = AuthState::AwaitingUserAuth {
            verification_uri: "https://example.com".to_string(),
            user_code: "CODE1".to_string(),
        };
        let b = AuthState::AwaitingUserAuth {
            verification_uri: "https://example.com".to_string(),
            user_code: "CODE2".to_string(),
        };
        assert_ne!(a, b);
    }

    #[test]
    fn authenticated_eq() {
        let profile = UserProfile {
            username: Some("user".to_string()),
            ..Default::default()
        };
        let a = AuthState::Authenticated {
            profile: profile.clone(),
        };
        let b = AuthState::Authenticated { profile };
        assert_eq!(a, b);
    }

    #[test]
    fn authenticated_ne_different_profile() {
        let a = AuthState::Authenticated {
            profile: UserProfile {
                username: Some("user1".to_string()),
                ..Default::default()
            },
        };
        let b = AuthState::Authenticated {
            profile: UserProfile {
                username: Some("user2".to_string()),
                ..Default::default()
            },
        };
        assert_ne!(a, b);
    }

    #[test]
    fn failed_eq() {
        let a = AuthState::Failed("error".to_string());
        let b = AuthState::Failed("error".to_string());
        assert_eq!(a, b);
    }

    #[test]
    fn failed_ne_different_message() {
        let a = AuthState::Failed("error 1".to_string());
        let b = AuthState::Failed("error 2".to_string());
        assert_ne!(a, b);
    }

    #[test]
    fn different_variants_ne() {
        let states: Vec<AuthState> = vec![
            AuthState::NotAuthenticated,
            AuthState::AwaitingUserAuth {
                verification_uri: "https://example.com".to_string(),
                user_code: "CODE".to_string(),
            },
            AuthState::Authenticated {
                profile: UserProfile::default(),
            },
            AuthState::Failed("err".to_string()),
        ];

        for (i, a) in states.iter().enumerate() {
            for (j, b) in states.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b, "Same variant at index {} should be equal", i);
                } else {
                    assert_ne!(a, b, "Variants at {} and {} should differ", i, j);
                }
            }
        }
    }

    #[test]
    fn clone_preserves_equality() {
        let original = AuthState::Authenticated {
            profile: UserProfile {
                username: Some("cloned".to_string()),
                first_name: Some("Clone".to_string()),
                last_name: Some("Test".to_string()),
                ..Default::default()
            },
        };
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn debug_format() {
        let state = AuthState::NotAuthenticated;
        let debug = format!("{:?}", state);
        assert!(debug.contains("NotAuthenticated"));

        let state = AuthState::Failed("oops".to_string());
        let debug = format!("{:?}", state);
        assert!(debug.contains("oops"));
    }
}

// ===========================================================================
// Edge cases and stress
// ===========================================================================

mod edge_cases {
    use super::*;

    /// Very long name fields.
    #[test]
    fn very_long_names() {
        let long_name = "A".repeat(10_000);
        let p = UserProfile {
            first_name: Some(long_name.clone()),
            last_name: Some(long_name.clone()),
            ..Default::default()
        };
        let display = p.display_name();
        assert_eq!(display.len(), 20_001); // "AAA...AAA AAA...AAA"
        assert_eq!(p.initials(), "A");
    }

    /// Names with special characters.
    #[test]
    fn special_characters_in_name() {
        let p = UserProfile {
            first_name: Some("O'Brien".to_string()),
            last_name: Some("Mc'Donald".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "O'Brien Mc'Donald");
    }

    /// Names with newlines.
    #[test]
    fn newlines_in_name() {
        let p = UserProfile {
            first_name: Some("Line1\nLine2".to_string()),
            last_name: Some("Last".to_string()),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "Line1\nLine2 Last");
    }

    /// Rapid state transitions.
    #[test]
    fn rapid_state_changes() {
        let mut m = AuthManager::new();
        for i in 0..1000 {
            if i % 3 == 0 {
                m.set_state(AuthState::NotAuthenticated);
            } else if i % 3 == 1 {
                m.set_state(AuthState::AwaitingUserAuth {
                    verification_uri: format!("https://link.tidal.com/{}", i),
                    user_code: format!("CODE{}", i),
                });
            } else {
                m.set_state(AuthState::Authenticated {
                    profile: UserProfile {
                        username: Some(format!("user{}", i)),
                        ..Default::default()
                    },
                });
            }
        }
        // Last iteration: 999 % 3 == 0 → NotAuthenticated
        assert_eq!(*m.state(), AuthState::NotAuthenticated);
    }

    /// display_name is deterministic across multiple calls.
    #[test]
    fn display_name_is_deterministic() {
        let p = UserProfile {
            first_name: Some("Consistent".to_string()),
            last_name: Some("Name".to_string()),
            ..Default::default()
        };
        let name1 = p.display_name();
        let name2 = p.display_name();
        let name3 = p.display_name();
        assert_eq!(name1, name2);
        assert_eq!(name2, name3);
    }

    /// initials is deterministic across multiple calls.
    #[test]
    fn initials_is_deterministic() {
        let p = UserProfile {
            first_name: Some("Stable".to_string()),
            ..Default::default()
        };
        let i1 = p.initials();
        let i2 = p.initials();
        assert_eq!(i1, i2);
    }
}
