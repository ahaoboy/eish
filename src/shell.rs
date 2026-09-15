//! The set of shells that `eish` can generate installers for.

use std::fmt;
use std::str::FromStr;

use serde::Serialize;

use crate::error::Error;

/// A shell dialect supported by the generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Shell {
    /// POSIX-ish `bash`/`sh`, usable on Linux, macOS, MSYS2, Cygwin and WSL.
    #[default]
    Bash,
    /// The `fish` shell, including `fish` running on Windows through MSYS2.
    Fish,
    /// Windows PowerShell 5.1 and PowerShell 7+.
    PowerShell,
}

impl Shell {
    /// Every shell, in the order shown in the CLI help.
    pub const ALL: [Shell; 3] = [Shell::Bash, Shell::Fish, Shell::PowerShell];

    /// Canonical lower-case name, as accepted by `--shell`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Fish => "fish",
            Shell::PowerShell => "powershell",
        }
    }

    /// A comma separated list of [`Shell::as_str`] values, for error messages.
    pub const fn list() -> &'static str {
        "bash, fish, powershell"
    }

    /// Short description used in the CLI help.
    pub const fn description(self) -> &'static str {
        match self {
            Shell::Bash => "POSIX shell installer (Linux, macOS, MSYS2, Cygwin, WSL)",
            Shell::Fish => "fish shell installer (Linux, macOS, MSYS2)",
            Shell::PowerShell => "PowerShell installer (Windows PowerShell 5.1 and PowerShell 7+)",
        }
    }

    /// Name of the embedded template; also used as the `minijinja` template id.
    pub const fn template_id(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Fish => "fish",
            Shell::PowerShell => "powershell",
        }
    }

    /// Embedded template source.
    pub const fn template(self) -> &'static str {
        match self {
            Shell::Bash => include_str!("templates/bash.sh"),
            Shell::Fish => include_str!("templates/fish.fish"),
            Shell::PowerShell => include_str!("templates/powershell.ps1"),
        }
    }
}

impl fmt::Display for Shell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Shell {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "bash" | "sh" | "shell" | "posix" => Ok(Shell::Bash),
            "fish" => Ok(Shell::Fish),
            "powershell" | "pwsh" | "ps" | "ps1" | "windows" => Ok(Shell::PowerShell),
            other => Err(Error::UnknownShell {
                input: other.to_string(),
                expected: Shell::list(),
            }),
        }
    }
}

// `clap` is a hard dependency of this crate, and deriving the CLI help from the
// same metadata keeps `--help` and the error messages from drifting apart.
impl clap::ValueEnum for Shell {
    fn value_variants<'a>() -> &'a [Self] {
        &Shell::ALL
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
        assert_eq!("sh".parse::<Shell>().unwrap(), Shell::Bash);
        assert_eq!("powershell".parse::<Shell>().unwrap(), Shell::PowerShell);
        assert_eq!("PWSH".parse::<Shell>().unwrap(), Shell::PowerShell);
    }

    #[test]
    fn rejects_unknown_shell() {
        let err = "csh".parse::<Shell>().unwrap_err();
        assert!(err.to_string().contains("unknown shell"));
    }

    #[test]
    fn round_trips_through_str() {
        for shell in Shell::ALL {
            assert_eq!(shell.as_str().parse::<Shell>().unwrap(), shell);
        }
    }
}
