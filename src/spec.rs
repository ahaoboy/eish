//! The install-specification model that every template renders from.

use std::fmt;
use std::str::FromStr;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::github::Release;
use crate::proxy::Proxy;
use crate::shell::Shell;
use crate::target;

/// The default installation directory, shared by every generated installer.
pub const DEFAULT_INSTALL_DIR: &str = "~/.ei";

/// The default minimum free disk space, in megabytes.
pub const DEFAULT_MIN_DISK_SPACE_MB: u64 = 10;

/// Where the generated installer downloads the binary from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    /// A GitHub *release* asset: `…/releases/latest/download/<file>`.
    Release,
    /// A file committed to the repository: `…/raw/<reference>/<file>`.
    File {
        /// Branch, tag or commit SHA to read the file from.
        reference: String,
    },
}

impl Resource {
    /// `release` or `file`, as written into the generated script.
    pub fn as_str(&self) -> &'static str {
        match self {
            Resource::Release => "release",
            Resource::File { .. } => "file",
        }
    }

    /// The repository reference, for [`Resource::File`].
    pub fn reference(&self) -> Option<&str> {
        match self {
            Resource::Release => None,
            Resource::File { reference } => Some(reference),
        }
    }
}

/// One prebuilt binary that the installer knows how to fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssetEntry {
    /// Rust target triple, e.g. `x86_64-unknown-linux-musl`.
    pub target: String,
    /// File name inside the release, e.g. `tool-x86_64-unknown-linux-musl.tar.gz`.
    pub filename: String,
    /// File size in bytes, as reported by the API.
    pub size: u64,
}

/// Everything the templates need, and the public API most library users touch.
///
/// ```
/// use eish::{InstallSpec, Shell, Proxy};
///
/// let spec = InstallSpec::new("easy-install", "easy-install")
///     .with_shell(Shell::Fish)
///     .with_tag("v1.0.0")
///     .with_proxy(Proxy::Xget);
///
/// let script = spec.render().unwrap();
/// assert!(script.contains("v1.0.0"));
/// ```
#[derive(Debug, Clone)]
pub struct InstallSpec {
    /// Repository owner (user or organisation).
    pub owner: String,
    /// Repository name.
    pub repo: String,
    /// Tag the user asked for; `latest` keeps the installer floating.
    pub tag: String,
    /// Tag the chosen release actually has.
    pub resolved_tag: Option<String>,
    /// Executable name; inferred from the assets when left as `None`.
    pub binary: Option<String>,
    /// Shell dialect to generate.
    pub shell: Shell,
    /// Default download proxy baked into the installer.
    pub proxy: Proxy,
    /// Release asset or repository file.
    pub resource: Resource,
    /// Default installation directory.
    pub install_dir: String,
    /// Default minimum free disk space, in megabytes.
    pub min_disk_space_mb: u64,
    /// Prebuilt binaries, one entry per supported target triple.
    pub assets: Vec<AssetEntry>,
    /// Target triple the installer uses when detection has to be bypassed.
    pub default_target: Option<String>,
    /// The exact command line that produced this spec, when it is known.
    ///
    /// Set by the CLI from its own arguments, which is the only way to record
    /// things the spec cannot represent — `--release-json` above all, since the
    /// file it named is not part of the spec. Library callers leave it `None`
    /// and get a reconstruction instead.
    pub invocation: Option<String>,
}

