//! Mapping between release asset file names and Rust target triples.
//!
//! Guessing is delegated to the [`guess_target`] crate, which understands the
//! informal naming schemes real releases use (`jq-linux-amd64`,
//! `biome-darwin-arm64`, `tool-win64.exe`, `yasm-1.3.0-win64.exe`, …) as well
//! as Rust's standard triples.
//!
//! `eish` adds two things on top of it:
//!
//! * [`KNOWN_TARGETS`], the allow-list of triples the generated installers can
//!   detect at runtime. Filtering guesses through it keeps the baked-in table
//!   and the runtime detection in sync — a target the script cannot detect is
//!   useless in the table, and a target the script detects must be in the
//!   table.
//! * [`best_asset`] / [`rank_for`], the *reverse* lookup. The generated table
//!   is keyed by platform and asks "which asset should this machine install?",
//!   whereas `guess_target` answers the opposite question ("which platform is
//!   this file for?"). A release names one file per platform, but `guess_target`
//!   legitimately returns several candidates per file, so the two directions are
//!   not interchangeable.

use std::str::FromStr;

use guess_target::{Abi, Arch, GuessTarget, Target};

/// Rust target triples the generated installers can detect at runtime.
///
/// Every entry is a [`Target`] variant upstream, so the table can always be
/// populated for it. The list is intentionally narrower than
/// `Target::iter()`: it only contains triples the shell, fish and PowerShell
/// detectors actually produce.
pub const KNOWN_TARGETS: &[&str] = &[
    // Linux (glibc / musl)
    "x86_64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-gnu",
    "aarch64-unknown-linux-musl",
    "armv7-unknown-linux-gnueabihf",
    "armv7-unknown-linux-musleabihf",
    "arm-unknown-linux-gnueabihf",
    "arm-unknown-linux-musleabihf",
    "i686-unknown-linux-gnu",
    "i686-unknown-linux-musl",
    "riscv64gc-unknown-linux-gnu",
    "riscv64gc-unknown-linux-musl",
    "loongarch64-unknown-linux-gnu",
    "loongarch64-unknown-linux-musl",
    "powerpc64le-unknown-linux-gnu",
    "s390x-unknown-linux-gnu",
    // macOS
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    // Windows
    "x86_64-pc-windows-msvc",
    "x86_64-pc-windows-gnu",
    "i686-pc-windows-msvc",
    "i686-pc-windows-gnu",
    "aarch64-pc-windows-msvc",
    "aarch64-pc-windows-gnullvm",
    // Android
    "aarch64-linux-android",
    "armv7-linux-androideabi",
    "i686-linux-android",
    "x86_64-linux-android",
    // BSD
    "x86_64-unknown-freebsd",
    "x86_64-unknown-netbsd",
];

/// Archive and executable suffixes the generated installers can unpack.
///
/// [`is_installable_asset`] is an allow-list built from these, on purpose: an
/// asset only gets baked into an installer when the script can actually turn it
/// into a runnable binary. That rules out `.msi`/`.deb`/`.dmg` style system
/// packages, binary patches such as `.bsdiff` (they need the previous version
/// to apply) and sidecars like `.sha256` or `.json`.
pub const SUPPORTED_SUFFIXES: &[&str] = &[
    ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".zip", ".gz", ".exe",
];

/// Whether a release asset can be installed by the generated scripts.
///
/// A file qualifies when it carries one of [`SUPPORTED_SUFFIXES`], or when it
/// has no extension at all — plenty of tools publish a bare binary such as
/// `jq-linux-amd64` or `biome-darwin-arm64`.
///
/// ```
/// use eish::target::is_installable_asset;
///
/// assert!(is_installable_asset("tool-x86_64-unknown-linux-musl.tar.gz"));
/// assert!(is_installable_asset("jq-linux-amd64"));
/// assert!(is_installable_asset("tool-win64.exe"));
/// assert!(is_installable_asset("lo-linux-x64.gz"));
///
/// // Sidecars, patches and system packages are not installable.
/// assert!(!is_installable_asset("deno-x86_64-unknown-linux-gnu.zip.sha256sum"));
/// assert!(!is_installable_asset("deno-aarch64-apple-darwin.from-2.9.5.bsdiff"));
/// assert!(!is_installable_asset("starship-x86_64-pc-windows-msvc.msi"));
/// assert!(!is_installable_asset("ripgrep_15.2.0-1_amd64.deb"));
/// ```
pub fn is_installable_asset(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();

    if SUPPORTED_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(suffix))
    {
        return true;
    }

    // A bare binary: `jq-linux-amd64`, `biome-darwin-arm64`, `starship`.
    !lower.contains('.')
}

