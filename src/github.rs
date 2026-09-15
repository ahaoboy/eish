//! A very small GitHub Releases API client.
//!
//! Only the handful of fields `eish` needs are deserialised, which keeps the
//! request cheap and the code easy to reason about.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::token::{Source, Token};

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
///
/// The client authenticates with, in order of preference, the token passed to
/// [`Client::with_token`] and a token discovered by [`Token::detect`]. Discovery
/// is what keeps an anonymous run from dying on the 60-requests-per-hour rate
/// limit; see [`crate::token`].
#[derive(Debug, Clone)]
pub struct Client {
    api_base: String,
    token: Option<Token>,
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
            token: None,
            agent: ureq::Agent::new_with_config(config),
        }
    }

    /// Point the client at a different API root (GitHub Enterprise, a mirror, …).
    #[must_use]
    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = api_base.into().trim_end_matches('/').to_string();
        self
    }

    /// Authenticate with an explicit token, bypassing discovery.
    ///
    /// An empty or blank value is ignored, leaving discovery enabled.
    #[must_use]
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token.and_then(Token::from_flag);
        self
    }

    /// The token to authenticate with, if any.
    ///
    /// A token supplied through [`Client::with_token`] always wins. Otherwise
    /// discovery runs once per process and the result is only used when it may
    /// be sent to this client's [`Client::api_base`].
    fn auth_token(&self) -> Option<&Token> {
        match &self.token {
            Some(token) => Some(token),
            None => Token::detect().filter(|token| token.may_send_to(&self.api_base)),
        }
    }

    /// Which credential the client will authenticate with, if any.
    ///
    /// Runs discovery on first call, so this is meant for diagnostics rather
    /// than for deciding whether to request a token.
    pub fn credential_source(&self) -> Option<Source> {
        self.auth_token().map(Token::source)
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

        let token = self.auth_token();
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {}", token.value()));
        }

        let mut response = request
            .call()
            .map_err(|source| map_request_error(&url, tag, token, source))?;

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

/// Turn a transport error into something the user can act on.
///
/// `token` is the credential the request actually carried, so the hint can name
/// the missing piece instead of guessing: with a token the limit is exhausted,
/// without one the fix is to supply credentials at all.
fn map_request_error(
    url: &str,
    tag: Option<&str>,
    token: Option<&Token>,
    source: ureq::Error,
) -> Error {
    let ureq::Error::StatusCode(status @ (404 | 403 | 401 | 429)) = source else {
        return Error::Http {
            url: url.to_string(),
            source: Box::new(source),
        };
    };

    // GitHub answers 404 rather than 403 for a private repository the caller
    // cannot see, so an unauthenticated miss is worth flagging as a possible
    // permission problem rather than "this release does not exist".
    if status == 404 && token.is_none() {
        return Error::ApiStatus {
            status,
            url: url.to_string(),
            hint: "not found; if the repository is private, \
                   pass --token, set GITHUB_TOKEN, or run `gh auth login`",
        };
    }

    let repo = url
        .split("/repos/")
        .nth(1)
        .map(|rest| rest.split('/').take(2).collect::<Vec<_>>().join("/"))
        .unwrap_or_default();

    if status == 404 {
        return Error::ReleaseNotFound {
            repo,
            tag: tag.unwrap_or("latest").to_string(),
        };
    }

    let hint = match (status, token.map(Token::source)) {
        (401, _) => "the credentials were rejected; refresh the token or run `gh auth login`",
        (_, Some(source)) => match source {
            Source::Flag => "--token was rejected or has no access to this repository",
            Source::Environment => "$GITHUB_TOKEN was rejected or has no access to this repository",
            Source::GhCli => {
                "the token from `gh auth token` has no access to this repository; \
                 run `gh auth refresh` or pass --token"
            }
            Source::GitCredential => {
                "the token from git credentials has no access to this repository; \
                 run `gh auth login` or pass --token"
            }
        },
        (403 | 429, None) => {
            "the anonymous rate limit (60 requests/hour) is exhausted and no credentials \
             were found; pass --token, set GITHUB_TOKEN, or run `gh auth login`"
        }
        (_, None) => "the request was refused; pass --token or set GITHUB_TOKEN",
    };

    Error::ApiStatus {
        status,
        url: url.to_string(),
        hint,
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
            Some(&Token::new("t", Source::Flag)),
            ureq::Error::StatusCode(404),
        );
        assert!(matches!(error, Error::ReleaseNotFound { .. }));
        assert!(error.to_string().contains("foo/bar"));
    }

    #[test]
    fn an_unauthenticated_404_mentions_private_repositories() {
        let error = map_request_error(
            "https://api.github.com/repos/foo/bar/releases/latest",
            None,
            None,
            ureq::Error::StatusCode(404),
        );
        // A missing token is the likelier explanation than a missing release.
        assert!(matches!(error, Error::ApiStatus { status: 404, .. }));
        assert!(error.to_string().contains("private"), "{error}");
    }

    #[test]
    fn rate_limit_hint_depends_on_whether_a_token_was_used() {
        let anonymous = map_request_error(
            "https://api.github.com/repos/foo/bar/releases/latest",
            None,
            None,
            ureq::Error::StatusCode(403),
        );
        assert!(anonymous.to_string().contains("rate limit"), "{anonymous}");

        let authenticated = map_request_error(
            "https://api.github.com/repos/foo/bar/releases/latest",
            None,
            Some(&Token::new("t", Source::Flag)),
            ureq::Error::StatusCode(403),
        );
        assert!(!authenticated.to_string().contains("rate limit"), "{authenticated}");
        assert!(authenticated.to_string().contains("--token"), "{authenticated}");

        // The hint names the helper that supplied the token.
        let from_gh = map_request_error(
            "https://api.github.com/repos/foo/bar/releases/latest",
            None,
            Some(&Token::new("t", Source::GhCli)),
            ureq::Error::StatusCode(403),
        );
        assert!(from_gh.to_string().contains("gh auth"), "{from_gh}");
    }

    #[test]
    fn a_rejected_token_is_reported_as_such() {
        let error = map_request_error(
            "https://api.github.com/repos/foo/bar/releases/latest",
            None,
            Some(&Token::new("t", Source::Environment)),
            ureq::Error::StatusCode(401),
        );
        assert!(error.to_string().contains("rejected"), "{error}");
    }
}