impl InstallSpec {
    /// A spec for `owner/repo` with sensible defaults (bash, no proxy, latest).
    pub fn new(owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            tag: "latest".to_string(),
            resolved_tag: None,
            binary: None,
            shell: Shell::default(),
            proxy: Proxy::default(),
            resource: Resource::Release,
            install_dir: DEFAULT_INSTALL_DIR.to_string(),
            min_disk_space_mb: DEFAULT_MIN_DISK_SPACE_MB,
            assets: Vec::new(),
            default_target: None,
            invocation: None,
        }
    }

    /// Record the command line that produced this spec.
    #[must_use]
    pub fn with_invocation(mut self, invocation: impl Into<String>) -> Self {
        let invocation = invocation.into();
        self.invocation = (!invocation.trim().is_empty()).then(|| invocation.trim().to_string());
        self
    }

    /// `owner/repo`, used in messages and comments.
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// The executable name that will be baked into the installer.
    pub fn binary_name(&self) -> &str {
        self.binary.as_deref().unwrap_or(&self.repo)
    }

    /// Set the tag, rejecting empty values.
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        let tag = tag.into();
        if !tag.is_empty() {
            self.tag = tag;
        }
        self
    }

    /// Select the shell dialect to generate.
    #[must_use]
    pub fn with_shell(mut self, shell: Shell) -> Self {
        self.shell = shell;
        self
    }

    /// Choose the proxy written into the installer.
    #[must_use]
    pub fn with_proxy(mut self, proxy: Proxy) -> Self {
        self.proxy = proxy;
        self
    }

    /// Override the executable name instead of inferring it from asset names.
    #[must_use]
    pub fn with_binary(mut self, binary: impl Into<String>) -> Self {
        self.binary = Some(binary.into());
        self
    }

    /// Set the default installation directory.
    #[must_use]
    pub fn with_install_dir(mut self, dir: impl Into<String>) -> Self {
        self.install_dir = dir.into();
        self
    }

    /// Set the default minimum free disk space, in megabytes.
    #[must_use]
    pub fn with_min_disk_space_mb(mut self, megabytes: u64) -> Self {
        self.min_disk_space_mb = megabytes;
        self
    }

    /// Switch between release assets and repository files.
    #[must_use]
    pub fn with_resource(mut self, resource: Resource) -> Self {
        self.resource = resource;
        self
    }

    /// Force the target triple the installer defaults to.
    #[must_use]
    pub fn with_default_target(mut self, target: Option<String>) -> Self {
        self.default_target = target;
        self
    }

    /// Fill the asset table from explicit file names instead of the API.
    ///
    /// This is what `--asset` uses, and it is the only way to generate an
    /// installer for a repository that does not publish a GitHub release.
    /// Because the user named the files themselves, even low-confidence guesses
    /// are accepted; only files that match no target at all are skipped.
    ///
    /// Returns the number of targets that could be matched.
    pub fn apply_assets<'a>(&mut self, file_names: impl IntoIterator<Item = &'a str>) -> usize {
        let files: Vec<(String, u64)> = file_names
            .into_iter()
            .map(|name| (name.to_string(), 0))
            .collect();
        self.fill_assets(&files, 1)
    }

    /// Fill the asset table from a release.
    ///
    /// Returns the number of targets that could be matched; `0` means the
    /// release does not publish recognisable binaries.
    pub fn apply_release(&mut self, release: &Release) -> usize {
        self.resolved_tag = Some(release.tag(&self.tag));

        let files: Vec<(String, u64)> = release
            .assets
            .iter()
            .map(|asset| (asset.name.clone(), asset.size))
            .collect();
        self.fill_assets(&files, target::MIN_RANK)
    }

    /// Build the target → asset table.
    ///
    /// The table is keyed by platform, so this is the inverse of
    /// [`target::guess_target`]: for every triple an installer can detect, it
    /// picks the asset that matches it best ([`target::best_asset`]). Doing it
    /// this way — rather than mapping each file to whichever platform it
    /// happens to name — is what lets a generic `jq-linux-amd64` coexist with a
    /// specific `jq-linux-musl-amd64` and still send each machine to the right
    /// file.
    fn fill_assets(&mut self, files: &[(String, u64)], min_rank: u32) -> usize {
        // Non-installable assets are filtered out inside `target::candidates`.
        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();

        for target in target::KNOWN_TARGETS {
            let Some((filename, _rank)) =
                target::best_asset(names.iter().copied(), target, min_rank)
            else {
                continue;
            };

            // `names` is already filtered, so a missing name cannot happen; the
            // lookup exists to carry the size through from the release.
            let size = files
                .iter()
                .find(|(name, _)| name == filename)
                .map_or(0, |(_, size)| *size);

            self.assets.push(AssetEntry {
                target: (*target).to_string(),
                filename: filename.to_string(),
                size,
            });
        }

        // Keep the table deterministic regardless of release asset ordering.
        self.assets.sort_by(|a, b| a.target.cmp(&b.target));

        if self.binary.is_none() {
            self.binary = infer_binary_name(self.assets.iter().map(|a| a.filename.as_str()));
        }

        self.assets.len()
    }

    /// Like [`InstallSpec::apply_release`], but fails when nothing matched.
    pub fn apply_release_checked(&mut self, release: &Release) -> Result<()> {
        if self.apply_release(release) == 0 {
            return Err(Error::NoAssets {
                repo: self.slug(),
                tag: self
                    .resolved_tag
                    .clone()
                    .unwrap_or_else(|| self.tag.clone()),
            });
        }
        Ok(())
    }

    /// Validate combinations that cannot work at runtime.
    pub fn validate(&self) -> Result<()> {
        if self.resource == Resource::Release && !self.proxy.supports_release_assets() {
            return Err(Error::UnsupportedProxy {
                proxy: self.proxy.as_str(),
            });
        }

        if let Some(target) = &self.default_target {
            if !self.assets.iter().any(|entry| &entry.target == target) {
                return Err(Error::UnknownTarget {
                    target: target.clone(),
                    available: self.available_targets(),
                });
            }
        }

        Ok(())
    }

    /// Every target triple in the asset table, sorted and space separated.
    pub fn available_targets(&self) -> String {
        let mut targets: Vec<&str> = self.assets.iter().map(|a| a.target.as_str()).collect();
        targets.sort_unstable();
        targets.join(" ")
    }

    /// Render the installer for [`InstallSpec::shell`].
    pub fn render(&self) -> Result<String> {
        crate::render::render(self)
    }

    /// The command that reproduces this installer.
    ///
    /// Returns the recorded [`InstallSpec::invocation`] when the CLI supplied
    /// one, and otherwise reconstructs an equivalent command from the fields —
    /// a library caller has no command line, but the spec still describes one.
    /// Only values differing from the documented defaults are spelled out, so
    /// the result stays readable.
    ///
    /// ```
    /// use eish::{InstallSpec, Proxy, Shell};
    ///
    /// let spec = InstallSpec::new("acme", "tool")
    ///     .with_shell(Shell::Fish)
    ///     .with_proxy(Proxy::Xget);
    /// assert_eq!(
    ///     spec.regenerate_command(),
    ///     "eish acme/tool --shell fish --proxy xget",
    /// );
    /// ```
    pub fn regenerate_command(&self) -> String {
        if let Some(invocation) = &self.invocation {
            return invocation.clone();
        }
        self.reconstructed_command()
    }

    /// Build a command from the spec fields, for callers without a command line.
    fn reconstructed_command(&self) -> String {
        let mut parts = vec!["eish".to_string()];

        let mut repo = self.slug();
        if self.tag != "latest" {
            repo.push('@');
            repo.push_str(&self.tag);
        }
        parts.push(repo);

        // Listed in the same order as `eish --help`, which makes a long command
        // easier to scan.
        parts.push(format!("--shell {}", self.shell));

        if self.proxy != Proxy::default() {
            parts.push(format!("--proxy {}", self.proxy));
        }
        if let Some(binary) = &self.binary {
            parts.push(format!("--binary {}", quote(binary)));
        }
        if let Some(target) = &self.default_target {
            parts.push(format!("--target {}", quote(target)));
        }
        if self.install_dir != DEFAULT_INSTALL_DIR {
            parts.push(format!("--dir {}", quote(&self.install_dir)));
        }
        if let Resource::File { reference } = &self.resource {
            parts.push("--type file".to_string());
            parts.push(format!("--ref {}", quote(reference)));
        }
        if self.min_disk_space_mb != DEFAULT_MIN_DISK_SPACE_MB {
            parts.push(format!("--min-disk-space {}", self.min_disk_space_mb));
        }

        parts.join(" ")
    }
}