/// Architecture aliases found in release names, mapped to the architecture they
/// denote.
///
/// The spelling is deliberately asymmetric: `x86` maps to [`Arch::I686`] even
/// though upstream also accepts it as a synonym for [`Arch::X86_64`], because a
/// file called `qjs-linux-x86` is a 32-bit build. Mapping an alias to the most
/// specific architecture it can denote is what makes [`arch_precision`] able to
/// tell `qjs-linux-x86` and `qjs-linux-x86_64` apart.
///
/// ```
/// use eish::target::arch_precision;
///
/// // The 64-bit name names the 64-bit architecture; the other does not.
/// assert!(arch_precision("qjs-linux-x86_64", "x86_64-unknown-linux-gnu") == 6);
/// assert!(arch_precision("qjs-linux-x86", "x86_64-unknown-linux-gnu") == 0);
/// // …and the reverse for the 32-bit architecture.
/// assert!(arch_precision("qjs-linux-x86", "i686-unknown-linux-gnu") == 3);
/// assert!(arch_precision("qjs-linux-x86_64", "i686-unknown-linux-gnu") == 0);
/// ```
pub fn arch_precision(file_name: &str, target: &str) -> usize {
    let Ok(target) = Target::from_str(target) else {
        return 0;
    };
    let arch = target.arch();

    let lower = file_name.to_ascii_lowercase();
    // The *longest* alias present decides which architecture the name is
    // talking about, so `x86_64` is not read as an `x86` build.
    match ALIASES
        .iter()
        .filter(|(alias, _)| lower.contains(alias))
        .max_by_key(|(alias, _)| alias.len())
    {
        Some((alias, a)) if *a == arch => alias.len(),
        _ => 0,
    }
}

/// See [`arch_precision`].
const ALIASES: &[(&str, Arch)] = &[
    ("x86_64", Arch::X86_64),
    ("x86-64", Arch::X86_64),
    ("amd64", Arch::X86_64),
    ("x64", Arch::X86_64),
    ("i686", Arch::I686),
    ("i386", Arch::I686),
    ("ia32", Arch::I686),
    ("386", Arch::I686),
    ("x86", Arch::I686),
    ("aarch64", Arch::Aarch64),
    ("arm64", Arch::Aarch64),
    ("armv8", Arch::Aarch64),
    ("armv7l", Arch::Armv7),
    ("armv7", Arch::Armv7),
    ("armhf", Arch::Armv7),
    ("armv6", Arch::Arm),
    ("armel", Arch::Arm),
    ("arm", Arch::Arm),
    ("riscv64gc", Arch::Riscv64gc),
    ("riscv64", Arch::Riscv64gc),
    ("loongarch64", Arch::Loongarch64),
    ("powerpc64le", Arch::Powerpc64le),
    ("ppc64le", Arch::Powerpc64le),
    ("powerpc64", Arch::Powerpc64),
    ("ppc64", Arch::Powerpc64),
    ("s390x", Arch::S390x),
];

/// Minimum `guess_target` rank a match needs before it is trusted.
///
/// | rank | meaning                                              |
/// | ---- | ---------------------------------------------------- |
/// | 30   | explicit Rust target triple                          |
/// | 25   | explicit triple, no vendor                           |
/// | 10   | OS + arch + ABI                                      |
/// | 5    | OS + arch                                            |
/// | 2    | OS only (`jq-linux-armel`) or a bare `win64` token   |
/// | 1    | arch only (`jq-osx-amd64`)                           |
///
/// Everything below 5 is a guess about *one* axis only, and acting on it is how
/// `jq-osx-amd64` ends up offered to FreeBSD users. Requiring 5 costs the odd
/// release that ships nothing but `tool-win64.zip`, which is what `--asset` is
/// for.
pub const MIN_RANK: u32 = 5;

