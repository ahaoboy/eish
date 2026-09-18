//! Command line front-end for `eish`.
//!
//! Everything the user can configure lives here; the actual work is delegated
//! to the library so that `eish` stays scriptable and testable.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::builder::PossibleValue;
use clap::{Parser, ValueEnum};

use eish::github::{Client, release_from_json_file};
use eish::spec::RepoSpec;
use eish::{InstallSpec, Proxy, Resource, Shell};

/// Generate a self-contained installation script for a GitHub release.
///
/// The release is inspected through the GitHub API, the asset that matches a
/// platform is baked into the script, and the result is written to stdout.
#[derive(Debug, Parser)]
#[command(
    name = "eish",
    version,
    about = "Generate installation scripts for GitHub release binaries",
    long_about = None,
    after_help = "Examples:\n  \
        eish easy-install/easy-install > install.sh\n  \
        eish cli/cli@v2.40.0 --shell powershell > install.ps1\n  \
        eish owner/repo --shell fish --proxy xget > install.fish\n  \
        eish owner/repo@v1.0.0 | bash"
)]
pub struct Cli {
    /// Repository to install from: `owner/repo`, `owner/repo@tag` or a full
    /// GitHub URL.
    #[arg(value_name = "SPEC")]
    spec: String,

    /// Shell dialect to generate.
    #[arg(short, long, value_enum, default_value_t = Shell::Bash)]
    shell: Shell,

    /// Proxy the generated installer uses by default.
    #[arg(long, value_enum, default_value_t = Proxy::Github)]
    proxy: Proxy,

    /// Tag to install; overrides the `@tag` suffix of SPEC.
    #[arg(long)]
    tag: Option<String>,

    /// Name of the executable inside the archive.
    ///
    /// Inferred from the asset file names when omitted.
    #[arg(short, long)]
    binary: Option<String>,

    /// Target triple the generated installer defaults to, skipping detection.
    #[arg(long)]
    target: Option<String>,

    /// Default installation directory baked into the installer.
    #[arg(long, default_value = eish::spec::DEFAULT_INSTALL_DIR)]
    dir: String,

    /// Download release assets or files committed to the repository.
    #[arg(long = "type", value_enum, default_value_t = ResourceType::Release)]
    resource_type: ResourceType,

    /// Branch, tag or commit used by `--type file`.
    #[arg(long = "ref", default_value = "main")]
    reference: String,

    /// Default minimum free disk space, in megabytes (0 disables the check).
    #[arg(long, default_value_t = eish::spec::DEFAULT_MIN_DISK_SPACE_MB)]
    min_disk_space: u64,

    /// Write the script here instead of stdout.
    #[arg(short, long, value_name = "PATH")]
    output: Option<PathBuf>,

    /// GitHub API root, e.g. a GitHub Enterprise or mirror endpoint.
    #[arg(long, default_value = eish::github::DEFAULT_API_BASE)]
    api_base: String,

    /// GitHub token used for the API request.
    ///
    /// When omitted, `GITHUB_TOKEN`, `GH_TOKEN`, `gh auth token` and
    /// `git credential fill` are tried in turn.
    #[arg(long)]
    token: Option<String>,

    /// Read the release from a JSON file instead of calling the API.
    #[arg(long, value_name = "PATH")]
    release_json: Option<PathBuf>,

    /// Name of a release asset to bundle, instead of querying the API.
    ///
    /// Repeat once per file; the target triple is guessed from each name.
    /// This is the only way to generate an installer for a repository that
    /// publishes no GitHub release at all.
    #[arg(long = "asset", value_name = "FILE")]
    assets: Vec<String>,

    /// Only list the platforms that would be supported, don't render.
    #[arg(long)]
    list: bool,

    /// Suppress progress messages.
    #[arg(short, long)]
    quiet: bool,
}

/// Which kind of resource the installer downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceType {
    /// A file attached to a GitHub release.
    Release,
    /// A file committed to the repository.
    File,
}
impl ValueEnum for ResourceType {
    fn value_variants<'a>() -> &'a [Self] {
        &[ResourceType::Release, ResourceType::File]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(match self {
            ResourceType::Release => {
                PossibleValue::new("release").help("a file attached to a GitHub release")
            }
            ResourceType::File => {
                PossibleValue::new("file").help("a file committed to the repository (see --ref)")
            }
        })
    }
}

