//! Command line front-end for `eish`.
//!
//! Everything the user can configure lives here; the actual work is delegated
//! to the library so that `eish` stays scriptable and testable.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::builder::PossibleValue;
use clap::{Parser, ValueEnum};
use log::{Level, LevelFilter, info};

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
    version = eish::VERSION,
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

    /// Install this program, when the release publishes several.
    ///
    /// Matches the name `eish` reports for the asset (`crash` and `crash-full`
    /// are different programs). Use `--list` on the repository to see the
    /// available names.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,

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

    /// Print progress messages.
    ///
    /// Repeat for more detail. Progress goes to stderr, so it never mixes with
    /// the script or the `--list` table on stdout.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Suppress everything but errors.
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
    /// Install the process-wide logger.
    ///
    /// Progress and warnings go to stderr through [`log`], which keeps stdout
    /// reserved for the generated script and the `--list` table — both are
    /// meant to be piped into something else, and a stray "querying GitHub"
    /// line in the middle of them is at best noise and at worst a parse error.
    ///
    /// The default level is `warn`, so a plain run is silent unless something
    /// needs attention. `-v` raises it to `info` and `-vv` to `debug`; `-q`
    /// drops it to errors only and wins if both are given, since it is the
    /// stricter request. `EISH_LOG` (or `RUST_LOG`) overrides all of this.
    pub fn init_logging(&self) {
        let default = if self.quiet {
            LevelFilter::Error
        } else {
            match self.verbose {
                0 => LevelFilter::Warn,
                1 => LevelFilter::Info,
                _ => LevelFilter::Debug,
            }
        };

        let mut builder = env_logger::Builder::new();
        builder.filter_level(default);
        // Parsed last, so an explicit setting beats the flags above.
        builder.parse_env("EISH_LOG");
        builder.parse_default_env();
        builder.format(|buffer, record| {
            use std::io::Write as _;
            let message = record.args();
            match record.level() {
                Level::Error => writeln!(buffer, "error: {message}"),
                // The message already reads as a sentence, so the level word is
                // only worth printing when it is not the ordinary case.
                Level::Warn => writeln!(buffer, "warning: {message}"),
                _ => writeln!(buffer, "eish: {message}"),
            }
        });
        let _ = builder.try_init();
    }

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
            .with_name(self.name.clone())
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
                let client = Client::new().with_api_base(self.api_base.clone());

                // Say which credential is in play, so a later 403 is easy to
                // attribute to the wrong token rather than to rate limiting.
                if let Some(source) = client.credential_source() {
                    info!("using credentials from {source}");
                }

                info!("querying GitHub for {}", spec.slug());
                Some(client.release(&spec.owner, &spec.repo, Some(&spec.tag))?)
            }
        };

        match &release {
            Some(release) => spec.apply_release_checked(release)?,
            None => {
                if spec.apply_assets(self.assets.iter().map(String::as_str)) == 0 {
                    return Err(spec.no_assets_error());
                }
            }
        }

        spec.validate()?;

        if self.list {
            for asset in &spec.assets {
                println!("{}\t{}\t{}", asset.program, asset.target, asset.filename);
            }
            return Ok(());
        }

        info!(
            "found {} target(s) for {} ({})",
            spec.assets.len(),
            spec.slug(),
            spec.resolved_tag.as_deref().unwrap_or(&spec.tag)
        );
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
                info!("wrote {}", path.display());
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
}

/// The command line that produced this process, for the generated header.
///
/// Taken from `argv` rather than rebuilt from the parsed options so that it
/// stays exactly what the user typed — including things the spec does not model,
/// such as `--release-json <path>`. The program name is normalised to `eish` so
/// the printed command does not depend on where the binary happens to live.
///
/// Nothing here needs hiding: credentials come from the environment or a helper
/// rather than the command line, so a generated script can be committed as-is.
fn invocation() -> String {
    let mut args = std::env::args();
    let _program = args.next();
    build_invocation(args)
}

/// Build the header command from the arguments after the program name.
///
/// Split out from [`invocation`] so the quoting can be tested without spawning
/// a process.
fn build_invocation(args: impl IntoIterator<Item = String>) -> String {
    std::iter::once("eish".to_string())
        .chain(args.into_iter().map(|arg| quote_arg(&arg)))
        .collect::<Vec<_>>()
        .join(" ")
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

    /// Credentials never reach the command line, so the header is safe to share
    /// as-is. This guards against someone adding a secret-bearing flag later
    /// without thinking about the generated header.
    #[test]
    fn no_cli_option_is_secret() {
        use clap::CommandFactory as _;

        let command = Cli::command();
        let suspicious: Vec<&str> = command
            .get_arguments()
            .filter_map(|arg| arg.get_long())
            .filter(|long| {
                let long = long.to_ascii_lowercase();
                long.contains("token") || long.contains("password") || long.contains("secret")
            })
            .collect();

        assert!(
            suspicious.is_empty(),
            "these options look secret and would be written into generated scripts: {suspicious:?}"
        );
    }
}
