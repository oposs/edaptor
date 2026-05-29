# edaptor M1 — Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `edaptor` binary that loads a TOML config, connects + binds to an OpenLDAP server over TLS, fetches the raw subschema, and prints a summary — proving the config → background-worker → ldap3/TLS → fetch pipeline end-to-end.

**Architecture:** Layered with a background LDAP worker thread. M1 builds the worker/channel plumbing and the config layer, with the network confined to `ldap3` running on the worker thread. No schema *parsing* and no TUI yet — those are later milestones. See the design spec at `docs/superpowers/specs/2026-05-29-edaptor-design.md` and the verified API facts at `docs/superpowers/research/2026-05-29-api-spike-findings.md`.

**Tech Stack:** Rust 2021; `ldap3` 0.12 (sync, native-tls); `native-tls`; `serde` + `toml`; `clap`; `rpassword`; `anyhow`/`thiserror`. Integration tests run against OpenLDAP in a **podman** container.

---

## Milestone Roadmap

This plan covers **M1 only**. Later milestones get their own plans, written after the preceding milestone lands (each reshapes the next). Each milestone produces working, testable software.

- **M1 — Foundation (this plan):** config + password resolution + TLS settings + background worker + bind + raw subschema fetch + CLI summary. *Deliverable:* `edaptor --config x` prints connection + schema counts.
- **M2 — Schema model & introspection:** parse the raw subschema with `ldap-types` (chumsky) into a typed model; resolve MUST/MAY across SUP inheritance; map attribute syntaxes → widget kinds. Headless; golden-file tests vs the live schema. *Deliverable:* a CLI subcommand dumps the resolved field set for a given objectClass.
- **M3 — TUI shell + generic object tier:** turbo-vision app shell (menu/status) behind `ui/facade`; `OutlineViewer` DIT browser with labels + incremental search; generic schema-driven form (FieldSpec); `ChangeSet` diff; LDIF preview; immediate apply. *Deliverable:* browse/edit any entry in the TUI.
- **M4 — Users & groups tier:** config profiles; Users/Groups lists; rich create/view/edit/delete; symmetric membership dual-pane (`FilteredList` custom view); memberOf-aware; last-member rule; MODRDN rename. *Deliverable:* the headline workflows.
- **M5 — Samba lifecycle:** `md4` NT-hash; `sambaDomain` SID discovery; SID/RID; acct flags; group mapping; synced password; Samba-enable. *Deliverable:* full Samba account management.
- **M6 — OU management, scale & polish:** OU create/rename/delete; paged-results lists at scale; LDAP-result-code → human message table; SASL EXTERNAL then GSSAPI auth (feature-gated); packaging. *Deliverable:* v1.

---

## M1 File Structure

```
edaptor/
├── Cargo.toml                 # deps
├── src/
│   ├── main.rs                # clap CLI; loads config, resolves pw, runs check, prints summary
│   ├── lib.rs                 # module exports + run_check() + CheckSummary
│   ├── config/
│   │   ├── mod.rs             # Config, ServerConfig, TlsConfig, AuthConfig, AuthMethod, Config::load
│   │   └── password.rs        # PasswordSource enum + parse + resolve
│   └── ldap/
│       ├── mod.rs             # `pub mod tls; pub mod worker;`
│       ├── tls.rs             # build_settings(&ServerConfig) -> LdapConnSettings
│       └── worker.rs          # Request/Response, RawSubschema, WorkerHandle (spawn/request/Drop)
├── scripts/
│   └── test-ldap.sh           # start/stop a podman OpenLDAP for integration tests
└── tests/
    └── integration.rs         # end-to-end test against the container (skipped if env unset)
```

**Responsibilities:** `config` owns parsing/validation and password resolution (no LDAP). `ldap::tls` turns config into `LdapConnSettings`. `ldap::worker` owns the thread + the only `ldap3` calls. `lib::run_check` is the headless entry the CLI and integration test both drive. `main` is thin glue.

---

## Task 1: Project scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`, `src/lib.rs`

- [ ] **Step 1: Initialize the crate**