impl Cli {
    /// Run the command, returning the process exit code.
    pub fn run(self) -> ExitCode {
        match self.execute() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                let mut source = std::error::Error::source(&error);
                while let Some(cause) = source {
                    eprintln!("  caused by: {cause}");
                    source = cause.source();
                }
                ExitCode::FAILURE
            }
        }
    }

    fn execute(&self) -> eish::Result<()> {
        let parsed = RepoSpec::parse(&self.spec)?;

        let mut spec = InstallSpec::new(parsed.owner, parsed.repo)
            .with_shell(self.shell)
            .with_proxy(self.proxy)
            .with_install_dir(self.dir.clone())
            .with_min_disk_space_mb(self.min_disk_space)
            .with_default_target(self.target.clone())
            .with_invocation(invocation())
            .with_resource(match self.resource_type {
                ResourceType::Release => Resource::Release,
                ResourceType::File => Resource::File {
                    reference: self.reference.clone(),
                },
            });

        if let Some(binary) = &self.binary {
            spec = spec.with_binary(binary.clone());
        }

        // `--tag` wins over the `@tag` suffix.
        if let Some(tag) = self.tag.clone().or(parsed.tag) {
            spec = spec.with_tag(tag);
        }

        let release = match (&self.release_json, self.assets.is_empty()) {
            // Explicit asset list: no network access at all.
            (_, false) => None,
            (Some(path), _) => Some(release_from_json_file(path)?),
            (None, _) => {
                let client = Client::new()
                    .with_api_base(self.api_base.clone())
                    .with_token(self.token.clone());

                // Say which credential is in play, so a later 403 is easy to
                // attribute to the wrong token rather than to rate limiting.
                if let Some(source) = client.credential_source() {
                    self.progress(&format!("using credentials from {source}"))?;
                }

                self.progress(&format!("querying GitHub for {}", spec.slug()))?;
                Some(client.release(&spec.owner, &spec.repo, Some(&spec.tag))?)
            }
        };

        match &release {
            Some(release) => spec.apply_release_checked(release)?,
            None => {
                if spec.apply_assets(self.assets.iter().map(String::as_str)) == 0 {
                    return Err(eish::Error::NoAssets {
                        repo: spec.slug(),
                        tag: spec.tag.clone(),
                    });
                }
            }
        }

        spec.validate()?;

        if self.list {
            for asset in &spec.assets {
                println!("{}\t{}", asset.target, asset.filename);
            }
            return Ok(());
        }

        self.progress(&format!(
            "found {} target(s) for {} ({})",
            spec.assets.len(),
            spec.slug(),
            spec.resolved_tag.as_deref().unwrap_or(&spec.tag)
        ))?;
        let script = spec.render()?;
        self.write(&script)
    }

    fn write(&self, script: &str) -> eish::Result<()> {
        match &self.output {
            Some(path) => {
                std::fs::write(path, script).map_err(|source| eish::Error::WriteFile {
                    path: path.clone(),
                    source,
                })?;
                self.progress(&format!("wrote {}", path.display()))?;
                Ok(())
            }
            None => {
                let mut out = std::io::stdout().lock();
                out.write_all(script.as_bytes())
                    .map_err(|source| eish::Error::WriteFile {
                        path: PathBuf::from("<stdout>"),
                        source,
                    })?;
                out.flush().map_err(|source| eish::Error::WriteFile {
                    path: PathBuf::from("<stdout>"),
                    source,
                })
            }
        }
    }

    /// Print a progress line on stderr, unless `--quiet` was given.
    fn progress(&self, message: &str) -> eish::Result<()> {
        if self.quiet {
            return Ok(());
        }
        let mut err = std::io::stderr().lock();
        writeln!(err, "eish: {message}").map_err(|source| eish::Error::WriteFile {
            path: PathBuf::from("<stderr>"),
            source,
        })
    }
}

/// The command line that produced this process, for the generated header.
///
/// Taken from `argv` rather than rebuilt from the parsed options so that it
/// stays exactly what the user typed — including things the spec does not model,
/// such as `--release-json <path>`. The program name is normalised to `eish` so
/// the printed command does not depend on where the binary happens to live.
///
/// Values of secret-bearing options are replaced with a placeholder: the
/// generated script is meant to be committed and shared, so a token that was
/// only ever passed on the command line must not end up inside it.
fn invocation() -> String {
    let mut args = std::env::args();
    let _program = args.next();
    build_invocation(args)
}

