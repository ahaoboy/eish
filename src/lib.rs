//! Generate self-contained installation scripts for GitHub release binaries.
//!
//! `eish` is the tool behind installers like the one shipped in
//! `install.sh`: it inspects a GitHub release, works out which asset belongs to
//! which platform, and renders a script that performs that mapping at runtime.
//!
//! The generated script needs nothing but `curl`/`wget`, `tar` and `unzip` — in
//! particular it never calls the GitHub API, so it keeps working when the API
//! is rate limited or the machine is behind a flaky network.
//!
//! # Library usage
//!
//! ```
//! use eish::{InstallSpec, Proxy, Shell};
//! use eish::github::Release;
//!
//! # let release_json = r#"{
//! #   "tag_name": "v1.0.0",
//! #   "assets": [
//! #     {"name": "mytool-x86_64-unknown-linux-musl.tar.gz", "size": 1},
//! #     {"name": "mytool-aarch64-apple-darwin.tar.gz", "size": 1}
//! #   ]
//! # }"#;
//! let release: Release = serde_json::from_str(release_json).unwrap();
//!
//! let mut spec = InstallSpec::new("acme", "mytool")
//!     .with_shell(Shell::Bash)
//!     .with_proxy(Proxy::Xget);
//!
//! spec.apply_release_checked(&release).unwrap();
//! assert_eq!(spec.binary_name(), "mytool");
//!
//! let script = spec.render().unwrap();
//! assert!(script.starts_with("#!/usr/bin/env bash"));
//! ```
//!
//! # Command line usage
//!
//! ```text
//! eish owner/repo@v1.0.0 > install.sh
//! eish owner/repo --shell powershell > install.ps1
//! ```
//!
//! # Features
//!
//! * `cli` *(default)* — builds the `eish` binary and pulls in `clap`. Turn it
//!   off to depend on the library alone:
//!
//!   ```toml
//!   eish = { version = "0.1", default-features = false }
//!   ```

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

pub mod error;
pub mod github;
pub mod proxy;
pub mod render;
pub mod shell;
pub mod spec;
pub mod target;
pub mod token;

pub use error::{Error, Result};
pub use github::{Asset, Client, Release};
pub use proxy::Proxy;
pub use render::render;
pub use shell::Shell;
pub use spec::{AssetEntry, InstallSpec, RepoSpec, Resource};
pub use target::{KNOWN_TARGETS, guess_target, guess_targets};
pub use token::Token;
