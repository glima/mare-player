// SPDX-License-Identifier: MIT

//! Authentication manager for TIDAL OAuth PKCE flow.
//!
//! This module handles:
//! - Secure credential storage using the system keyring
//! - Session token refresh and persistence
//!
//! The flow itself lives in [`client`](super::client). TIDAL only serves the
//! hi-res tier to PKCE clients (see [`client_identity`](super::client_identity)),
//! and PKCE has no device code to poll: the user signs in through the browser
//! and hands the resulting redirect URL back to the app.

use keyring::Entry;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

const SERVICE_NAME: &str = "cosmic-applet-mare";
const SESSION_KEY: &str = "tidal_session";

/// Stored authentication tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCredentials {
    /// The serialized TidalClient session JSON
    pub session_json: String,
    /// Timestamp when the session was stored
    pub stored_at: chrono::DateTime<chrono::Utc>,
    /// User ID if available
    pub user_id: Option<String>,
    /// Username if available
    pub username: Option<String>,
}

/// A pending PKCE login: the TIDAL page the user has to complete.
///
/// After signing in, TIDAL redirects the browser to whichever redirect URI we
/// asked for, carrying the authorization code that
/// [`TidalAppClient::complete_login`](super::client::TidalAppClient::complete_login)
/// exchanges for tokens. Which one we can ask for depends on the desktop — see
/// [`login_uri`](super::login_uri).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginRequest {
    /// `https://login.tidal.com/authorize?…` — open this in a browser.
    pub authorize_url: String,
    /// Whether the browser will hand the authorization code back to us.
    ///
    /// True when this desktop routes `tidal://` to Maré, in which case signing
    /// in is all the user has to do. False when we had to ask for the https
    /// redirect instead, which lands in the browser and has to be copied.
    pub delivers_itself: bool,
}

/// Authentication state
/// Profile information for the authenticated user
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserProfile {
    /// TIDAL username (login handle)
    pub username: Option<String>,
    /// User's first name
    pub first_name: Option<String>,
    /// User's last name
    pub last_name: Option<String>,
    /// Full display name (from TIDAL's `fullName` field)
    pub full_name: Option<String>,
    /// Nickname / display name chosen by the user
    pub nickname: Option<String>,
    /// Email address
    pub email: Option<String>,
    /// Profile picture URL (fetched separately from TIDAL API)
    pub picture_url: Option<String>,
    /// Subscription plan label (e.g. "HiFi Plus", "HiFi", "Free")
    /// Fetched from /v1/users/{id}/subscription
    pub subscription_plan: Option<String>,
}

impl UserProfile {
    /// Best display name, checked in order:
    /// 1. "First Last" (if both non-empty)
    /// 2. full_name (TIDAL's `fullName` field)
    /// 3. nickname
    /// 4. first_name alone
    /// 5. username (if it doesn't look like an email)
    /// 6. email
    /// 7. "Signed in"
    pub fn display_name(&self) -> String {
        // Try "First Last"
        match (&self.first_name, &self.last_name) {
            (Some(f), Some(l)) if !f.is_empty() && !l.is_empty() => {
                return format!("{} {}", f, l);
            }
            _ => {}
        }
        // Try full_name from TIDAL
        if let Some(name) = &self.full_name
            && !name.is_empty()
        {
            return name.clone();
        }
        // Try nickname
        if let Some(nick) = &self.nickname
            && !nick.is_empty()
        {
            return nick.clone();
        }
        // Try first_name alone
        if let Some(f) = &self.first_name
            && !f.is_empty()
        {
            return f.clone();
        }
        // Fall back to username (skip if it looks like an email)
        if let Some(u) = &self.username
            && !u.is_empty()
            && !u.contains('@')
        {
            return u.clone();
        }
        // Fall back to email
        if let Some(e) = &self.email
            && !e.is_empty()
        {
            return e.clone();
        }
        "Signed in".to_string()
    }

    /// First letter of the display name (for avatar fallback)
    pub fn initials(&self) -> String {
        let name = self.display_name();
        name.chars().next().unwrap_or('?').to_uppercase().to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    /// Not authenticated, need to start login flow
    NotAuthenticated,
    /// Waiting for the user to finish signing in in their browser and paste
    /// the redirect URL back.
    AwaitingUserAuth {
        /// The TIDAL authorize URL the user was sent to.
        authorize_url: String,
    },
    /// Successfully authenticated
    Authenticated {
        /// Full user profile (name, email, picture, etc.)
        profile: UserProfile,
    },
    /// Authentication failed with error
    Failed(String),
}

/// Manages authentication flow and credential storage
pub struct AuthManager {
    /// Current authentication state
    state: AuthState,
}

impl Default for AuthManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthManager {
    /// Create a new AuthManager
    pub fn new() -> Self {
        Self { state: AuthState::NotAuthenticated }
    }

    /// Get the current authentication state
    pub fn state(&self) -> &AuthState {
        &self.state
    }

    /// Set the authentication state
    pub fn set_state(&mut self, state: AuthState) {
        self.state = state;
    }

    /// Store credentials securely in the system keyring
    pub fn store_credentials(credentials: &StoredCredentials) -> Result<(), String> {
        let entry = Entry::new(SERVICE_NAME, SESSION_KEY).map_err(|e| format!("Failed to create keyring entry: {}", e))?;

        let json = serde_json::to_string(credentials).map_err(|e| format!("Failed to serialize credentials: {}", e))?;

        entry.set_password(&json).map_err(|e| format!("Failed to store credentials: {}", e))?;

        info!("Credentials stored successfully in keyring");
        Ok(())
    }

    /// Load credentials from the system keyring
    pub fn load_credentials() -> Result<Option<StoredCredentials>, String> {
        let entry = match Entry::new(SERVICE_NAME, SESSION_KEY) {
            Ok(e) => e,
            Err(e) => {
                warn!("Failed to create keyring entry: {}", e);
                return Ok(None);
            }
        };

        match entry.get_password() {
            Ok(json) => {
                let credentials: StoredCredentials =
                    serde_json::from_str(&json).map_err(|e| format!("Failed to deserialize credentials: {}", e))?;
                debug!("Loaded credentials from keyring");
                Ok(Some(credentials))
            }
            Err(keyring::Error::NoEntry) => {
                debug!("No credentials found in keyring");
                Ok(None)
            }
            Err(e) => {
                error!("Failed to load credentials: {}", e);
                Err(format!("Failed to load credentials: {}", e))
            }
        }
    }

    /// Delete stored credentials from the keyring
    pub fn delete_credentials() -> Result<(), String> {
        let entry = Entry::new(SERVICE_NAME, SESSION_KEY).map_err(|e| format!("Failed to create keyring entry: {}", e))?;

        match entry.delete_credential() {
            Ok(()) => {
                info!("Credentials deleted from keyring");
                Ok(())
            }
            Err(keyring::Error::NoEntry) => {
                debug!("No credentials to delete");
                Ok(())
            }
            Err(e) => {
                error!("Failed to delete credentials: {}", e);
                Err(format!("Failed to delete credentials: {}", e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_state_default() {
        let manager = AuthManager::new();
        assert_eq!(*manager.state(), AuthState::NotAuthenticated);
    }
}
