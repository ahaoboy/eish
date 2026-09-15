//! Error type shared by the library and the CLI.

use std::path::PathBuf;

use thiserror::Error;

use crate::shell::Shell;

/// Convenience alias for [`Result`] using [`enum@Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Every way in which `eish` can fail.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The `--shell` value could not be recognised.
    #[error("unknown shell `{input}` (expected one of: {expected})")]
    UnknownShell {
        /// The value that was supplied.
        input: String,
        /// Comma separated list of accepted values.
        expected: &'static str,
    },

    /// The `--proxy` value could not be recognised.
    #[error("unknown proxy `{input}` (expected one of: {expected})")]
    UnknownProxy {
        /// The value that was supplied.
        input: String,
        /// Comma separated list of accepted values.
        expected: &'static str,
    },

    /// The positional `owner/repo[@tag]` argument could not be parsed.
    #[error(
        "invalid repository spec `{input}` (expected `<owner>/<repo>` or `<owner>/<repo>@<tag>`)"
    )]
    InvalidSpec {
        /// The value that was supplied.
        input: String,
    },

    /// The release requested by the user does not exist (or is not published).
    #[error("no release found for {repo} (tag: {tag})")]
    ReleaseNotFound {
        /// `owner/repo`.
        repo: String,
        /// The requested tag, or `latest`.
        tag: String,
    },

    /// None of the files attached to the release look like prebuilt binaries.
    #[error(
        "release {repo} (tag: {tag}) has no assets matching a known Rust target triple; \
         use `--target`/`--binary` or check that the release publishes binaries"
    )]
    NoAssets {
        /// `owner/repo`.
        repo: String,
        /// The resolved tag.
        tag: String,
    },

    /// A proxy was selected that cannot serve GitHub release assets.
    #[error(
        "proxy `{proxy}` cannot serve GitHub release assets; use `github`, `gh-proxy` or `xget`, \
         or switch to `--type file`"
    )]
    UnsupportedProxy {
        /// The offending proxy name.
        proxy: &'static str,
    },

    /// A target triple was requested that the release does not publish.
    #[error("no asset for target `{target}`; available targets: {available}")]
    UnknownTarget {
        /// The target triple that was requested.
        target: String,
        /// Space separated list of targets that *are* available.
        available: String,
    },

    /// The GitHub API answered, but with an error status we can explain.
    #[error("GitHub API request to {url} failed with HTTP {status}: {hint}")]
    ApiStatus {
        /// The status code that was returned.
        status: u16,
        /// The URL that was requested.
        url: String,
        /// A short explanation of the most likely cause.
        hint: &'static str,
    },

    /// The GitHub API request failed.
    #[error("failed to query {url}: {source}")]
    Http {
        /// The URL that was requested.
        url: String,
        /// The underlying transport error.
        #[source]
        source: Box<ureq::Error>,
    },

    /// The GitHub API returned something we could not decode.
    #[error("failed to decode GitHub API response: {source}")]
    Decode {
        /// The underlying JSON error.
        #[source]
        source: serde_json::Error,
    },

    /// A JSON file could not be read.
    #[error("failed to read {path}: {source}")]
    ReadFile {
        /// The path that was read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The generated script could not be written to disk.
    #[error("failed to write {path}: {source}")]
    WriteFile {
        /// The path that was written.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Rendering the installer template failed.
    #[error("failed to render the {shell} installer: {source}")]
    Render {
        /// The shell whose template failed.
        shell: Shell,
        /// The underlying template error.
        #[source]
        source: minijinja::Error,
    },
}