Run:
```bash
cargo init --name edaptor --vcs none
```
Expected: creates `Cargo.toml` and `src/main.rs`.

- [ ] **Step 2: Write `Cargo.toml`**

Replace `Cargo.toml` with (use `cargo add` afterward if you prefer to pin the exact latest patch releases — these caret ranges resolve to the latest compatible):

```toml
[package]
name = "edaptor"
version = "0.1.0"
edition = "2021"
description = "TUI for editing OpenLDAP directories (users, groups, memberships)"

[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
toml = "1"
anyhow = "1"
thiserror = "2"
ldap3 = "0.12"            # default features: sync + tls (native-tls)
native-tls = "0.2"       # build a custom TlsConnector for a configured CA
rpassword = "7"          # read the bind password without echo

[dev-dependencies]
tempfile = "3"

[[bin]]
name = "edaptor"
path = "src/main.rs"

[lib]
name = "edaptor"
path = "src/lib.rs"
```

- [ ] **Step 3: Write a minimal `src/lib.rs`**

```rust
//! edaptor — a schema-driven OpenLDAP TUI. M1 exposes a headless check pipeline.

pub mod config;
pub mod ldap;
```

(`config` and `ldap` modules are created in later tasks; this will not compile until Task 2/5. That is expected — Step 4 only confirms the toolchain resolves deps.)

- [ ] **Step 4: Make `src/lib.rs` temporarily empty to verify the build**

Temporarily set `src/lib.rs` to just a doc comment and `src/main.rs` to:

```rust
fn main() {
    println!("edaptor scaffold");
}
```

Run: `cargo build`
Expected: PASS — downloads and compiles all dependencies.

- [ ] **Step 5: Restore `src/lib.rs` module declarations**

Put back the `pub mod config; pub mod ldap;` from Step 3. (Build will fail until Task 2 + Task 5 create those modules — that is fine; do not run build again until then.)

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/
git commit -m "M1: scaffold edaptor crate with dependencies"
```

---

## Task 2: Config types and loading

**Files:**
- Create: `src/config/mod.rs`
- Test: inline `#[cfg(test)]` module in `src/config/mod.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/config/mod.rs` with the test module first (types come in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let toml = r#"
            [server]
            uri = "ldaps://ldap.example.com:636"
            base_dn = "dc=example,dc=com"

            [auth]
            method = "simple"
            bind_dn = "cn=ldapmanager,dc=example,dc=com"
            password_source = "prompt"
        "#;
        let cfg: Config = toml::from_str(toml).expect("should parse");
        assert_eq!(cfg.server.uri, "ldaps://ldap.example.com:636");
        assert_eq!(cfg.server.base_dn, "dc=example,dc=com");
        assert_eq!(cfg.server.timeout_secs, 10); // default
        assert!(cfg.server.tls.verify); // default true
        assert_eq!(cfg.auth.method, AuthMethod::Simple);
        assert_eq!(cfg.auth.bind_dn.as_deref(), Some("cn=ldapmanager,dc=example,dc=com"));
    }

    #[test]
    fn tls_defaults_to_verify_true_when_table_absent() {
        let toml = r#"
            [server]
            uri = "ldap://ldap.example.com:389"
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"
        "#;
        let cfg: Config = toml::from_str(toml).expect("should parse");
        assert!(cfg.server.tls.verify);
        assert!(!cfg.server.start_tls); // default false
        assert_eq!(cfg.auth.method, AuthMethod::Simple); // default
    }

    #[test]
    fn missing_uri_is_an_error() {
        let toml = r#"
            [server]
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"
        "#;
        assert!(toml::from_str::<Config>(toml).is_err());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib config`
Expected: FAIL — `Config`, `AuthMethod` not defined / module won't compile.

- [ ] **Step 3: Write the config types and loader (above the test module)**

Prepend to `src/config/mod.rs`:

```rust
//! Configuration: connection properties + auth. (Entry profiles arrive in M4.)

pub mod password;
pub use password::PasswordSource;

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub uri: String,
    pub base_dn: String,
    #[serde(default)]
    pub start_tls: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub tls: TlsConfig,
}