/// Every candidate match for an asset, restricted to [`KNOWN_TARGETS`].
///
/// Assets that are not installable ([`is_installable_asset`]) always yield an
/// empty list, which keeps every lookup in this module — [`guess_target`],
/// [`rank_for`], [`best_asset`] — from ever seeing a checksum or an `.msi`.
///
/// Results keep `guess_target`'s ordering, i.e. most specific first, and may
/// list the same target more than once when several naming rules apply.
pub fn candidates(file_name: &str) -> Vec<GuessTarget> {
    if !is_installable_asset(file_name) {
        return Vec::new();
    }

    guess_target::guess_target(file_name)
        .into_iter()
        .filter(|guess| is_known(guess.target))
        .collect()
}

/// Whether `target` is one of the triples the installers can detect.
fn is_known(target: Target) -> bool {
    KNOWN_TARGETS.contains(&target.to_str())
}

/// Rank of `file_name` as an asset for `target`.
///
/// Returns `0` when the file cannot be used for that target at all; higher
/// values mean a more specific match. This is the inverse of
/// [`guess_target`]: it answers "which asset should `target` install?".
///
/// ```
/// use eish::target::rank_for;
///
/// assert!(rank_for("jq-linux-amd64", "x86_64-unknown-linux-gnu") > 0);
/// // An amd64 build is not usable on ARM.
/// assert_eq!(rank_for("jq-linux-amd64", "aarch64-unknown-linux-gnu"), 0);
/// // Unrelated files score nothing.
/// assert_eq!(rank_for("checksums.txt", "x86_64-unknown-linux-gnu"), 0);
/// ```
pub fn rank_for(file_name: &str, target: &str) -> u32 {
    candidates(file_name)
        .iter()
        .filter(|guess| guess.target.to_str() == target)
        .map(|guess| guess.rank)
        .max()
        .unwrap_or(0)
}

/// The asset that best serves `target`, with its rank.
///
/// `files` is expected in release order; ties are resolved in favour of the
/// first file, which keeps the choice stable across runs. Matches below
/// `min_rank` are ignored — see [`MIN_RANK`].
///
/// Rank alone is not always decisive. Upstream accepts `x86` as a synonym for
/// both `i686` and `x86_64`, so `qjs-linux-x86` and `qjs-linux-x86_64` score
/// equally for an x86_64 target and the wrong (32-bit) file would win on release
/// order. Ties are therefore broken by [`arch_precision`], which prefers the
/// name that spells the architecture out.
///
/// ```
/// use eish::target::{best_asset, MIN_RANK};
///
/// let files = ["jq-linux-amd64", "jq-macos-arm64", "jq-windows-amd64.exe"];
/// let (file, rank) = best_asset(files, "x86_64-unknown-linux-gnu", MIN_RANK).unwrap();
/// assert_eq!(file, "jq-linux-amd64");
/// assert!(rank > 0);
///
/// // A dedicated ARM64 build beats the generic `win64` one.
/// let files = ["jq-win64.exe", "jq-windows-arm64.exe"];
/// assert_eq!(
///     best_asset(files, "aarch64-pc-windows-msvc", MIN_RANK).unwrap().0,
///     "jq-windows-arm64.exe"
/// );
///
/// // The 64-bit build wins even though it is listed after the 32-bit one.
/// let files = ["qjs-linux-x86", "qjs-linux-x86_64"];
/// assert_eq!(
///     best_asset(files, "x86_64-unknown-linux-gnu", MIN_RANK).unwrap().0,
///     "qjs-linux-x86_64"
/// );
/// assert_eq!(
///     best_asset(files, "i686-unknown-linux-gnu", MIN_RANK).unwrap().0,
///     "qjs-linux-x86"
/// );
/// ```
pub fn best_asset<'a, I>(files: I, target: &str, min_rank: u32) -> Option<(&'a str, u32)>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut best: Option<(&'a str, u32, usize)> = None;

    for file in files {
        let rank = rank_for(file, target);
        if rank < min_rank {
            continue;
        }

        let score = (rank, arch_precision(file, target));
        if best.is_none_or(|(_, best_rank, best_precision)| score > (best_rank, best_precision)) {
            best = Some((file, rank, score.1));
        }
    }

    best.map(|(file, rank, _)| (file, rank))
}

