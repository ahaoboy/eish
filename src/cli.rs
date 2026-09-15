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