fn default_timeout() -> u64 {
    10
}

#[derive(Debug, Deserialize)]
pub struct TlsConfig {
    #[serde(default)]
    pub ca_cert: Option<PathBuf>,
    #[serde(default)]
    pub client_cert: Option<PathBuf>,
    #[serde(default)]
    pub client_key: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub verify: bool,
}

fn default_true() -> bool {
    true
}

// Manual Default so an absent [server.tls] table yields verify = true.
impl Default for TlsConfig {
    fn default() -> Self {
        TlsConfig { ca_cert: None, client_cert: None, client_key: None, verify: true }
    }
}

#[derive(Debug, Deserialize)]
pub struct AuthConfig {
    #[serde(default)]
    pub method: AuthMethod,
    #[serde(default)]
    pub bind_dn: Option<String>,
    #[serde(default)]
    pub password_source: PasswordSource,
}

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    #[default]
    Simple,
    External,
    Gssapi,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let config: Config = toml::from_str(&text)
            .with_context(|| format!("parsing config {}", path.display()))?;
        Ok(config)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib config`
Expected: PASS (3 tests). The `password` submodule does not exist yet, so the crate won't compile — proceed to Task 3 which creates it, then re-run. (If you want this task green in isolation, temporarily comment out `pub mod password; pub use password::PasswordSource;` and the `password_source` field; but it is simpler to do Task 3 next and run both together.)

- [ ] **Step 5: Commit (after Task 3 compiles)**

Defer the commit to the end of Task 3, since `config/mod.rs` references `password`.

---

## Task 3: Password source resolution

**Files:**
- Create: `src/config/password.rs`
- Test: inline `#[cfg(test)]` module in `src/config/password.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/config/password.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_variant() {
        assert_eq!("prompt".parse::<PasswordSource>().unwrap(), PasswordSource::Prompt);
        assert_eq!(
            "env:EDAPTOR_PW".parse::<PasswordSource>().unwrap(),
            PasswordSource::Env("EDAPTOR_PW".to_string())
        );
        assert_eq!(
            "command:pass ldap/mgr".parse::<PasswordSource>().unwrap(),
            PasswordSource::Command("pass ldap/mgr".to_string())
        );
    }

    #[test]
    fn rejects_unknown_and_empty() {
        assert!("nonsense".parse::<PasswordSource>().is_err());
        assert!("env:".parse::<PasswordSource>().is_err());
        assert!("command:   ".parse::<PasswordSource>().is_err());
    }

    #[test]
    fn resolves_from_env() {
        // Use a unique var name to avoid cross-test interference.
        std::env::set_var("EDAPTOR_TEST_PW_VAR", "s3cret");
        let src = PasswordSource::Env("EDAPTOR_TEST_PW_VAR".to_string());
        assert_eq!(src.resolve().unwrap(), "s3cret");
    }

    #[test]
    fn resolves_from_command_and_trims_newline() {
        let src = PasswordSource::Command("printf 'hunter2\\n'".to_string());
        assert_eq!(src.resolve().unwrap(), "hunter2");
    }

    #[test]
    fn failing_command_errors() {
        let src = PasswordSource::Command("exit 3".to_string());
        assert!(src.resolve().is_err());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib password`
Expected: FAIL — `PasswordSource` not defined.

- [ ] **Step 3: Write the implementation (above the test module)**

Prepend to `src/config/password.rs`:

```rust
//! How the bind password is obtained. Never stored in the config file.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Deserializer};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordSource {
    /// Prompt the operator interactively (no echo).
    Prompt,
    /// Read from the named environment variable.
    Env(String),
    /// Run a shell command; its stdout (trailing newline trimmed) is the password.
    Command(String),
}

impl Default for PasswordSource {
    fn default() -> Self {
        PasswordSource::Prompt
    }
}