/// Quote a value for a POSIX shell if it contains anything that would otherwise
/// be split or interpreted.
///
/// A leading `~` is deliberately *not* treated as safe: unquoted it would be
/// expanded by the shell, which changes what is passed to `eish`.
fn quote(value: &str) -> String {
    let safe = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/:+@=,".contains(c));

    if safe {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

/// Guess the executable name from the assets that made it into the table.
///
/// `guess_target` already strips the platform off every asset name, so each
/// table entry votes for a candidate and the most common one wins. Voting over
/// the *chosen* files rather than every release asset matters for releases that
/// ship several products (deno publishes `deno`, `denort` and `libdenort`
/// archives, each covering the same platforms). The candidate is then passed
/// through [`normalise_binary_candidate`] because ambiguous names leak platform
/// words into the tool name (`tool-universal2-apple-darwin` is reported as
/// `tool-universal2-apple`). Returns `None` when no file carried a name.
fn infer_binary_name<'a>(files: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut candidates: Vec<(String, usize)> = Vec::new();

    for file in files {
        let Some(reported) = target::guess_binary_name(file) else {
            continue;
        };
        let Some(candidate) = normalise_binary_candidate(&reported) else {
            continue;
        };

        match candidates
            .iter_mut()
            .find(|(name, _)| name.eq_ignore_ascii_case(&candidate))
        {
            Some((_, votes)) => *votes += 1,
            None => candidates.push((candidate, 1)),
        }
    }

    candidates
        .into_iter()
        .max_by_key(|(_, votes)| *votes)
        .map(|(name, _)| name)
}

/// Drop platform, architecture and version noise from a candidate binary name.
///
/// Turns `mytool-linux`, `mytool-universal2-apple` and `mytool-1.2.3` all into
/// `mytool`, so the assets of one release agree on a single name.
fn normalise_binary_candidate(raw: &str) -> Option<String> {
    /// Words that describe the platform rather than the program.
    const PLATFORM_WORDS: &[&str] = &[
        "unknown",
        "linux",
        "linuxstatic",
        "darwin",
        "macos",
        "mac",
        "osx",
        "apple",
        "windows",
        "win",
        "win32",
        "win64",
        "unix",
        "freebsd",
        "netbsd",
        "openbsd",
        "android",
        "musl",
        "gnu",
        "msvc",
        "gnullvm",
        "mingw",
        "eabi",
        "eabihf",
        "hf",
        "universal",
        "universal2",
        "portable",
        "static",
        "dynamic",
        "dynamically",
        "pc",
        "vendor",
        "target",
        "release",
        "debug",
        "bin",
        "dist",
        "install",
        "setup",
        "full",
        "minimal",
    ];

    /// Words that describe the CPU architecture.
    const ARCH_WORDS: &[&str] = &[
        "amd64",
        "x86",
        "x86-64",
        "x64",
        "i386",
        "i686",
        "x86_64",
        "aarch64",
        "arm64",
        "armv6",
        "armv7",
        "armv7l",
        "armv8",
        "armhf",
        "armel",
        "arm",
        "riscv64",
        "riscv64gc",
        "riscv",
        "loongarch64",
        "loongarch",
        "powerpc64le",
        "ppc64le",
        "ppc64",
        "s390x",
        "32",
        "64",
    ];

    let is_noise = |token: &str| {
        let lower = token.to_ascii_lowercase();
        PLATFORM_WORDS.contains(&lower.as_str())
            || ARCH_WORDS.contains(&lower.as_str())
            || is_version_like(&lower)
    };

    // Keep the separator handling simple: a program name never contains `-` or
    // `_` in practice, while release tooling uses both freely.
    let kept: Vec<&str> = raw
        .split(['-', '_'])
        .filter(|token| !token.is_empty() && !is_noise(token))
        .collect();

    if kept.is_empty() {
        return None;
    }

    let candidate = kept.join("-");
    if candidate.contains(['/', '\\']) || candidate.starts_with('.') {
        return None;
    }
    Some(candidate)
}

/// Whether a token looks like a version or a bare build number.
fn is_version_like(token: &str) -> bool {
    let trimmed = token.strip_prefix('v').unwrap_or(token);
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
        && trimmed.contains(|c: char| c.is_ascii_digit())
}

/// A parsed `owner/repo[@tag]` argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSpec {
    /// Repository owner.
    pub owner: String,
    /// Repository name.
    pub repo: String,
    /// Requested tag, if the user pinned one.
    pub tag: Option<String>,
}

