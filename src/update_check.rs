// SPDX-License-Identifier: GPL-3.0-only

//! Whether a newer Maré Player release is out.
//!
//! Asked once per run, at startup: the project's latest published release on
//! GitHub (drafts and pre-releases are never "latest") against this build's
//! version. A failed check is no news, and shows nothing.

use std::time::Duration;

use serde::Deserialize;

/// A release newer than the one running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    /// Its version, without the tag's leading `v` ("0.4.0").
    pub version: String,
    /// Its release page, with the release notes.
    pub url: String,
}

/// The parts of GitHub's release object we read.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
}

/// GitHub's API URL for the project's latest release, from the crate's
/// `repository`. `None` if the project isn't on GitHub.
fn latest_release_url(repository: &str) -> Option<String> {
    let path = repository.strip_prefix("https://github.com/")?.trim_end_matches('/');
    (!path.is_empty()).then(|| format!("https://api.github.com/repos/{path}/releases/latest"))
}

/// The latest release, if it is newer than this build.
pub async fn check() -> Result<Option<AvailableUpdate>, String> {
    let url = latest_release_url(env!("CARGO_PKG_REPOSITORY")).ok_or("the project is not on GitHub")?;
    let response = reqwest::Client::new()
        .get(url)
        .header(reqwest::header::USER_AGENT, concat!("mare-player/", env!("CARGO_PKG_VERSION")))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?;
    let body = response.text().await.map_err(|e| e.to_string())?;
    let release: Release = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    Ok(newer_than(env!("CARGO_PKG_VERSION"), release))
}

/// `release`, if it is newer than the `current` version.
fn newer_than(current: &str, release: Release) -> Option<AvailableUpdate> {
    (parse_version(&release.tag_name)? > parse_version(current)?)
        .then(|| AvailableUpdate { version: release.tag_name.trim_start_matches('v').to_string(), url: release.html_url })
}

/// `1.2.3` or `v1.2.3` as numbers. Anything else, pre-release suffixes
/// included, is not a version this compares.
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim_start_matches('v').split('.').map(|part| part.parse::<u64>().ok());
    let parsed = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(parsed)
}

#[cfg(test)]
mod tests {
    use super::{AvailableUpdate, Release, latest_release_url, newer_than, parse_version};

    fn release(tag: &str) -> Release {
        Release { tag_name: tag.into(), html_url: format!("https://github.com/glima/mare-player/releases/tag/{tag}") }
    }

    #[test]
    fn the_api_url_comes_from_the_repository() {
        assert_eq!(
            latest_release_url("https://github.com/glima/mare-player").as_deref(),
            Some("https://api.github.com/repos/glima/mare-player/releases/latest")
        );
        assert!(latest_release_url(env!("CARGO_PKG_REPOSITORY")).is_some());
        assert_eq!(latest_release_url("https://codeberg.org/someone/thing"), None);
        assert_eq!(latest_release_url("https://github.com/"), None);
    }

    #[test]
    fn only_a_higher_version_is_an_update() {
        assert_eq!(
            newer_than("0.3.9", release("v0.3.10")),
            Some(AvailableUpdate {
                version: "0.3.10".into(),
                url: "https://github.com/glima/mare-player/releases/tag/v0.3.10".into()
            })
        );
        assert!(newer_than("0.3.9", release("v0.4.0")).is_some());
        assert!(newer_than("0.3.9", release("1.0.0")).is_some());
        assert_eq!(newer_than("0.3.9", release("v0.3.9")), None);
        assert_eq!(newer_than("0.3.10", release("v0.3.9")), None, "numbers, not text");
        assert_eq!(newer_than("0.3.9", release("nightly")), None);
        assert_eq!(newer_than("0.4.0-rc1", release("v0.4.0")), None, "a pre-release build isn't compared");
    }

    #[test]
    fn versions_are_three_numbers() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("1.2.3-beta"), None);
    }
}