impl FromStr for PasswordSource {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        if s == "prompt" {
            Ok(PasswordSource::Prompt)
        } else if let Some(var) = s.strip_prefix("env:") {
            if var.is_empty() {
                return Err(anyhow!("password_source 'env:' needs a variable name"));
            }
            Ok(PasswordSource::Env(var.to_string()))
        } else if let Some(cmd) = s.strip_prefix("command:") {
            if cmd.trim().is_empty() {
                return Err(anyhow!("password_source 'command:' needs a command"));
            }
            Ok(PasswordSource::Command(cmd.to_string()))
        } else {
            Err(anyhow!(
                "invalid password_source '{s}': expected 'prompt', 'env:VAR', or 'command:...'"
            ))
        }
    }
}

impl<'de> Deserialize<'de> for PasswordSource {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl PasswordSource {
    pub fn resolve(&self) -> Result<String> {
        match self {
            PasswordSource::Prompt => {
                rpassword::prompt_password("LDAP bind password: ")
                    .context("reading password from prompt")
            }
            PasswordSource::Env(var) => {
                std::env::var(var).with_context(|| format!("environment variable '{var}' is not set"))
            }
            PasswordSource::Command(cmd) => run_password_command(cmd),
        }
    }
}

fn run_password_command(cmd: &str) -> Result<String> {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("running password command '{cmd}'"))?;
    if !output.status.success() {
        return Err(anyhow!("password command '{cmd}' exited with status {}", output.status));
    }
    let pw = String::from_utf8(output.stdout)
        .context("password command output was not valid UTF-8")?;
    Ok(pw.trim_end_matches(['\n', '\r']).to_string())
}
```

- [ ] **Step 4: Run the config + password tests together**

Run: `cargo test --lib`
Expected: PASS — all Task 2 and Task 3 tests (8 total).

- [ ] **Step 5: Commit**

```bash
git add src/config/ src/lib.rs
git commit -m "M1: config types, loading, and password source resolution"
```

---

## Task 4: TLS settings builder

**Files:**
- Create: `src/ldap/mod.rs`
- Create: `src/ldap/tls.rs`
- Test: inline `#[cfg(test)]` module in `src/ldap/tls.rs`

- [ ] **Step 1: Create the ldap module file**

Create `src/ldap/mod.rs`:

```rust
//! LDAP layer: TLS settings and the background worker thread (the only code that
//! touches the network).

pub mod tls;
pub mod worker;
```

(`worker` is created in Task 5; the crate will not compile until then. That is expected — Task 4 tests are run after Task 5 in Step 4, or you may temporarily comment out `pub mod worker;` to run Task 4 in isolation.)

- [ ] **Step 2: Write the failing tests**

Create `src/ldap/tls.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServerConfig, TlsConfig};
    use std::io::Write;

    fn server_with_tls(tls: TlsConfig, start_tls: bool) -> ServerConfig {
        ServerConfig {
            uri: "ldaps://ldap.example.com:636".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            start_tls,
            timeout_secs: 10,
            tls,
        }
    }

    #[test]
    fn builds_settings_with_no_custom_ca() {
        let server = server_with_tls(TlsConfig::default(), false);
        assert!(build_settings(&server).is_ok());
    }

    #[test]
    fn builds_settings_with_starttls_and_no_verify() {
        let mut tls = TlsConfig::default();
        tls.verify = false;
        let server = server_with_tls(tls, true);
        assert!(build_settings(&server).is_ok());
    }

    #[test]
    fn missing_ca_file_is_an_error() {
        let mut tls = TlsConfig::default();
        tls.ca_cert = Some("/no/such/ca.pem".into());
        let server = server_with_tls(tls, false);
        let err = build_settings(&server).unwrap_err();
        assert!(err.to_string().contains("reading CA cert"), "got: {err}");
    }

    #[test]
    fn garbage_ca_file_is_a_parse_error() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"this is not a certificate").unwrap();
        let mut tls = TlsConfig::default();
        tls.ca_cert = Some(f.path().to_path_buf());
        let server = server_with_tls(tls, false);
        let err = build_settings(&server).unwrap_err();
        assert!(err.to_string().contains("parsing CA cert"), "got: {err}");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib tls`
Expected: FAIL — `build_settings` not defined.

- [ ] **Step 4: Write the implementation (above the test module)**

Prepend to `src/ldap/tls.rs`:

```rust
//! Turn a ServerConfig into ldap3 LdapConnSettings (native-tls backend).
//!
//! M1 wires the configured CA and the verify flag. Client-certificate identity
//! (for SASL EXTERNAL) is added in the auth milestone (M6).

use anyhow::{Context, Result};
use ldap3::LdapConnSettings;
use native_tls::{Certificate, TlsConnector};

use crate::config::ServerConfig;

pub fn build_settings(server: &ServerConfig) -> Result<LdapConnSettings> {
    let mut settings = LdapConnSettings::new();

    // StartTLS upgrades an ldap:// connection (do NOT combine with ldaps://).
    if server.start_tls {
        settings = settings.set_starttls(true);
    }

    // Trust a custom CA if configured.
    if let Some(ca_path) = &server.tls.ca_cert {
        let pem = std::fs::read(ca_path)
            .with_context(|| format!("reading CA cert {}", ca_path.display()))?;
        let ca = Certificate::from_pem(&pem)
            .with_context(|| format!("parsing CA cert {}", ca_path.display()))?;
        let connector = TlsConnector::builder()
            .add_root_certificate(ca)
            .build()
            .context("building TLS connector")?;
        settings = settings.set_connector(connector);
    }

    // Disable verification only when explicitly configured (testing).
    if !server.tls.verify {
        settings = settings.set_no_tls_verify(true);
    }

    Ok(settings)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib tls` (after Task 5 exists, or with `pub mod worker;` temporarily commented out)
Expected: PASS (4 tests).

- [ ] **Step 6: Commit (after Task 5 compiles)**

Defer to the end of Task 5, since `ldap/mod.rs` references `worker`.

---

## Task 5: Worker thread, messages, and run_check

**Files:**
- Create: `src/ldap/worker.rs`
- Modify: `src/lib.rs` (add `run_check` + `CheckSummary`)

- [ ] **Step 1: Write the worker message types and handle**

Create `src/ldap/worker.rs`:

