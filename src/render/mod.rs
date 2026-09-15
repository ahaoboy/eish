//! Template rendering for the three supported shells.
//!
//! Templates live in `src/templates/` and are embedded into the binary with
//! [`include_str!`], so `eish` never needs to find files at runtime.
//!
//! minijinja is used with custom delimiters (`<% %>` for statements, `<{ }>`
//! for expressions) because the default `{{ }}` / `{% %}` markers collide with
//! shell parameter expansion such as `${#var}`.

use minijinja::Environment;
use minijinja::syntax::SyntaxConfig;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::spec::InstallSpec;
use crate::target;

mod delimiters {
    pub const BLOCK_START: &str = "<%";
    pub const BLOCK_END: &str = "%>";
    pub const VARIABLE_START: &str = "<{";
    pub const VARIABLE_END: &str = "}>";
    pub const COMMENT_START: &str = "<#";
    pub const COMMENT_END: &str = "#>";
}

/// The flat context handed to the templates.
#[derive(Debug, Serialize)]
struct Context<'a> {
    /// `owner/repo`.
    slug: &'a str,
    owner: &'a str,
    repo: &'a str,
    /// Tag requested by the user; `latest` means "follow the newest release".
    tag: &'a str,
    /// Tag the release actually has.
    resolved_tag: &'a str,
    /// Executable name.
    binary: &'a str,
    /// Shell dialect being generated.
    shell: &'a str,
    /// Default proxy baked into the installer.
    proxy: &'a str,
    /// `release` or `file`.
    resource_type: &'static str,
    /// Repository reference used for `resource_type == "file"`.
    resource_ref: &'a str,
    /// Default installation directory.
    install_dir: &'a str,
    /// Default minimum free disk space, in megabytes.
    min_disk_space_mb: u64,
    /// Default target triple, when one is forced.
    default_target: &'a str,
    /// Prebuilt binaries, sorted by target triple.
    assets: Vec<&'a crate::spec::AssetEntry>,
    /// Every supported target triple, in table order.
    targets: Vec<&'a str>,
    /// Detected triple -> compatible triples that this release ships.
    fallbacks: Vec<Fallback<'a>>,
    /// Version of `eish` that produced the script.
    eish_version: &'static str,
}

/// One entry of the generated fallback table.
#[derive(Debug, Serialize)]
struct Fallback<'a> {
    /// The triple the installer detected on the machine.
    target: &'static str,
    /// Compatible triples, best first, that this release does ship.
    alternatives: Vec<&'a str>,
}

impl<'a> Context<'a> {
    fn new(spec: &'a InstallSpec, slug: &'a str) -> Self {
        let mut assets: Vec<_> = spec.assets.iter().collect();
        assets.sort_by(|a, b| a.target.cmp(&b.target));
        let targets = assets.iter().map(|asset| asset.target.as_str()).collect();

        Self {
            slug,
            owner: &spec.owner,
            repo: &spec.repo,
            tag: &spec.tag,
            resolved_tag: spec.resolved_tag.as_deref().unwrap_or(&spec.tag),
            binary: spec.binary_name(),
            shell: spec.shell.as_str(),
            proxy: spec.proxy.as_str(),
            resource_type: spec.resource.as_str(),
            resource_ref: spec.resource.reference().unwrap_or("main"),
            install_dir: &spec.install_dir,
            min_disk_space_mb: spec.min_disk_space_mb,
            default_target: spec.default_target.as_deref().unwrap_or(""),
            fallbacks: build_fallbacks(spec),
            targets,
            assets,
            eish_version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// Build the runtime fallback table.
///
/// The installer first looks the detected triple up in the asset table. When
/// that fails, it walks the compatible triples listed here — so the fallbacks
/// only ever point at files this release actually ships, and only within one
/// platform (see [`crate::target::compatible_targets`]).
fn build_fallbacks(spec: &InstallSpec) -> Vec<Fallback<'static>> {
    let available: Vec<&str> = spec.assets.iter().map(|a| a.target.as_str()).collect();

    target::KNOWN_TARGETS
        .iter()
        .filter_map(|known| {
            let alternatives: Vec<&str> = target::compatible_targets(known)
                .into_iter()
                .filter(|candidate| available.contains(candidate))
                .collect();

            (!alternatives.is_empty()).then_some(Fallback {
                target: known,
                alternatives,
            })
        })
        .collect()
}

/// Render the installer described by `spec`.
///
/// The returned string is a complete, self-contained script: it embeds the
/// repository, the tag, the executable name and the asset table, and never
/// needs network access to GitHub's API.
pub fn render(spec: &InstallSpec) -> Result<String> {
    spec.validate()?;

    let mut env = Environment::new();
    env.set_syntax(
        SyntaxConfig::builder()
            .block_delimiters(delimiters::BLOCK_START, delimiters::BLOCK_END)
            .variable_delimiters(delimiters::VARIABLE_START, delimiters::VARIABLE_END)
            .comment_delimiters(delimiters::COMMENT_START, delimiters::COMMENT_END)
            .build()
            .map_err(|source| Error::Render {
                shell: spec.shell,
                source,
            })?,
    );
    env.set_keep_trailing_newline(true);
    env.add_template(spec.shell.template_id(), spec.shell.template())
        .map_err(|source| Error::Render {
            shell: spec.shell,
            source,
        })?;

    // `slug` is built here so that `Context` can borrow it for the whole render.
    let slug = spec.slug();
    let context = Context::new(spec, &slug);

    env.get_template(spec.shell.template_id())
        .and_then(|template| template.render(&context))
        .map_err(|source| Error::Render {
            shell: spec.shell,
            source,
        })
}
