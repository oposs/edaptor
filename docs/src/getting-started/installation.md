# Installation

Releases after 1.7.0 are published as a Homebrew formula, Debian and RPM
packages, and archives. Releases up to 1.7.0 ship archives only.

## Homebrew (macOS and Linux)

The eDAPtor repository is its own tap:

```bash
brew tap oposs/edaptor https://github.com/oposs/edaptor
brew install edaptor
```

The formula installs the binary, the man page and the annotated example
configuration under `$(brew --prefix)/share/edaptor/examples/`.

## Debian and Ubuntu

Packages for `amd64` and `arm64` are in the OETIKER+PARTNER package registry:

```bash
sudo install -d -m 0755 /etc/apt/keyrings
sudo curl -o /etc/apt/keyrings/gitea-oposs.asc https://gitea.oetiker.ch/api/packages/oposs/debian/repository.key
echo "deb [signed-by=/etc/apt/keyrings/gitea-oposs.asc] https://gitea.oetiker.ch/api/packages/oposs/debian stable main" | sudo tee /etc/apt/sources.list.d/oposs.list
sudo apt update
sudo apt install edaptor
```

## Fedora, RHEL, Rocky and Alma

Packages for `x86_64` and `aarch64`. On Fedora 41 and later (dnf5):

```bash
sudo dnf config-manager addrepo --from-repofile=https://gitea.oetiker.ch/api/packages/oposs/rpm.repo
sudo dnf install edaptor
```

On RHEL, Rocky, Alma and Fedora before 41 (dnf4):

```bash
sudo dnf config-manager --add-repo https://gitea.oetiker.ch/api/packages/oposs/rpm.repo
sudo dnf install edaptor
```

The packages install `/usr/bin/edaptor`, the man page `edaptor(1)` and the
annotated example configuration in `/usr/share/doc/edaptor/examples/`.

## Archives

Each [GitHub Release](https://github.com/oposs/edaptor/releases) carries
archives for Linux (`x86_64`/`aarch64`, static musl), macOS (Intel and Apple
Silicon), Windows, and **illumos** (`x86_64`, for example OmniOS). The illumos
binary is dynamically linked (illumos has no static libc) and links only
against base system libraries. Each archive holds the binary, the man page
(except on Windows), the example configurations, the README and the licence.

## Building from source

eDAPtor is a Rust application. With a recent stable Rust toolchain installed,
build a release binary with:

```bash
cargo build --release --bin edaptor
```

The resulting binary is at `target/release/edaptor`. `make man` builds the man
page `man/edaptor.1` from `docs/manual.md`; it needs `pandoc`.

The repository pins its tools with [mise](https://mise.jdx.dev/). If you use
mise, `mise install` picks up the pinned Rust toolchain (and the docs toolchain)
automatically, so you do not have to manage versions by hand.

## TLS backend

eDAPtor uses the [rustls](https://github.com/rustls/rustls) TLS backend, so
**no OpenSSL is needed** to build or run it. This also means static,
self-contained `musl` release binaries can be produced without vendoring a TLS
library.

## Configuration file

eDAPtor reads a single TOML configuration file. Point it at one explicitly with
`--config <path>`, or let it search `~/.config/edaptor/` and `/etc/edaptor/`
for `*.toml` files (one file found is loaded, several open a picker):

```bash
edaptor --config /path/to/config.toml
```

The config file declares your server connection and authentication; entry
profiles are detected from the directory and can be overridden. See the
[Configuration](../configuration/overview.md) section for the full reference.