```rust
//! The background LDAP worker thread. It owns the ldap3 connection and is the
//! only place network I/O happens. Callers send a Request and block for a
//! Response over a per-request reply channel.

use anyhow::{anyhow, Context, Result};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use ldap3::{LdapConn, Scope, SearchEntry};

use crate::config::{AuthMethod, Config};
use crate::ldap::tls::build_settings;

/// A request to the worker. Each is paired with a reply Sender in the channel.
pub enum Request {
    /// Fetch the raw (unparsed) subschema description strings.
    FetchSubschema,
    /// Unbind and stop the worker thread.
    Shutdown,
}

/// Raw subschema: the server's description strings, not yet parsed (that is M2).
#[derive(Debug, Clone, Default)]
pub struct RawSubschema {
    pub object_classes: Vec<String>,
    pub attribute_types: Vec<String>,
    pub ldap_syntaxes: Vec<String>,
}

pub enum Response {
    Subschema(RawSubschema),
    Done,
    Error(String),
}

type Job = (Request, Sender<Response>);

pub struct WorkerHandle {
    tx: Sender<Job>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    /// Spawn the worker, connecting + binding synchronously so connection or
    /// credential failures surface immediately as an Err from spawn().
    pub fn spawn(config: Config, password: String) -> Result<WorkerHandle> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (startup_tx, startup_rx) = mpsc::channel::<std::result::Result<(), String>>();

        let join = thread::spawn(move || {
            let mut conn = match connect_and_bind(&config, &password) {
                Ok(conn) => {
                    let _ = startup_tx.send(Ok(()));
                    conn
                }
                Err(e) => {
                    let _ = startup_tx.send(Err(e.to_string()));
                    return;
                }
            };
            worker_loop(&mut conn, &config, rx);
        });

        match startup_rx.recv() {
            Ok(Ok(())) => Ok(WorkerHandle { tx, join: Some(join) }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(anyhow!(e))
            }
            Err(_) => {
                let _ = join.join();
                Err(anyhow!("worker thread exited before reporting startup status"))
            }
        }
    }

    /// Send a request and block for its response.
    pub fn request(&self, req: Request) -> Result<Response> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send((req, reply_tx))
            .map_err(|_| anyhow!("worker thread is gone"))?;
        reply_rx
            .recv()
            .map_err(|_| anyhow!("worker dropped the reply channel"))
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        let (reply_tx, _reply_rx) = mpsc::channel();
        let _ = self.tx.send((Request::Shutdown, reply_tx));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn connect_and_bind(config: &Config, password: &str) -> Result<LdapConn> {
    let settings = build_settings(&config.server)?;
    let mut conn = LdapConn::with_settings(settings, &config.server.uri)
        .with_context(|| format!("connecting to {}", config.server.uri))?;

    match config.auth.method {
        AuthMethod::Simple => {
            let bind_dn = config
                .auth
                .bind_dn
                .as_deref()
                .ok_or_else(|| anyhow!("auth.method = simple requires auth.bind_dn"))?;
            conn.simple_bind(bind_dn, password)
                .context("sending simple bind")?
                .success()
                .context("LDAP rejected the bind credentials")?;
        }
        AuthMethod::External => {
            return Err(anyhow!("auth.method = external is not implemented until M6"));
        }
        AuthMethod::Gssapi => {
            return Err(anyhow!("auth.method = gssapi is not implemented until M6"));
        }
    }
    Ok(conn)
}

fn worker_loop(conn: &mut LdapConn, config: &Config, rx: Receiver<Job>) {
    while let Ok((req, reply)) = rx.recv() {
        match req {
            Request::FetchSubschema => {
                let resp = match fetch_subschema(conn, &config.server.base_dn) {
                    Ok(raw) => Response::Subschema(raw),
                    Err(e) => Response::Error(e.to_string()),
                };
                let _ = reply.send(resp);
            }
            Request::Shutdown => {
                let _ = conn.unbind();
                let _ = reply.send(Response::Done);
                break;
            }
        }
    }
}

fn fetch_subschema(conn: &mut LdapConn, base_dn: &str) -> Result<RawSubschema> {
    // 1. Find the subschema subentry DN (operational attribute on the base entry).
    let (entries, _res) = conn
        .search(base_dn, Scope::Base, "(objectClass=*)", vec!["subschemaSubentry"])?
        .success()
        .context("reading subschemaSubentry")?;
    let subschema_dn = entries
        .into_iter()
        .map(SearchEntry::construct)
        .find_map(|e| e.attrs.get("subschemaSubentry").and_then(|v| v.first().cloned()))
        .ok_or_else(|| anyhow!("server did not expose subschemaSubentry on {base_dn}"))?;

    // 2. Read the schema definition strings from that entry.
    let (entries, _res) = conn
        .search(
            &subschema_dn,
            Scope::Base,
            "(objectClass=subschema)",
            vec!["objectClasses", "attributeTypes", "ldapSyntaxes"],
        )?
        .success()
        .context("reading subschema definitions")?;
    let entry = entries
        .into_iter()
        .map(SearchEntry::construct)
        .next()
        .ok_or_else(|| anyhow!("subschema entry {subschema_dn} not found"))?;

    Ok(RawSubschema {
        object_classes: entry.attrs.get("objectClasses").cloned().unwrap_or_default(),
        attribute_types: entry.attrs.get("attributeTypes").cloned().unwrap_or_default(),
        ldap_syntaxes: entry.attrs.get("ldapSyntaxes").cloned().unwrap_or_default(),
    })
}
```

