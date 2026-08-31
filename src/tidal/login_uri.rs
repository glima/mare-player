// SPDX-License-Identifier: MIT

//! Receiving the PKCE sign-in callback as a `tidal://` URI.
//!
//! TIDAL's registration for the client we authenticate as
//! ([`client_identity`](super::client_identity)) accepts two redirect URIs:
//! `https://tidal.com/android/login/auth`, which lands in the browser, and
//! `tidal://login/auth`, which the desktop hands straight to us. Every other
//! scheme tried against the authorize endpoint — `http://localhost:…`,
//! `http://127.0.0.1:…`, `com.aspiro.tidal://…` — answers with error 11102.
//!
//! So when Maré is registered as the handler for `x-scheme-handler/tidal`, the
//! browser delivers the authorization code to the app and signing in costs the
//! user nothing beyond logging in. When it isn't, we fall back to the https
//! redirect and the user copies the address of the page that fails to load.
//!
//! The browser launches `cosmic-applet-mare <uri>`, a *second* process; the
//! code verifier lives in the one already running. That process therefore
//! forwards the URI over the session bus ([`forward_to_running_instance`]) and
//! exits, and the running app picks it up from the service below.

use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use zbus::{connection::Builder, interface};

/// URI scheme TIDAL redirects to, and that we register as a handler for.
pub const CALLBACK_SCHEME: &str = "tidal";

/// The redirect URI to ask TIDAL for when we can receive it ourselves.
pub const CALLBACK_REDIRECT_URI: &str = "tidal://login/auth";

/// The redirect URI to fall back to: reaches the browser, not us.
pub const PASTE_REDIRECT_URI: &str = "https://tidal.com/android/login/auth";

/// Bus name the running instance owns so a callback process can find it.
const BUS_NAME: &str = "io.github.cosmic_applet_mare.Login";
const OBJECT_PATH: &str = "/io/github/cosmic_applet_mare/Login";

/// Keeps the service's bus connection alive for the life of the process.
static CONNECTION: std::sync::OnceLock<zbus::Connection> = std::sync::OnceLock::new();

/// The D-Bus object a callback process talks to.
struct LoginCallback {
    uri_tx: mpsc::UnboundedSender<String>,
}

#[interface(name = "io.github.cosmic_applet_mare.Login")]
impl LoginCallback {
    /// Hand a `tidal://login/auth?code=…` URI to the running app.
    async fn complete(&self, uri: String) {
        debug!("received login callback URI over D-Bus");
        if self.uri_tx.send(uri).is_err() {
            warn!("login callback arrived but the app is no longer listening");
        }
    }
}

/// Start the service that receives sign-in callbacks.
///
/// Returns the receiving end of the URI channel. Failure is not fatal: the
/// login view keeps offering the paste fallback.
pub async fn start_login_uri_service() -> Result<mpsc::UnboundedReceiver<String>, String> {
    let (uri_tx, uri_rx) = mpsc::unbounded_channel();

    let connection = Builder::session()
        .map_err(|e| format!("Failed to create session bus builder: {e}"))?
        .name(BUS_NAME)
        .map_err(|e| format!("Failed to request bus name: {e}"))?
        .serve_at(OBJECT_PATH, LoginCallback { uri_tx })
        .map_err(|e| format!("Failed to serve LoginCallback: {e}"))?
        .build()
        .await
        .map_err(|e| format!("Failed to build D-Bus connection: {e}"))?;

    // The bus name is held for as long as the connection lives, and this
    // service is wanted for the life of the process.
    let _ = CONNECTION.set(connection);

    info!("Login callback service registered at {BUS_NAME} on session bus");
    Ok(uri_rx)
}

/// Hand `uri` to the already-running Maré and return.
///
/// Called from `main` when the browser launches us with the callback URI. The
/// exchange needs the PKCE code verifier held by the process that started the
/// login, so there is nothing useful this process can do alone.
pub async fn forward_to_running_instance(uri: &str) -> Result<(), String> {
    let connection = zbus::Connection::session().await.map_err(|e| format!("no session bus: {e}"))?;

    connection
        .call_method(Some(BUS_NAME), OBJECT_PATH, Some("io.github.cosmic_applet_mare.Login"), "Complete", &(uri))
        .await
        .map_err(|e| format!("no running Maré Player to hand the sign-in to: {e}"))?;

    Ok(())
}

/// Whether this desktop will route `tidal://` URIs to Maré.
///
/// Asks `xdg-mime`, which is what the browser's portal consults. A missing
/// `xdg-utils`, or an association pointing at something else (TIDAL's own app,
/// say), counts as "no" — in which case the login view asks for the URL by
/// hand rather than promising an automatic return.
pub fn handler_is_registered() -> bool {
    let scheme = format!("x-scheme-handler/{CALLBACK_SCHEME}");
    let Ok(out) = std::process::Command::new("xdg-mime").args(["query", "default", &scheme]).output() else {
        debug!("xdg-mime unavailable; assuming tidal:// is not routed to us");
        return false;
    };
    let handler = String::from_utf8_lossy(&out.stdout);
    let handler = handler.trim();
    let ours = handler.starts_with("io.github.cosmic-applet-mare");
    if !ours {
        // Worth saying out loud: this is the whole difference between signing
        // in with one click and copying a URL, and the cause is never visible
        // from inside the app.
        info!(
            handler = if handler.is_empty() { "(none)" } else { handler },
            "tidal:// is not routed to Maré, so the sign-in will ask for the URL by hand; \
             claim it with: xdg-mime default io.github.cosmic-applet-mare.desktop x-scheme-handler/tidal"
        );
    }
    ours
}

/// The redirect URI to ask TIDAL for, given how this desktop is set up.
pub fn redirect_uri() -> &'static str {
    if handler_is_registered() { CALLBACK_REDIRECT_URI } else { PASTE_REDIRECT_URI }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uris_are_the_two_tidal_accepts() {
        // Both are registered for our client id; every other redirect the
        // authorize endpoint has been tried with answers with error 11102.
        assert_eq!(CALLBACK_REDIRECT_URI, "tidal://login/auth");
        assert_eq!(PASTE_REDIRECT_URI, "https://tidal.com/android/login/auth");
    }

    #[test]
    fn callback_scheme_matches_the_callback_redirect() {
        assert!(CALLBACK_REDIRECT_URI.starts_with(&format!("{CALLBACK_SCHEME}://")));
    }
}
