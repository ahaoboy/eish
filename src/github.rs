//! A very small GitHub Releases API client.
//!
//! Only the handful of fields `eish` needs are deserialised, which keeps the
//! request cheap and the code easy to reason about.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::error::{Error, Result};

/// `User-Agent` sent with every request. GitHub rejects requests without one.
pub const USER_AGENT: &str = concat!("eish/", env!("CARGO_PKG_VERSION"));

/// The default GitHub REST API endpoint.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";

/// A published release.
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    /// The tag the release was published under.
    #[serde(default)]
    pub tag_name: Option<String>,
    /// The human readable release title.
    #[serde(default)]
    pub name: Option<String>,
    /// The files attached to the release.
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    /// The tag name, or `fallback` when the API did not report one.
    pub fn tag(&self, fallback: &str) -> String {
        self.tag_name
            .as_deref()
            .filter(|tag| !tag.is_empty())
            .unwrap_or(fallback)
            .to_string()
    }
}

/// A single file attached to a release.
#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    /// File name, e.g. `mytool-x86_64-unknown-linux-musl.tar.gz`.
    pub name: String,
    /// File size in bytes.
    #[serde(default)]
    pub size: u64,
}

/// Blocking client for the GitHub REST API.
#[derive(Debug, Clone)]
pub struct Client {
    api_base: String,
    token: Option<String>,
    agent: ureq::Agent,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Create a client talking to `https://api.github.com`.
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(USER_AGENT)
            .build();

        Self {
            api_base: DEFAULT_API_BASE.to_string(),
            token: std::env::var("GITHUB_TOKEN")
                .ok()
                .filter(|token| !token.is_empty()),
            agent: ureq::Agent::new_with_config(config),
        }
    }

    /// Point the client at a different API root (GitHub Enterprise, a mirror, …).
    #[must_use]
    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = api_base.into().trim_end_matches('/').to_string();
        self
    }

    /// Attach a personal access token to lift the anonymous rate limit.
    #[must_use]
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token.filter(|token| !token.is_empty());
        self
    }

    /// Fetch `latest`, or the release tagged `tag`.
    pub fn release(&self, owner: &str, repo: &str, tag: Option<&str>) -> Result<Release> {
        let repo_slug = format!("{owner}/{repo}");
        let url = match tag {
            None | Some("latest") => {
                format!("{}/repos/{repo_slug}/releases/latest", self.api_base)
            }
            Some(tag) => format!("{}/repos/{repo_slug}/releases/tags/{tag}", self.api_base),
        };

        let mut request = self
            .agent
            .get(&url)
            .header("Accept", "application/vnd.github+json");

        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }

        let mut response = request
            .call()
            .map_err(|source| map_request_error(&url, tag, source))?;

        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|source| Error::Http {
                url: url.clone(),
                source: Box::new(source),
            })?;

        serde_json::from_str(&body).map_err(|source| Error::Decode { source })
    }
}

/// Read a release object from a JSON file.
///
/// Accepts anything shaped like the GitHub REST API response (for example the
/// output of `gh release view --json tagName,name,assets`), which makes it
/// possible to generate installers offline.
pub fn release_from_json_file(path: impl AsRef<Path>) -> Result<Release> {
    let path = path.as_ref();
    let data = std::fs::read_to_string(path).map_err(|source| Error::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&data).map_err(|source| Error::Decode { source })
}

fn map_request_error(url: &str, tag: Option<&str>, source: ureq::Error) -> Error {
    match source {
        ureq::Error::StatusCode(status @ (404 | 403 | 401 | 429)) => {
            let repo = url
                .split("/repos/")
                .nth(1)
                .map(|rest| rest.split('/').take(2).collect::<Vec<_>>().join("/"))
                .unwrap_or_default();

            if status == 404 {
                Error::ReleaseNotFound {
                    repo,
                    tag: tag.unwrap_or("latest").to_string(),
                }
            } else {
                Error::ApiStatus {
                    status,
                    url: url.to_string(),
                    hint: if matches!(status, 403 | 429) {
                        "the unauthenticated rate limit is probably exhausted; \
                         pass --token or set GITHUB_TOKEN"
                    } else {
                        "the supplied GitHub token was rejected"
                    },
                }
            }
        }
        source => Error::Http {
            url: url.to_string(),
            source: Box::new(source),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialises_a_release() {
        let json = r#"{
            "tag_name": "v1.2.3",
            "name": "Release 1.2.3",
            "assets": [
                {"name": "tool-x86_64-unknown-linux-musl.tar.gz", "size": 100}
            ]
        }"#;

        let release: Release = serde_json::from_str(json).unwrap();
        assert_eq!(release.tag("latest"), "v1.2.3");
        assert_eq!(release.assets.len(), 1);
        assert_eq!(
            release.assets[0].name,
            "tool-x86_64-unknown-linux-musl.tar.gz"
        );
        assert_eq!(release.assets[0].size, 100);
    }

    #[test]
    fn tolerates_partial_releases() {
        let release: Release = serde_json::from_str("{}").unwrap();
        assert_eq!(release.tag("latest"), "latest");
        assert!(release.assets.is_empty());
    }

    #[test]
    fn maps_404_to_release_not_found() {
        let error = map_request_error(
            "https://api.github.com/repos/foo/bar/releases/tags/v1",
            Some("v1"),
            ureq::Error::StatusCode(404),
        );
        assert!(matches!(error, Error::ReleaseNotFound { .. }));
        assert!(error.to_string().contains("foo/bar"));
    }
}