- [ ] **Step 2: Add `run_check` + `CheckSummary` to `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! edaptor — a schema-driven OpenLDAP TUI. M1 exposes a headless check pipeline.

pub mod config;
pub mod ldap;

use anyhow::{anyhow, Result};

use crate::config::Config;
use crate::ldap::worker::{Request, Response, WorkerHandle};

/// Result of the M1 connectivity + schema-fetch check.
pub struct CheckSummary {
    pub uri: String,
    pub bind_dn: Option<String>,
    pub object_class_count: usize,
    pub attribute_type_count: usize,
    pub ldap_syntax_count: usize,
}

/// Connect, bind, fetch the raw subschema, and summarize. Drives both the CLI
/// and the integration test. The worker is shut down cleanly when `handle` drops.
pub fn run_check(config: Config, password: String) -> Result<CheckSummary> {
    let uri = config.server.uri.clone();
    let bind_dn = config.auth.bind_dn.clone();

    let handle = WorkerHandle::spawn(config, password)?;
    match handle.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => Ok(CheckSummary {
            uri,
            bind_dn,
            object_class_count: raw.object_classes.len(),
            attribute_type_count: raw.attribute_types.len(),
            ldap_syntax_count: raw.ldap_syntaxes.len(),
        }),
        Response::Error(e) => Err(anyhow!(e)),
        Response::Done => Err(anyhow!("unexpected Done response to FetchSubschema")),
    }
}
```

- [ ] **Step 3: Build and run all unit tests**

Run: `cargo test --lib`
Expected: PASS — all config/password/tls tests compile and pass (the worker has no unit tests; it is exercised by the integration test in Task 6).

Run: `cargo build`
Expected: PASS — the whole crate compiles.

- [ ] **Step 4: Commit**

```bash
git add src/
git commit -m "M1: background LDAP worker, subschema fetch, and run_check"
```

---

## Task 6: Integration test against a podman OpenLDAP

**Files:**
- Create: `scripts/test-ldap.sh`
- Create: `tests/integration.rs`

- [ ] **Step 1: Write the container helper script**

Create `scripts/test-ldap.sh`:

```bash
#!/usr/bin/env bash
# Start/stop a throwaway OpenLDAP server for edaptor integration tests (podman).
# Usage: scripts/test-ldap.sh [start|stop]
set -euo pipefail

NAME=edaptor-test-ldap
IMAGE=docker.io/bitnami/openldap:2.6

case "${1:-start}" in
  start)
    podman run -d --rm --name "$NAME" \
      -p 1389:1389 \
      -e LDAP_ROOT="dc=example,dc=org" \
      -e LDAP_ADMIN_USERNAME="admin" \
      -e LDAP_ADMIN_PASSWORD="adminpassword" \
      "$IMAGE" >/dev/null
    echo "Waiting for LDAP to accept connections..."
    for _ in $(seq 1 30); do
      if podman exec "$NAME" ldapsearch -x -H ldap://localhost:1389 \
           -b "dc=example,dc=org" -s base >/dev/null 2>&1; then
        echo "Ready."
        echo "  export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389"
        echo "  export EDAPTOR_TEST_ADMIN_PW=adminpassword"
        exit 0
      fi
      sleep 1
    done
    echo "ERROR: LDAP did not become ready in time" >&2
    exit 1
    ;;
  stop)
    podman stop "$NAME" >/dev/null 2>&1 || true
    echo "Stopped $NAME"
    ;;
  *)
    echo "usage: $0 [start|stop]" >&2
    exit 1
    ;;
esac
```

Then: `chmod +x scripts/test-ldap.sh`

- [ ] **Step 2: Write the integration test**

Create `tests/integration.rs`:

```rust
//! End-to-end test against a live OpenLDAP server.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes (no silent skip).

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::run_check;

fn test_config(uri: String) -> (Config, String) {
    let config = Config {
        server: ServerConfig {
            uri,
            base_dn: "dc=example,dc=org".to_string(),
            start_tls: false,
            timeout_secs: 10,
            tls: TlsConfig::default(),
        },
        auth: AuthConfig {
            method: AuthMethod::Simple,
            bind_dn: Some("cn=admin,dc=example,dc=org".to_string()),
            password_source: PasswordSource::Env("EDAPTOR_TEST_ADMIN_PW".to_string()),
        },
    };
    let password =
        std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".to_string());
    (config, password)
}

#[test]
fn connects_binds_and_fetches_subschema() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP connects_binds_and_fetches_subschema: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };

    let (config, password) = test_config(uri);
    let summary = run_check(config, password).expect("run_check should succeed against the server");

    assert!(summary.object_class_count > 0, "expected objectClasses in subschema");
    assert!(summary.attribute_type_count > 0, "expected attributeTypes in subschema");
    assert!(summary.ldap_syntax_count > 0, "expected ldapSyntaxes in subschema");
}

#[test]
fn wrong_password_is_rejected() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP wrong_password_is_rejected: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };

    let (config, _password) = test_config(uri);
    let err = run_check(config, "definitely-wrong".to_string()).unwrap_err();
    assert!(
        err.to_string().contains("rejected the bind credentials")
            || err.to_string().to_lowercase().contains("invalid"),
        "expected a bind rejection, got: {err}"
    );
}
```

