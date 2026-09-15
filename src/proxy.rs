//! Download proxies that can be embedded into the generated installers.
//!
//! A proxy only changes how the *generated script* builds download URLs. By
//! default the generated script talks to GitHub directly, which is what most
//! users want; the other variants exist for networks where `github.com` or
//! `objects.githubusercontent.com` are unreachable.

use std::fmt;
use std::str::FromStr;

use serde::Serialize;

use crate::error::Error;

/// A mirror / CDN that can be used to download release assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Proxy {
    /// Talk to `github.com` directly. No proxy is inserted.
    #[default]
    Github,
    /// Route requests through `gh-proxy.com`.
    GhProxy,
    /// Route requests through `xget.xi-xu.me`.
    Xget,
    /// Serve single files from `cdn.jsdelivr.net`.
    ///
    /// jsDelivr only mirrors repository files, so it cannot be used together
    /// with `--type release`.
    Jsdelivr,
    /// Serve single files from `cdn.statically.io`.
    ///
    /// Like jsDelivr this cannot be used together with `--type release`.
    Statically,
}

impl Proxy {
    /// Every proxy, in the order shown in the CLI help.
    pub const ALL: [Proxy; 5] = [
        Proxy::Github,
        Proxy::GhProxy,
        Proxy::Xget,
        Proxy::Jsdelivr,
        Proxy::Statically,
    ];

    /// Canonical name, as accepted by `--proxy`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Proxy::Github => "github",
            Proxy::GhProxy => "gh-proxy",
            Proxy::Xget => "xget",
            Proxy::Jsdelivr => "jsdelivr",
            Proxy::Statically => "statically",
        }
    }

    /// A comma separated list of [`Proxy::as_str`] values, for error messages.
    pub const fn list() -> &'static str {
        "github, gh-proxy, xget, jsdelivr, statically"
    }

    /// Short description used in the CLI help.
    pub const fn description(self) -> &'static str {
        match self {
            Proxy::Github => "download straight from github.com (no proxy)",
            Proxy::GhProxy => "mirror github.com through gh-proxy.com",
            Proxy::Xget => "mirror github.com through xget.xi-xu.me",
            Proxy::Jsdelivr => "serve repository files through cdn.jsdelivr.net",
            Proxy::Statically => "serve repository files through cdn.statically.io",
        }
    }

    /// Whether the proxy can serve `github.com/<owner>/<repo>/releases/...` URLs.
    ///
    /// The file-only CDNs cannot, which makes them incompatible with the
    /// `release` resource type.
    pub const fn supports_release_assets(self) -> bool {
        matches!(self, Proxy::Github | Proxy::GhProxy | Proxy::Xget)
    }
}

impl fmt::Display for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Proxy {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "github" | "direct" | "none" | "off" => Ok(Proxy::Github),
            "gh-proxy" | "ghproxy" => Ok(Proxy::GhProxy),
            "xget" => Ok(Proxy::Xget),
            "jsdelivr" => Ok(Proxy::Jsdelivr),
            "statically" => Ok(Proxy::Statically),
            other => Err(Error::UnknownProxy {
                input: other.to_string(),
                expected: Proxy::list(),
            }),
        }
    }
}

// Deriving the CLI help from the same metadata keeps `--help` and the error
// messages from drifting apart. Only the binary needs it.
#[cfg(feature = "cli")]
impl clap::ValueEnum for Proxy {
    fn value_variants<'a>() -> &'a [Self] {
        &Proxy::ALL
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(self.as_str()).help(self.description()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_aliases() {
        assert_eq!("direct".parse::<Proxy>().unwrap(), Proxy::Github);
        assert_eq!("ghproxy".parse::<Proxy>().unwrap(), Proxy::GhProxy);
        assert_eq!("XGet".parse::<Proxy>().unwrap(), Proxy::Xget);
    }

    #[test]
    fn server_only_cdns_reject_release_assets() {
        assert!(Proxy::Github.supports_release_assets());
        assert!(Proxy::GhProxy.supports_release_assets());
        assert!(!Proxy::Jsdelivr.supports_release_assets());
        assert!(!Proxy::Statically.supports_release_assets());
    }

    #[test]
    fn round_trips_through_str() {
        for proxy in Proxy::ALL {
            assert_eq!(proxy.as_str().parse::<Proxy>().unwrap(), proxy);
        }
    }
}