impl RepoSpec {
    /// `owner/repo`, used in messages.
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// Parse `owner/repo`, `owner/repo@v1.0.0` or a full GitHub URL.
    pub fn parse(input: &str) -> Result<Self> {
        let invalid = || Error::InvalidSpec {
            input: input.to_string(),
        };

        let trimmed = input.trim();
        let without_scheme = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))
            .unwrap_or(trimmed);
        let path = without_scheme
            .strip_prefix("github.com/")
            .unwrap_or(without_scheme)
            .trim_end_matches('/');

        let (path, tag) = match path.split_once('@') {
            Some((path, tag)) => (path, Some(tag.trim())),
            None => (path, None),
        };

        let mut parts = path.splitn(2, '/');
        let owner = parts.next().unwrap_or_default().trim();
        let repo = parts.next().unwrap_or_default().trim();
        let repo = repo.trim_end_matches(".git");

        if owner.is_empty() || repo.is_empty() || repo.contains('/') {
            return Err(invalid());
        }

        Ok(Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            tag: tag.filter(|tag| !tag.is_empty()).map(str::to_string),
        })
    }
}

impl FromStr for RepoSpec {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for RepoSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)?;
        if let Some(tag) = &self.tag {
            write!(f, "@{tag}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::Asset;

    fn release_with(names: &[&str]) -> Release {
        Release {
            tag_name: Some("v2.0.0".to_string()),
            name: None,
            assets: names
                .iter()
                .map(|name| Asset {
                    name: (*name).to_string(),
                    size: 10,
                })
                .collect(),
        }
    }

    #[test]
    fn parses_repo_specs() {
        let spec = RepoSpec::parse("rust-lang/rust").unwrap();
        assert_eq!(spec.owner, "rust-lang");
        assert_eq!(spec.repo, "rust");
        assert_eq!(spec.tag, None);

        let spec = RepoSpec::parse("owner/repo@v1.2.3").unwrap();
        assert_eq!(spec.tag.as_deref(), Some("v1.2.3"));

        let spec = RepoSpec::parse("https://github.com/owner/repo.git").unwrap();
        assert_eq!(spec.slug(), "owner/repo");
    }

    #[test]
    fn rejects_bad_repo_specs() {
        for input in ["", "owner", "owner/", "/repo", "a/b/c"] {
            assert!(RepoSpec::parse(input).is_err(), "should reject {input:?}");
        }
    }

    #[test]
    fn builds_the_asset_table() {
        let mut spec = InstallSpec::new("easy-install", "easy-install");
        let matched = spec.apply_release(&release_with(&[
            "ei-x86_64-pc-windows-gnu.zip",
            "ei-x86_64-unknown-linux-musl.tar.gz",
            "ei-aarch64-apple-darwin.tar.gz",
            "checksums.txt",
        ]));

        assert_eq!(matched, 3);
        assert_eq!(spec.resolved_tag.as_deref(), Some("v2.0.0"));
        assert_eq!(spec.binary_name(), "ei");
        assert_eq!(spec.assets.len(), 3);
    }

    #[test]
    fn keeps_the_first_asset_for_duplicate_targets() {
        let mut spec = InstallSpec::new("o", "r");
        spec.apply_release(&release_with(&[
            "tool-x86_64-unknown-linux-musl.tar.gz",
            "tool-x86_64-unknown-linux-musl.zip",
        ]));

        assert_eq!(spec.assets.len(), 1);
        assert_eq!(
            spec.assets[0].filename,
            "tool-x86_64-unknown-linux-musl.tar.gz"
        );
    }

    #[test]
    fn prefers_the_explicit_binary_name() {
        let mut spec = InstallSpec::new("o", "r").with_binary("custom");
        spec.apply_release(&release_with(&["tool-x86_64-unknown-linux-musl.tar.gz"]));
        assert_eq!(spec.binary_name(), "custom");
    }

    #[test]
    fn rejects_release_assets_with_file_only_proxies() {
        let spec = InstallSpec::new("o", "r").with_proxy(Proxy::Jsdelivr);
        assert!(spec.validate().is_err());
    }

    #[test]
    fn infers_the_binary_name_from_loosely_named_assets() {
        // Every asset agrees on `jq` once the platform words are dropped.
        let mut spec = InstallSpec::new("jqlang", "jq");
        spec.apply_release(&release_with(&[
            "jq-linux-amd64",
            "jq-linux-arm64",
            "jq-macos-amd64",
            "jq-windows-amd64.exe",
        ]));
        assert_eq!(spec.binary_name(), "jq");
    }

    #[test]
    fn infers_the_binary_name_from_underscored_assets() {
        let mut spec = InstallSpec::new("owner", "name");
        spec.apply_release(&release_with(&[
            "mytool_1.2.3_linux_x86_64.tar.gz",
            "mytool_1.2.3_windows_x86_64.zip",
        ]));
        assert_eq!(spec.binary_name(), "mytool");
    }

    #[test]
    fn falls_back_to_the_repo_name_without_assets() {
        let spec = InstallSpec::new("owner", "some-tool");
        assert_eq!(spec.binary_name(), "some-tool");
    }

    #[test]
    fn reconstructs_a_minimal_command() {
        let spec = InstallSpec::new("acme", "tool");
        // Defaults are omitted, but the shell is always stated because it
        // cannot be inferred from the repository.
        assert_eq!(spec.regenerate_command(), "eish acme/tool --shell bash");
    }

    #[test]
    fn reconstructs_the_tag_and_options() {
        let spec = InstallSpec::new("acme", "tool")
            .with_tag("v1.2.3")
            .with_shell(Shell::Fish)
            .with_proxy(Proxy::Xget)
            .with_binary("mytool")
            .with_default_target(Some("x86_64-unknown-linux-gnu".to_string()))
            .with_install_dir("/opt/tool")
            .with_min_disk_space_mb(200);

        let command = spec.regenerate_command();
        for expected in [
            "eish acme/tool@v1.2.3",
            "--shell fish",
            "--proxy xget",
            "--binary mytool",
            "--target x86_64-unknown-linux-gnu",
            "--dir /opt/tool",
            "--min-disk-space 200",
        ] {
            assert!(
                command.contains(expected),
                "{expected:?} missing from {command}"
            );
        }
    }

    #[test]
    fn reconstructs_the_file_resource() {
        let spec = InstallSpec::new("acme", "tool").with_resource(Resource::File {
            reference: "dev".to_string(),
        });
        let command = spec.regenerate_command();
        assert!(command.contains("--type file"), "{command}");
        assert!(command.contains("--ref dev"), "{command}");
    }

    #[test]
    fn quotes_values_that_need_it() {
        // A space would otherwise split the argument in two.
        let spec = InstallSpec::new("acme", "tool").with_install_dir("/opt/my tools");
        assert!(
            spec.regenerate_command().contains("--dir '/opt/my tools'"),
            "{}",
            spec.regenerate_command()
        );

        // A leading `~` is unsafe unquoted: the shell would expand it, which
        // changes the argument `eish` receives.
        let spec = InstallSpec::new("acme", "tool").with_install_dir("~/bin");
        assert!(
            spec.regenerate_command().contains("--dir '~/bin'"),
            "{}",
            spec.regenerate_command()
        );
    }

    #[test]
    fn an_explicit_invocation_wins() {
        // The CLI records argv verbatim, which is the only way to capture things
        // the spec does not model — `--release-json` above all.
        let spec = InstallSpec::new("acme", "tool")
            .with_invocation("eish acme/tool --release-json release.json --quiet");
        assert_eq!(
            spec.regenerate_command(),
            "eish acme/tool --release-json release.json --quiet"
        );
    }

    #[test]
    fn a_blank_invocation_is_ignored() {
        let spec = InstallSpec::new("acme", "tool").with_invocation("   ");
        assert_eq!(spec.regenerate_command(), "eish acme/tool --shell bash");
    }

    #[test]
    fn the_command_appears_in_every_rendered_script() {
        for shell in Shell::ALL {
            let spec = InstallSpec::new("acme", "tool")
                .with_shell(shell)
                .with_invocation("eish acme/tool --shell bash --proxy xget");
            let script = spec.render().unwrap();

            assert!(
                script.contains("eish acme/tool --shell bash --proxy xget"),
                "{shell} header is missing the command"
            );
            assert!(
                script.contains("not meant to be"),
                "{shell} header is missing the do-not-edit notice"
            );
        }
    }
}