- [ ] **Step 3: Start the server and run the test to verify it passes**

Run:
```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo test --test integration -- --nocapture
```
Expected: PASS (2 tests) — the subschema fetch returns non-zero counts and the wrong password is rejected.

- [ ] **Step 4: Confirm the skip path**

Run:
```bash
unset EDAPTOR_TEST_LDAP_URI
cargo test --test integration -- --nocapture
```
Expected: PASS, printing `SKIP ...` for both tests.

- [ ] **Step 5: Stop the server**

Run: `scripts/test-ldap.sh stop`

- [ ] **Step 6: Commit**

```bash
git add scripts/test-ldap.sh tests/integration.rs
git commit -m "M1: podman OpenLDAP integration test for connect/bind/subschema"
```

> **Note on TLS coverage:** the integration test binds over plaintext `ldap://` to keep the harness simple; it proves the worker/channel/bind/fetch pipeline. The TLS settings builder is unit-tested (Task 4) but full end-to-end LDAPS is *not* exercised in M1 — it is deferred to a later milestone's harness (with seeded certs). This limitation is intentional and called out so coverage is not overstated.

---

## Task 7: CLI wiring in main

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Write the CLI**

Replace `src/main.rs`:

```rust
//! edaptor CLI. M1: connect, bind, print a schema summary, then exit.
//! (The TUI replaces this default action in M3.)

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use edaptor::config::Config;

#[derive(Parser)]
#[command(name = "edaptor", about = "TUI for editing OpenLDAP directories")]
struct Cli {
    /// Path to the configuration file
    /// (default: $XDG_CONFIG_HOME/edaptor/config.toml or ~/.config/edaptor/config.toml).
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

fn default_config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("edaptor/config.toml");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/edaptor/config.toml")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(default_config_path);

    let config = Config::load(&config_path)?;
    let password = config
        .auth
        .password_source
        .resolve()
        .context("resolving bind password")?;

    let summary = edaptor::run_check(config, password)?;

    println!("Connected to {}", summary.uri);
    if let Some(dn) = &summary.bind_dn {
        println!("Bound as {dn}");
    }
    println!(
        "Subschema: {} objectClasses, {} attributeTypes, {} ldapSyntaxes",
        summary.object_class_count, summary.attribute_type_count, summary.ldap_syntax_count
    );
    Ok(())
}
```

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: PASS.

- [ ] **Step 3: Manual end-to-end verification**

Run:
```bash
scripts/test-ldap.sh start
cat > /tmp/edaptor-test.toml <<'EOF'
[server]
uri = "ldap://localhost:1389"
base_dn = "dc=example,dc=org"

[auth]
method = "simple"
bind_dn = "cn=admin,dc=example,dc=org"
password_source = "env:EDAPTOR_PW"
EOF
EDAPTOR_PW=adminpassword cargo run -- --config /tmp/edaptor-test.toml
scripts/test-ldap.sh stop
```
Expected output (counts vary by server):
```
Connected to ldap://localhost:1389
Bound as cn=admin,dc=example,dc=org
Subschema: NN objectClasses, MMM attributeTypes, KK ldapSyntaxes
```

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "M1: CLI entry point (--config) printing the connection summary"
```

---

## M1 Definition of Done

- [ ] `cargo test --lib` passes (config, password, tls unit tests).
- [ ] `cargo test --test integration` passes with the container running, and prints SKIP (still passing) without it.
- [ ] `cargo run -- --config <file>` connects, binds over the configured transport, and prints non-zero subschema counts.
- [ ] `cargo clippy --all-targets` is clean (run it; fix warnings).
- [ ] All seven tasks committed.
