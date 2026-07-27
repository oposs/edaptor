# Installation

## Building from source

eDAPtor is a Rust application. With a recent stable Rust toolchain installed,
build a release binary with:

```bash
cargo build --release
```

The resulting binary is at `target/release/edaptor`.

The repository pins its tools with [mise](https://mise.jdx.dev/). If you use
mise, `mise install` picks up the pinned Rust toolchain (and the docs toolchain)
automatically, so you do not have to manage versions by hand.

## TLS backend

eDAPtor uses the [rustls](https://github.com/rustls/rustls) TLS backend, so
**no OpenSSL is needed** to build or run it. This also means static,
self-contained `musl` release binaries can be produced without vendoring a TLS
library.

Pre-built binaries are published with each
[GitHub Release](https://github.com/oposs/edaptor/releases) for Linux
(`x86_64`/`aarch64`, static musl), macOS (Intel and Apple Silicon), Windows, and
**illumos** (`x86_64`, e.g. OmniOS). The illumos binary is dynamically linked —
illumos has no static libc — and links only against base system libraries.

## Configuration file

eDAPtor reads a single TOML configuration file. Point it at one explicitly with
`--config <path>`, or let it fall back to the default location
`~/.config/edaptor/config.toml`:

```bash
edaptor --config /path/to/config.toml
```

The config file declares your server connection, authentication, and the entry
profiles that describe what a "user" and a "group" mean in your directory. See
the [Configuration](../configuration/overview.md) section for the full
reference.