/// The most likely target for an asset.
///
/// ```
/// use eish::guess_target;
///
/// assert_eq!(
///     guess_target("tool-x86_64-unknown-linux-musl.tar.gz"),
///     Some("x86_64-unknown-linux-musl"),
/// );
/// assert_eq!(guess_target("source.tar.gz"), None);
/// ```
pub fn guess_target(file_name: &str) -> Option<&'static str> {
    candidates(file_name)
        .first()
        .map(|guess| guess.target.to_str())
}

/// Every target an asset is a *best* candidate for.
///
/// Usually a single triple. Ambiguous names such as `tool-linux-armv7.tar.gz`
/// yield several, and macOS "universal" binaries yield both Darwin targets
/// because they genuinely run on either CPU.
///
/// ```
/// use eish::guess_targets;
///
/// assert_eq!(
///     guess_targets("tool-darwin-universal.tar.gz"),
///     vec!["x86_64-apple-darwin", "aarch64-apple-darwin"],
/// );
/// assert_eq!(guess_targets("checksums.txt"), Vec::<&str>::new());
/// ```
pub fn guess_targets(file_name: &str) -> Vec<&'static str> {
    let candidates = candidates(file_name);
    let Some(top_rank) = candidates.iter().map(|guess| guess.rank).max() else {
        return Vec::new();
    };

    let mut targets: Vec<&'static str> = candidates
        .iter()
        .filter(|guess| guess.rank == top_rank)
        .map(|guess| guess.target.to_str())
        .collect();
    targets.dedup();
    targets
}

/// The executable name implied by an asset file name.
///
/// `guess_target` reports the tool name it stripped off the platform, so
/// `jq-linux-amd64` yields `jq`. Platform-only leftovers such as
/// `tool-universal2-apple-darwin` are cleaned up by [`crate::spec`]'s noise
/// stripping.
pub fn guess_binary_name(file_name: &str) -> Option<String> {
    candidates(file_name)
        .first()
        .map(|guess| guess.name.clone())
        .filter(|name| !name.is_empty())
}

/// Triples that can run a binary built for `target`.
///
/// Mirrors `guess_target`'s own ABI compatibility rules (private upstream) and
/// extends them to the `eabi`/`eabihf` variants and the `gnullvm` Windows ABI,
/// so a musl request can fall back to a glibc build (and vice versa) on every
/// architecture `eish` supports.
///
/// ```
/// use eish::target::compatible_targets;
///
/// assert_eq!(
///     compatible_targets("x86_64-unknown-linux-musl"),
///     vec!["x86_64-unknown-linux-gnu"],
/// );
/// ```
pub fn compatible_targets(target: &str) -> Vec<&'static str> {
    let Ok(target) = Target::from_str(target) else {
        return Vec::new();
    };

    KNOWN_TARGETS
        .iter()
        .copied()
        .filter(|other| {
            let Ok(other) = Target::from_str(other) else {
                return false;
            };
            other != target
                && other.arch() == target.arch()
                && other.os() == target.os()
                && is_compatible_abi(other.abi(), target.abi())
        })
        .collect()
}

