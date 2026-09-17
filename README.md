# eish

Generate self-contained installation scripts for binaries published as GitHub
release assets.

`eish` inspects a release, works out which asset belongs to which platform, and
renders an `install.sh`, `install.fish` or `install.ps1` with that mapping baked
in. The generated script needs nothing but `curl`/`wget`, `tar` and `unzip` — it
never calls the GitHub API, so it keeps working when the API is rate limited or
the machine is behind a flaky network.

```sh
eish easy-install/easy-install > install.sh
eish cli/cli@v2.40.0 --shell powershell > install.ps1
eish https://github.com/quickjs-ng/quickjs --shell fish --proxy xget > install.fish
```

## Install

```sh
cargo install --path .
```

## Usage

```text
eish <SPEC> [OPTIONS]

  <SPEC>                    owner/repo, owner/repo@tag, or a full GitHub URL
  -s, --shell <SHELL>       bash | fish | powershell      [default: bash]
      --proxy <PROXY>       github | gh-proxy | xget |
                            jsdelivr | statically        [default: github]
      --tag <TAG>           tag to install, overrides `@tag`
  -b, --binary <BINARY>     executable name inside the archive
      --target <TARGET>     target triple the script defaults to
      --dir <DIR>           default install directory      [default: ~/.ei]
      --type <TYPE>         release | file                 [default: release]
      --ref <REF>           reference used by --type file
  -o, --output <PATH>       write to a file instead of stdout
      --release-json <P>    read the release from a JSON file
      --asset <FILE>        bundle an asset explicitly (repeatable)
      --list                list detected platforms and exit
  -q, --quiet               suppress progress messages
```

A few more examples:

```sh
# Which platforms would be supported?
eish quickjs-ng/quickjs --list

# Generate without touching the network at all
eish owner/repo --asset tool-linux-amd64 --asset tool-macos-arm64 > install.sh

# Pipe straight into a shell
eish owner/repo@v1.2.3 | bash
```

### Offline installs

The generated script can install from a file you already have, which is what
makes it usable on a machine with no network access. Download the asset once
while online:

```sh
curl -LO https://github.com/quickjs-ng/quickjs/releases/latest/download/qjs-linux-x86_64
```

The file name must match an asset of the release — that name is the only thing
the installer has to go on, and it is also what decides the target triple. When
several targets share one file, the machine's own platform picks between them.

```sh
./install.sh --file ./qjs-linux-x86_64
```

or:

```sh
EI_FILE=./qjs-linux-x86_64 ./install.sh
```

The PowerShell installer uses `-File`:

```powershell
.\install.ps1 -File .\qjs-windows-x86_64.exe
```

A file whose name is not an asset is rejected rather than guessed at, and the
message lists the names that would work.

### Credentials

GitHub allows 60 anonymous API requests per hour per IP, which is easy to
exhaust. `eish` looks for credentials rather than failing, in this order:

1. `--token`
2. `GITHUB_TOKEN` or `GH_TOKEN`
3. `gh auth token` (GitHub CLI)
4. `git credential fill` (Git Credential Manager)

It reports which one it used, so a later `403` is easy to attribute. Tokens
found by `gh` or `git` are only ever sent to github.com; tokens you supply
explicitly also go to a custom `--api-base`.

## Library

```rust
use eish::{Client, InstallSpec, Proxy, Shell};

let mut spec = InstallSpec::new("easy-install", "easy-install")
    .with_shell(Shell::Bash)
    .with_proxy(Proxy::Xget);

let release = Client::new().release("easy-install", "easy-install", None)?;
spec.apply_release_checked(&release)?;
std::fs::write("install.sh", spec.render()?)?;
```

The `cli` feature (on by default) builds the binary and pulls in `clap`. For the
library alone:

```toml
eish = { version = "0.1", default-features = false }
```

## What the generated scripts do

1. Detect the platform — OS, architecture and libc (so Alpine gets a static
   build and glibc distributions get a dynamic one).
2. Look it up in the embedded table to find the right asset.
3. Fall back to a compatible build (musl ↔ glibc, msvc ↔ gnu) when the exact
   one was not published.
4. Download, unpack, install into `~/.ei` and update `PATH`.

`--help` on any generated script documents its options, and each one has a
matching environment variable (`EI_DIR`, `EI_PROXY`, `EI_TARGET`, `EI_FILE`, …):

```sh
EI_DIR=/usr/local/bin ./install.sh
```

## Supported

Formats: `.tar.gz`, `.tgz`, `.tar.xz`, `.txz`, `.tar.bz2`, `.tbz2`, `.zip`,
`.gz`, `.exe`, and bare binaries such as `qjs-linux-x86_64`. Anything else
(checksums, `.msi`, `.deb`, `.bsdiff`, …) is ignored.

Targets: Linux (glibc and musl), macOS, Windows, Android and the BSDs, across
x86_64, aarch64, armv7, arm, i686, riscv64gc, loongarch64, powerpc64le and
s390x.

Asset names are matched by the
[`guess-target`](https://github.com/ahaoboy/guess-target) crate, covering both
Rust triples and the informal names releases actually use.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## License

MIT OR Apache-2.0.