/// Build the header command from the arguments after the program name.
///
/// Split out from [`invocation`] so the redaction can be tested without
/// spawning a process.
fn build_invocation(args: impl IntoIterator<Item = String>) -> String {
    let mut parts = vec!["eish".to_string()];
    let mut args = args.into_iter().peekable();

    while let Some(arg) = args.next() {
        // `--token=value`
        if let Some((name, _)) = arg.split_once('=') {
            if is_secret_option(name) {
                parts.push(format!("{name}=<redacted>"));
            } else {
                parts.push(quote_arg(&arg));
            }
            continue;
        }

        // `--token value`
        if is_secret_option(&arg) {
            // Consume the next argument unconditionally. Skipping a value that
            // starts with `-` would risk printing it, and clap rejects such a
            // value anyway, so there is nothing to preserve by being clever.
            args.next();
            parts.push(format!("{arg} <redacted>"));
            continue;
        }

        parts.push(quote_arg(&arg));
    }

    parts.join(" ")
}

/// Options whose value must never be written into a generated script.
///
/// Today `--token` is the only one, and it has no short alias. Anything added
/// here must stay in step with the `Cli` struct: a new secret-bearing option
/// that is not listed would be printed in full.
fn is_secret_option(arg: &str) -> bool {
    matches!(arg, "--token")
}

/// Quote a single argument for a POSIX shell if it needs it.
fn quote_arg(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/:+@=,".contains(c));

    if safe {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(args: &[&str]) -> String {
        build_invocation(args.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn records_the_arguments_as_typed() {
        assert_eq!(
            invocation(&["acme/tool@v1", "--shell", "fish", "-o", "install.fish"]),
            "eish acme/tool@v1 --shell fish -o install.fish"
        );
    }

    #[test]
    fn redacts_a_token_passed_as_a_separate_argument() {
        let command = invocation(&["acme/tool", "--token", "ghp_secret", "--list"]);
        assert!(!command.contains("ghp_secret"), "{command}");
        assert_eq!(command, "eish acme/tool --token <redacted> --list");
    }

    #[test]
    fn redacts_a_token_passed_with_equals() {
        let command = invocation(&["acme/tool", "--token=ghp_secret"]);
        assert!(!command.contains("ghp_secret"), "{command}");
        assert_eq!(command, "eish acme/tool --token=<redacted>");
    }

    #[test]
    fn redacts_a_token_that_starts_with_a_dash() {
        // The value is consumed regardless of its shape, so a secret cannot
        // survive by looking like the next option.
        let command = invocation(&["acme/tool", "--token", "-dashed", "--list"]);
        assert!(!command.contains("dashed"), "{command}");
        assert_eq!(command, "eish acme/tool --token <redacted> --list");
    }

    #[test]
    fn a_dangling_token_still_reports_the_option() {
        // clap complains about the missing value; the header should not invent
        // one.
        assert_eq!(
            invocation(&["acme/tool", "--token"]),
            "eish acme/tool --token <redacted>"
        );
    }

    #[test]
    fn quotes_arguments_that_need_it() {
        assert_eq!(
            invocation(&["acme/tool", "--dir", "/opt/my tools"]),
            "eish acme/tool --dir '/opt/my tools'"
        );
        // A quote inside a value needs the POSIX escape dance.
        assert_eq!(invocation(&["a b's"]), r"eish 'a b'\''s'");
        // Unquoted, the shell would expand this before `eish` saw it.
        assert_eq!(invocation(&["--dir", "~/bin"]), "eish --dir '~/bin'");
    }

    #[test]
    fn leaves_ordinary_arguments_unquoted() {
        assert_eq!(
            invocation(&[
                "https://github.com/a/b@v1.2.3",
                "--target=x86_64-unknown-linux-gnu"
            ]),
            "eish https://github.com/a/b@v1.2.3 --target=x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn only_the_token_option_is_secret() {
        assert!(is_secret_option("--token"));
        // A lookalike must not be redacted, or the header would lose detail.
        assert!(!is_secret_option("--token-file"));
        assert!(!is_secret_option("--tag"));
    }
}