/// Whether two ABIs can substitute for each other on the same OS and CPU.
///
/// Only same-architecture pairs qualify: a 32-bit build is not a substitute for
/// a 64-bit one, and `arm` code does not run on `armv7` cores.
fn is_compatible_abi(a: Option<Abi>, b: Option<Abi>) -> bool {
    matches!(
        (a, b),
        // Linux: musl and glibc builds are interchangeable.
        (Some(Abi::Musl), Some(Abi::Gnu))
            | (Some(Abi::Gnu), Some(Abi::Musl))
            | (Some(Abi::Musleabi), Some(Abi::Gnueabi))
            | (Some(Abi::Gnueabi), Some(Abi::Musleabi))
            | (Some(Abi::Musleabihf), Some(Abi::Gnueabihf))
            | (Some(Abi::Gnueabihf), Some(Abi::Musleabihf))
            // Windows: both ABIs link against the same system libraries.
            | (Some(Abi::Msvc), Some(Abi::Gnu))
            | (Some(Abi::Gnu), Some(Abi::Msvc))
            | (Some(Abi::Msvc), Some(Abi::Gnullvm))
            | (Some(Abi::Gnullvm), Some(Abi::Msvc))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The allow-list is a contract with the shell detectors, so a typo would
    /// silently produce an installer that can never match anything.
    #[test]
    fn every_known_target_is_a_real_target() {
        for target in KNOWN_TARGETS {
            assert!(
                Target::from_str(target).is_ok(),
                "{target} is not a target triple recognised by guess-target"
            );
        }
    }

    #[test]
    fn known_targets_are_unique() {
        let mut sorted = KNOWN_TARGETS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), KNOWN_TARGETS.len(), "duplicate entries");
    }

    #[test]
    fn guesses_canonical_triples() {
        let cases = [
            ("ei-x86_64-pc-windows-gnu.zip", "x86_64-pc-windows-gnu"),
            (
                "tool-aarch64-unknown-linux-musl.tar.gz",
                "aarch64-unknown-linux-musl",
            ),
            ("tool-aarch64-apple-darwin.tar.gz", "aarch64-apple-darwin"),
            (
                "tool_armv7-unknown-linux-gnueabihf.tar.gz",
                "armv7-unknown-linux-gnueabihf",
            ),
            (
                "tool-x86_64-unknown-linux-musl",
                "x86_64-unknown-linux-musl",
            ),
            (
                "deno-x86_64-unknown-linux-gnu.zip",
                "x86_64-unknown-linux-gnu",
            ),
            (
                "tool-i686-unknown-linux-musl.tar.gz",
                "i686-unknown-linux-musl",
            ),
            (
                "tool-aarch64-pc-windows-gnullvm.zip",
                "aarch64-pc-windows-gnullvm",
            ),
            (
                "tool-s390x-unknown-linux-gnu.tar.gz",
                "s390x-unknown-linux-gnu",
            ),
            (
                "tool-powerpc64le-unknown-linux-gnu.tar.gz",
                "powerpc64le-unknown-linux-gnu",
            ),
            ("tool-x86_64-unknown-netbsd.tar.gz", "x86_64-unknown-netbsd"),
        ];

        for (file, expected) in cases {
            assert_eq!(guess_target(file), Some(expected), "for {file}");
        }
    }

    #[test]
    fn guesses_loosely_named_assets() {
        let cases = [
            ("tool-linux-amd64.tar.gz", "x86_64-unknown-linux-gnu"),
            ("tool-linux-x86_64-musl.zip", "x86_64-unknown-linux-musl"),
            ("jq-linux-arm64", "aarch64-unknown-linux-gnu"),
            ("tool-darwin-arm64.tar.gz", "aarch64-apple-darwin"),
            ("tool-macos-x64.zip", "x86_64-apple-darwin"),
            ("biome-darwin-arm64", "aarch64-apple-darwin"),
            ("tool-win64.zip", "x86_64-pc-windows-gnu"),
            ("jq-windows-arm64.exe", "aarch64-pc-windows-msvc"),
            ("tool-windows-x86_64-gnu.zip", "x86_64-pc-windows-gnu"),
            ("jq-linux-i386", "i686-unknown-linux-gnu"),
            ("jq-linux-riscv64", "riscv64gc-unknown-linux-gnu"),
            ("jq-linux-s390x", "s390x-unknown-linux-gnu"),
            ("mytool_1.2.3_windows_x86_64.zip", "x86_64-pc-windows-gnu"),
            (
                "mpv-v0.41.0-x86_64-pc-windows-msvc.zip",
                "x86_64-pc-windows-msvc",
            ),
        ];

        for (file, expected) in cases {
            assert_eq!(guess_target(file), Some(expected), "for {file}");
        }
    }

    #[test]
    fn ignores_non_binary_assets() {
        for file in [
            "source.tar.gz",
            "checksums.txt",
            "tool.spdx.json",
            "tool.md5",
            "custom-name.bin",
        ] {
            assert_eq!(guess_target(file), None, "for {file}");
            assert_eq!(rank_for(file, "x86_64-unknown-linux-gnu"), 0, "for {file}");
        }
    }

    #[test]
    fn never_returns_an_undetectable_target() {
        // `jq-linux-amd64` also matches `x86_64-unknown-linux-gnux32` and
        // `-ohos` upstream, neither of which the installers can detect.
        for guess in candidates("jq-linux-amd64") {
            assert!(
                KNOWN_TARGETS.contains(&guess.target.to_str()),
                "{} leaked into the candidate list",
                guess.target.to_str()
            );
        }
    }

    #[test]
    fn universal_darwin_covers_both_architectures() {
        for file in [
            "tool-darwin-universal.tar.gz",
            "tool-universal2-apple-darwin.tar.gz",
        ] {
            let mut targets = guess_targets(file);
            targets.sort_unstable();
            assert_eq!(targets, vec!["aarch64-apple-darwin", "x86_64-apple-darwin"]);
        }
    }

    #[test]
    fn ties_between_abis_are_reported_together() {
        // `jq-linux-amd64` does not say which libc it was built against, so
        // both the glibc and the musl build are equally valid answers.
        let mut targets = guess_targets("jq-linux-amd64");
        targets.sort_unstable();
        assert_eq!(
            targets,
            vec!["x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl"]
        );

        // A name that does pin the libc and ABI yields exactly one answer.
        for file in [
            "tool-x86_64-unknown-linux-musl.tar.gz",
            "lo-linux-musl-x64.gz",
            "jq-macos-arm64",
            "tool-darwin-arm64.tar.gz",
            "tool-x86_64-pc-windows-gnu.zip",
            "tool-i686-unknown-linux-gnu.tar.gz",
        ] {
            assert_eq!(guess_targets(file).len(), 1, "for {file}");
        }

        // Windows msvc and gnullvm builds are interchangeable, so a name that
        // names neither reports both.
        let mut targets = guess_targets("jq-windows-arm64.exe");
        targets.sort_unstable();
        assert_eq!(
            targets,
            vec!["aarch64-pc-windows-gnullvm", "aarch64-pc-windows-msvc"]
        );
    }

    #[test]
    fn picks_the_best_asset_per_target() {
        // Two files, both usable on x86_64 Linux, but the musl one is explicit.
        let files = ["lo-linux-x64.gz", "lo-linux-musl-x64.gz"];
        assert_eq!(
            best_asset(files, "x86_64-unknown-linux-musl", MIN_RANK)
                .unwrap()
                .0,
            "lo-linux-musl-x64.gz"
        );
        assert_eq!(
            best_asset(files, "x86_64-unknown-linux-gnu", MIN_RANK)
                .unwrap()
                .0,
            "lo-linux-x64.gz"
        );

        // An arch specific build beats a generic `amd64` one.
        let files = ["jq-linux-amd64", "jq-linux-arm64"];
        assert_eq!(
            best_asset(files, "aarch64-unknown-linux-gnu", MIN_RANK)
                .unwrap()
                .0,
            "jq-linux-arm64"
        );
        assert_eq!(
            best_asset(files, "x86_64-unknown-linux-gnu", MIN_RANK)
                .unwrap()
                .0,
            "jq-linux-amd64"
        );

        // `jq-win64.exe` only implies "some Windows", so once a file naming the
        // architecture shows up it wins on both counts: a higher rank and a
        // lower-ranked rival being dropped entirely.
        let files = [
            "jq-win64.exe",
            "jq-windows-amd64.exe",
            "jq-windows-arm64.exe",
        ];
        assert_eq!(
            best_asset(files, "aarch64-pc-windows-msvc", MIN_RANK)
                .unwrap()
                .0,
            "jq-windows-arm64.exe"
        );
        assert_eq!(
            best_asset(files, "x86_64-pc-windows-msvc", MIN_RANK)
                .unwrap()
                .0,
            "jq-windows-amd64.exe"
        );
    }

    #[test]
    fn ties_keep_the_first_asset() {
        // Both files describe the same platform, so release order decides.
        let files = [
            "tool-x86_64-unknown-linux-musl.zip",
            "tool-x86_64-unknown-linux-musl.tar.gz",
        ];
        assert_eq!(
            best_asset(files, "x86_64-unknown-linux-musl", MIN_RANK)
                .unwrap()
                .0,
            "tool-x86_64-unknown-linux-musl.zip"
        );
    }

    #[test]
    fn prefers_the_name_that_spells_out_the_architecture() {
        // `x86` is an alias for both i686 and x86_64 upstream, so these two
        // files score equally for an x86_64 target. Without a tie-break the
        // 32-bit build wins on release order, which is a silent
        // "exec format error" for the user.
        let files = ["qjs-linux-x86", "qjs-linux-x86_64"];
        assert_eq!(
            best_asset(files, "x86_64-unknown-linux-gnu", MIN_RANK)
                .unwrap()
                .0,
            "qjs-linux-x86_64"
        );
        assert_eq!(
            best_asset(files, "i686-unknown-linux-gnu", MIN_RANK)
                .unwrap()
                .0,
            "qjs-linux-x86"
        );

        // The same ambiguity exists on Windows.
        let files = ["qjs-windows-x86.exe", "qjs-windows-x86_64.exe"];
        assert_eq!(
            best_asset(files, "x86_64-pc-windows-msvc", MIN_RANK)
                .unwrap()
                .0,
            "qjs-windows-x86_64.exe"
        );
        assert_eq!(
            best_asset(files, "i686-pc-windows-msvc", MIN_RANK)
                .unwrap()
                .0,
            "qjs-windows-x86.exe"
        );

        // `arm64` and `armv8` are the spelled-out AArch64 names.
        let files = ["tool-linux-arm", "tool-linux-arm64"];
        assert_eq!(
            best_asset(files, "aarch64-unknown-linux-gnu", MIN_RANK)
                .unwrap()
                .0,
            "tool-linux-arm64"
        );
        assert_eq!(
            best_asset(files, "arm-unknown-linux-gnueabihf", MIN_RANK)
                .unwrap()
                .0,
            "tool-linux-arm"
        );
    }

    #[test]
    fn arch_precision_reads_the_longest_alias() {
        // `x86_64` contains `x86`, but the longer alias decides.
        assert!(arch_precision("qjs-linux-x86_64", "x86_64-unknown-linux-gnu") > 0);
        assert_eq!(
            arch_precision("qjs-linux-x86", "x86_64-unknown-linux-gnu"),
            0
        );
        assert!(arch_precision("qjs-linux-x86", "i686-unknown-linux-gnu") > 0);

        // `arm64` contains `arm`.
        assert_eq!(
            arch_precision("tool-arm64", "arm-unknown-linux-gnueabihf"),
            0
        );
        assert!(arch_precision("tool-arm64", "aarch64-unknown-linux-gnu") > 0);
        assert!(arch_precision("tool-arm", "arm-unknown-linux-gnueabihf") > 0);

        // A name with no architecture at all scores nothing.
        assert_eq!(arch_precision("tool-linux", "x86_64-unknown-linux-gnu"), 0);
    }

    #[test]
    fn ignores_vague_matches_below_the_threshold() {
        // `jq-osx-amd64` only reveals the architecture. Acting on it would
        // offer a macOS binary to FreeBSD, NetBSD and Android users.
        for target in [
            "x86_64-unknown-freebsd",
            "x86_64-unknown-netbsd",
            "x86_64-linux-android",
            // Even the correct platform is not trusted from an arch-only name:
            // the release would have to offer a better Darwin asset.
            "x86_64-apple-darwin",
        ] {
            assert_eq!(best_asset(["jq-osx-amd64"], target, MIN_RANK), None);
        }

        // An OS-only token is equally unusable.
        assert_eq!(
            best_asset(
                ["jq-linux-armel"],
                "powerpc64le-unknown-linux-gnu",
                MIN_RANK
            ),
            None
        );

        // Dropping the threshold accepts them, which is what `--asset` relies
        // on: the user named the file explicitly.
        assert_eq!(
            best_asset(["jq-osx-amd64"], "x86_64-apple-darwin", 1)
                .unwrap()
                .0,
            "jq-osx-amd64"
        );
    }

    #[test]
    fn reports_the_tool_name() {
        assert_eq!(guess_binary_name("jq-linux-amd64").as_deref(), Some("jq"));
        assert_eq!(
            guess_binary_name("starship-x86_64-unknown-linux-musl.tar.gz").as_deref(),
            Some("starship")
        );
        assert_eq!(guess_binary_name("checksums.txt"), None);
    }

    #[test]
    fn accepts_only_unpackable_formats() {
        for file in [
            "tool-x86_64-unknown-linux-musl.tar.gz",
            "tool-x86_64-unknown-linux-gnu.tgz",
            "tool-x86_64-unknown-linux-musl.tar.xz",
            "tool-x86_64-pc-windows-gnu.zip",
            "tool-win64.exe",
            "lo-linux-x64.gz",
            // Bare binaries carry no extension at all.
            "jq-linux-amd64",
            "biome-darwin-arm64",
            "starship",
        ] {
            assert!(is_installable_asset(file), "should accept {file}");
        }
    }

    #[test]
    fn rejects_sidecars_patches_and_system_packages() {
        for file in [
            // Checksums, signatures and metadata that ship next to the binary.
            "deno-x86_64-unknown-linux-gnu.zip.sha256sum",
            "starship-x86_64-apple-darwin.tar.gz.sha256",
            "tool-x86_64-unknown-linux-gnu.tar.gz.asc",
            "lib.deno.d.ts",
            "jq-attestation.json",
            // Binary patches need the previous version to apply.
            "deno-aarch64-apple-darwin.from-2.9.5.bsdiff",
            // System packages the installers cannot unpack.
            "starship-x86_64-pc-windows-msvc.msi",
            "ripgrep_15.2.0-1_amd64.deb",
            "tool-1.0.0.x86_64.rpm",
            "tool-1.0.0.dmg",
            "tool-1.0.0.AppImage",
        ] {
            assert!(!is_installable_asset(file), "should reject {file}");
            // And, crucially, must never reach the asset table.
            assert_eq!(rank_for(file, "x86_64-unknown-linux-gnu"), 0, "for {file}");
        }
    }

    #[test]
    fn keeps_abi_fallbacks_within_one_platform() {
        assert_eq!(
            compatible_targets("x86_64-unknown-linux-musl"),
            vec!["x86_64-unknown-linux-gnu"]
        );
        assert_eq!(
            compatible_targets("x86_64-pc-windows-msvc"),
            vec!["x86_64-pc-windows-gnu"]
        );
        assert_eq!(
            compatible_targets("armv7-unknown-linux-gnueabihf"),
            vec!["armv7-unknown-linux-musleabihf"]
        );
        assert_eq!(
            compatible_targets("aarch64-apple-darwin"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn never_falls_back_across_architectures() {
        // A 32-bit build is not a substitute for a 64-bit one, and `arm` code
        // does not run on `armv7` cores.
        assert!(
            !compatible_targets("x86_64-unknown-linux-gnu").contains(&"i686-unknown-linux-gnu")
        );
        assert!(
            !compatible_targets("arm-unknown-linux-gnueabihf")
                .contains(&"armv7-unknown-linux-gnueabihf")
        );
        assert!(!compatible_targets("aarch64-pc-windows-msvc").contains(&"x86_64-pc-windows-msvc"));
    }
}
