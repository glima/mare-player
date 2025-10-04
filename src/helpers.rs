// SPDX-License-Identifier: MIT

//! Utility helper functions for Maré Player.
//!
//! This module contains shared utility functions used across the application,
//! including clipboard operations, URL handling, and text manipulation.

/// Generate a song.link URL for sharing
///
/// Takes a TIDAL URL and returns a universal song.link URL that works
/// across multiple streaming platforms.
pub async fn generate_songlink(tidal_url: &str) -> Result<String, String> {
    let client = reqwest::Client::new();

    // Build the URL with query parameters
    let url = format!(
        "https://api.song.link/v1-alpha.1/links?url={}&userCountry=US",
        urlencoding::encode(tidal_url)
    );

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("API returned status: {}", response.status()));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    let json: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Failed to parse JSON: {}", e))?;

    json.get("pageUrl")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No pageUrl in response".to_string())
}

/// Copy text to clipboard using system tools
///
/// Tries multiple clipboard tools in order of preference:
/// 1. wl-copy (Wayland)
/// 2. xclip (X11)
/// 3. xsel (X11 fallback)
pub async fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use tokio::process::Command;

    // Try wl-copy first (Wayland)
    let wl_result = Command::new("wl-copy").arg(text).output().await;

    if let Ok(output) = wl_result
        && output.status.success()
    {
        return Ok(());
    }

    // Fall back to xclip (X11)
    let xclip_result = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(std::process::Stdio::piped())
        .spawn();

    if let Ok(mut child) = xclip_result
        && let Some(stdin) = child.stdin.as_mut()
    {
        use tokio::io::AsyncWriteExt;
        if stdin.write_all(text.as_bytes()).await.is_ok()
            && let Ok(status) = child.wait().await
            && status.success()
        {
            return Ok(());
        }
    }

    // Fall back to xsel
    let xsel_result = Command::new("xsel")
        .args(["--clipboard", "--input"])
        .stdin(std::process::Stdio::piped())
        .spawn();

    if let Ok(mut child) = xsel_result
        && let Some(stdin) = child.stdin.as_mut()
    {
        use tokio::io::AsyncWriteExt;
        if stdin.write_all(text.as_bytes()).await.is_ok()
            && let Ok(status) = child.wait().await
            && status.success()
        {
            return Ok(());
        }
    }

    Err("No clipboard tool available (tried wl-copy, xclip, xsel)".to_string())
}

/// Open a URL in the default browser via the XDG Desktop Portal (`OpenURI`
/// over D-Bus).  This is near-instant because it talks directly to the
/// portal daemon (e.g. `xdg-desktop-portal-cosmic`) instead of spawning the
/// `xdg-open` shell script, which can take several seconds on some setups.
///
/// Falls back to `xdg-open` if the portal call fails.
pub async fn open_in_browser(url: &str) -> Result<(), String> {
    // Try the XDG Desktop Portal first — fast, no subprocess.
    match open_uri_via_portal(url).await {
        Ok(()) => return Ok(()),
        Err(e) => {
            tracing::debug!("Portal OpenURI failed, falling back to xdg-open: {}", e);
        }
    }

    // Fallback: spawn xdg-open.
    use tokio::process::Command;

    let result = Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("Failed to open browser: {}", e)),
    }
}

/// Call `org.freedesktop.portal.OpenURI.OpenURI` on the session bus.
async fn open_uri_via_portal(url: &str) -> Result<(), String> {
    use std::collections::HashMap;
    use zbus::Connection;
    use zbus::zvariant::Value;

    let connection = Connection::session()
        .await
        .map_err(|e| format!("D-Bus session connection failed: {}", e))?;

    // OpenURI(parent_window: s, uri: s, options: a{sv}) → o
    let _response: zbus::zvariant::OwnedObjectPath = connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.OpenURI"),
            "OpenURI",
            &("", url, HashMap::<&str, Value<'_>>::new()),
        )
        .await
        .map_err(|e| format!("Portal OpenURI call failed: {}", e))?
        .body()
        .deserialize()
        .map_err(|e| format!("Portal OpenURI response parse failed: {}", e))?;

    Ok(())
}

/// Compute the maximum number of characters for description text (album
/// reviews, artist bios) based on the current window width.
///
/// At the typical applet popup width (~360 px) this returns 300 characters.
/// The limit scales linearly so wider windows show proportionally more text,
/// with a floor of 150 characters for very narrow windows.
pub fn max_description_chars(window_width: f32) -> usize {
    // Baseline: 300 chars at 360 px.  ~0.83 chars per pixel.
    const BASE_WIDTH: f32 = 360.0;
    const BASE_CHARS: f32 = 300.0;
    const MIN_CHARS: usize = 150;

    let effective_width = if window_width > 0.0 {
        window_width
    } else {
        BASE_WIDTH
    };

    let scaled = (effective_width / BASE_WIDTH * BASE_CHARS).round() as usize;
    scaled.max(MIN_CHARS)
}

/// Format a duration in seconds as a human-readable time string.
///
/// - Values under one hour are shown as `M:SS` (e.g. `3:05`).
/// - Values of one hour or more are shown as `H:MM:SS` (e.g. `1:02:07`).
/// - Negative values are clamped to `0:00`.
pub fn format_seconds(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_seconds_short() {
        assert_eq!(format_seconds(0.0), "0:00");
        assert_eq!(format_seconds(5.9), "0:05");
        assert_eq!(format_seconds(65.0), "1:05");
        assert_eq!(format_seconds(185.0), "3:05");
    }

    #[test]
    fn test_format_seconds_hour() {
        assert_eq!(format_seconds(3600.0), "1:00:00");
        assert_eq!(format_seconds(3727.0), "1:02:07");
    }

    #[test]
    fn test_format_seconds_negative() {
        assert_eq!(format_seconds(-10.0), "0:00");
    }

    #[test]
    fn test_max_description_chars_baseline() {
        // At the baseline popup width we get 300 chars.
        assert_eq!(max_description_chars(360.0), 300);
    }

    #[test]
    fn test_max_description_chars_wider() {
        // Wider window → more chars.
        assert!(max_description_chars(720.0) > 300);
    }

    #[test]
    fn test_max_description_chars_narrow() {
        // Very narrow → clamps to minimum.
        assert_eq!(max_description_chars(50.0), 150);
    }

    #[test]
    fn test_max_description_chars_zero_width() {
        // Zero / uninitialised width falls back to baseline.
        assert_eq!(max_description_chars(0.0), 300);
    }
}
