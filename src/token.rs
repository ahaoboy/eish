//! GitHub credential discovery.
//!
//! Anonymous GitHub API access is capped at 60 requests per hour per IP, which
//! is easy to exhaust while iterating — the usual symptom is a bare
//! `HTTP 403` on the release lookup. Rather than stopping there, `eish` looks
//! for credentials the user has already stored, in the order they would expect:
//!
//! 1. `GITHUB_TOKEN` / `GH_TOKEN` in the environment
//! 2. `gh auth token` (GitHub CLI)
//! 3. `git credential fill` (Git Credential Manager, `credential.helper`)
//!
//! There is deliberately no `--token` flag. A secret on the command line lands in
//! shell history, in `ps` output, and in anything that records the command —
//! including the header of the very script this tool writes. Both supported
//! paths keep it somewhere the shell never sees.
//!
//! Discovery is best effort by design: a missing command, a timeout or empty
//! output all mean "no token", and the request simply stays anonymous.

use std::fmt;
use std::io::{Read as _, Write as _};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long a credential helper may run before it is abandoned.
///
/// `git credential fill` can hand off to a helper that waits for user input, so
/// an unbounded wait would hang `eish` on a machine with no keyring unlocked.
const HELPER_TIMEOUT: Duration = Duration::from_secs(3);

/// Where a token came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Source {
    /// Handed to [`crate::Client::with_token`] by the caller.
    Explicit,
    /// Read from `GITHUB_TOKEN` or `GH_TOKEN`.
    Environment,
    /// Obtained by running `gh auth token`.
    GhCli,
    /// Obtained from `git credential fill`.
    GitCredential,
}

impl Source {
    /// Short identifier used in messages.
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Explicit => "an explicitly supplied token",
            Source::Environment => "$GITHUB_TOKEN",
            Source::GhCli => "gh auth token",
            Source::GitCredential => "git credential fill",
        }
    }

    /// Whether the token was *found* rather than supplied by the caller.
    pub const fn is_discovered(self) -> bool {
        matches!(self, Source::GhCli | Source::GitCredential)
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A credential, together with where it came from.
///
/// `Debug` deliberately redacts the value: the type flows through error paths
/// and progress messages, where a leaked token would end up in terminal output
/// and CI logs.
#[derive(Clone, PartialEq, Eq)]
pub struct Token {
    value: String,
    source: Source,
}

impl Token {
    /// Wrap a known token.
    pub fn new(value: impl Into<String>, source: Source) -> Self {
        Self {
            value: value.into(),
            source,
        }
    }

    /// A token supplied by the caller through [`crate::Client::with_token`].
    ///
    /// Returns `None` for an empty or whitespace-only value, so callers can
    /// treat "provided but useless" the same as "not provided".
    pub fn explicit(value: impl Into<String>) -> Option<Self> {
        Self::clean(value.into(), Source::Explicit)
    }

    /// The token in `GITHUB_TOKEN` or `GH_TOKEN`, if either is set.
    ///
    /// `GH_TOKEN` is the name the GitHub CLI uses, and both are conventional in
    /// CI, so neither is treated as more authoritative.
    pub fn from_env() -> Option<Self> {
        for name in ["GITHUB_TOKEN", "GH_TOKEN"] {
            if let Ok(value) = std::env::var(name)
                && let Some(token) = Self::clean(value, Source::Environment)
            {
                return Some(token);
            }
        }
        None
    }

    /// The token the GitHub CLI has stored, via `gh auth token`.
    pub fn from_gh_cli() -> Option<Self> {
        let mut command = Command::new("gh");
        command.arg("auth").arg("token");
        // `gh` should never wait for input here.
        command.env("GH_PROMPT_DISABLED", "1");

        run_command(&mut command, None, HELPER_TIMEOUT)
            .and_then(|value| Self::clean(value, Source::GhCli))
    }

    /// The token a git credential helper has stored.
    ///
    /// Git Credential Manager is what stores GitHub credentials on Windows and
    /// macOS, so this is often the only place a usable token exists.
    pub fn from_git_credential() -> Option<Self> {
        let mut command = Command::new("git");
        command.arg("credential").arg("fill");
        // Without this git may pop up a GUI or terminal prompt, which would
        // block an otherwise non-interactive command.
        command.env("GIT_TERMINAL_PROMPT", "0");

        let request = "protocol=https\nhost=github.com\n\n";
        run_command(&mut command, Some(request), HELPER_TIMEOUT)
            .and_then(|output| parse_credential_output(&output))
            .and_then(|value| Self::clean(value, Source::GitCredential))
    }

    /// Discover a token, running the helpers at most once per process.
    ///
    /// The result — including "no token" — is cached, so a run that makes
    /// several API calls does not spawn `gh` repeatedly.
    pub fn detect() -> Option<&'static Token> {
        static CACHE: OnceLock<Option<Token>> = OnceLock::new();

        CACHE
            .get_or_init(|| {
                Self::from_env()
                    .or_else(Self::from_gh_cli)
                    .or_else(Self::from_git_credential)
            })
            .as_ref()
    }

    /// The token value, for the `Authorization` header.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Where the token came from.
    pub fn source(&self) -> Source {
        self.source
    }

    /// Whether this token may be sent to `api_base`.
    ///
    /// Tokens the caller supplied — through [`crate::Client::with_token`] or the
    /// environment — go wherever `--api-base` points, because the caller asked
    /// for exactly that. Credentials *discovered* from `gh` or `git` belong to
    /// github.com, so they are only attached when the API root really is
    /// github.com; sending them to an arbitrary `--api-base` host would hand a
    /// github.com token to a third party.
    pub fn may_send_to(&self, api_base: &str) -> bool {
        !self.source.is_discovered()
            || host_of(api_base)
                .is_some_and(|host| host == "github.com" || host.ends_with(".github.com"))
    }

    /// Normalise a candidate value, rejecting blanks.
    fn clean(value: String, source: Source) -> Option<Self> {
        let value = value.trim();
        (!value.is_empty()).then(|| Self::new(value, source))
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Token")
            .field("source", &self.source)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Pull the `password` field out of `git credential fill` output.
///
/// The format is a list of `key=value` lines (`protocol`, `host`, `username`,
/// `password`, …) terminated by a blank line.
fn parse_credential_output(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "password" && !value.trim().is_empty()).then(|| value.trim().to_string())
    })
}

