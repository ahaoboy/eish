//! `eish` command line interface.
//!
//! The heavy lifting lives in the library crate; this binary only parses
//! arguments and prints the result.

mod cli;

use clap::Parser;

fn main() -> std::process::ExitCode {
    let cli = cli::Cli::parse();
    // Logging is installed after parsing because `--verbose` and `--quiet`
    // decide its level.
    cli.init_logging();
    cli.run()
}