/// The host component of a URL, without scheme, userinfo or port.
fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    Some(host.split(':').next().unwrap_or(host))
}

/// Run `command`, returning trimmed stdout, or `None` on failure or timeout.
///
/// `std::process::Command` has no timeout, so the child is polled and killed
/// once `timeout` elapses. That is what keeps a wedged credential helper from
/// hanging the whole run.
fn run_command(command: &mut Command, input: Option<&str>, timeout: Duration) -> Option<String> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = command.spawn().ok()?;

    if let Some(input) = input {
        let mut stdin = child.stdin.take()?;
        stdin.write_all(input.as_bytes()).ok()?;
        // Dropping the handle sends EOF, which is what the helper waits for.
        drop(stdin);
    }

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut stdout = child.stdout.take()?;
                let mut output = String::new();
                stdout.read_to_string(&mut output).ok()?;
                let output = output.trim();
                return (!output.is_empty()).then(|| output.to_string());
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_credential_output() {
        let output = "protocol=https\nhost=github.com\nusername=octocat\npassword=gho_secret\n\n";
        assert_eq!(
            parse_credential_output(output).as_deref(),
            Some("gho_secret")
        );

        // CRLF, as produced on Windows.
        let output = "protocol=https\r\nhost=github.com\r\npassword=gho_secret\r\n";
        assert_eq!(
            parse_credential_output(output).as_deref(),
            Some("gho_secret")
        );
    }

    #[test]
    fn ignores_output_without_a_password() {
        assert_eq!(parse_credential_output(""), None);
        assert_eq!(
            parse_credential_output("protocol=https\nhost=github.com\n"),
            None
        );
        // A helper that reports an empty password is not usable.
        assert_eq!(parse_credential_output("password=\n"), None);
        // Only the `password` key counts, not a lookalike.
        assert_eq!(parse_credential_output("password_hash=abc\n"), None);
    }

    #[test]
    fn rejects_blank_values() {
        assert!(Token::explicit("").is_none());
        assert!(Token::explicit("   ").is_none());
        assert!(Token::explicit(" token ").is_some());
        assert_eq!(Token::explicit(" token ").unwrap().value(), "token");
    }

    #[test]
    fn parses_url_hosts() {
        assert_eq!(
            host_of("https://api.github.com/repos/x/y"),
            Some("api.github.com")
        );
        assert_eq!(host_of("https://github.com"), Some("github.com"));
        assert_eq!(
            host_of("https://user:pw@github.com:443/x"),
            Some("github.com")
        );
        assert_eq!(
            host_of("http://ghe.corp.example/api/v3"),
            Some("ghe.corp.example")
        );
        assert_eq!(host_of("not a url"), None);
    }

    #[test]
    fn discovered_tokens_stay_on_github() {
        let discovered = Token::new("t", Source::GhCli);
        assert!(discovered.may_send_to("https://api.github.com"));
        assert!(discovered.may_send_to("https://github.com"));
        assert!(!discovered.may_send_to("https://ghe.corp.example/api/v3"));
        // A lookalike host must not be mistaken for github.com.
        assert!(!discovered.may_send_to("https://github.com.evil.example"));
        assert!(!discovered.may_send_to("https://notgithub.com"));
    }

    #[test]
    fn supplied_tokens_go_wherever_the_caller_points() {
        for source in [Source::Explicit, Source::Environment] {
            let token = Token::new("t", source);
            assert!(token.may_send_to("https://ghe.corp.example/api/v3"));
            assert!(token.may_send_to("https://api.github.com"));
        }
    }

    #[test]
    fn debug_redacts_the_value() {
        let token = Token::new("gho_supersecret", Source::Explicit);
        let rendered = format!("{token:?}");
        assert!(!rendered.contains("gho_supersecret"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        assert!(rendered.contains("Explicit"), "{rendered}");
    }
}
