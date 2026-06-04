# rustls TLS Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `native-tls`/OpenSSL TLS backend with rustls so static musl release builds need no OpenSSL and no vendoring, preserving the exact current TLS semantics (custom CA, `verify=false`, StartTLS, connect timeout).

**Architecture:** `src/ldap/tls.rs::build_settings` turns a `ServerConfig` into an `ldap3::LdapConnSettings`. We swap the ldap3 TLS feature from `tls-native` to `tls-rustls-ring`, drop `native-tls`, add `rustls` + `rustls-pemfile`, and rewrite `build_settings` to build a `rustls::ClientConfig` for the custom-CA case via `set_config`, while the `verify=false` case uses ldap3's built-in `set_no_tls_verify`. This is a backend swap with identical externally-visible behaviour, verified by the existing unit tests (one assertion updated) plus `cargo tree` checks proving OpenSSL is gone.

**Tech Stack:** Rust, `ldap3 0.12` (`tls-rustls-ring`), `rustls 0.23`, `rustls-pemfile 2`, `anyhow`.

---

## Background facts (verified against the registry before writing this plan)

These were confirmed by reading `~/.cargo/registry/.../ldap3-0.12.1/` — do not re-litigate them, but they explain the code below:

- ldap3 0.12.1 pins `rustls = "0.23.31"`, so our direct `rustls` dep unifies to the same crate. ldap3's `tls-rustls-ring` feature turns on `rustls/ring`, which provides the process-default crypto provider — so `ClientConfig::builder()` works with **no** manual `CryptoProvider::install_default()` call (ldap3's own `create_config` relies on exactly this).
  - **CRITICAL — crypto-provider collision:** `rustls 0.23`'s *default* features include the `aws-lc-rs` provider. ldap3 pulls rustls with `default-features = false` + `ring`. Because Cargo **unions** features across the single shared rustls instance, a plain `rustls = "0.23"` (defaults on) would compile in **both** `aws-lc-rs` *and* `ring`. With two providers present, `ClientConfig::builder()` cannot auto-select one and **panics at runtime** ("Could not automatically determine the process-level CryptoProvider") — or fails to build if aws-lc-rs's cmake/clang toolchain is absent. The fix is to take our rustls dep with **`default-features = false`**: `std`/`tls12`/`ring` all still arrive via ldap3's feature set through unification, so a single provider (`ring`) is active and `builder()` resolves. This is why Task 1 uses `rustls = { version = "0.23", default-features = false }`, **not** `rustls = "0.23"`.
  - **None of the original four unit tests reach `ClientConfig::builder()`** (no-CA and verify=false skip the CA branch; missing-CA bails at `fs::read`; garbage-CA bails at the empty-cert check). So a provider collision would pass `cargo build`/`test`/`clippy`/`tree` silently. Task 1 therefore adds a **valid-CA test** that drives the full `rustls_pemfile::certs → RootCertStore::add → ClientConfig::builder → set_config` path, converting the latent panic into a `cargo test` failure.
- `LdapConnSettings` exposes `set_conn_timeout(Duration)`, `set_starttls(bool)`, `set_no_tls_verify(bool)`, and `set_config(Arc<rustls::ClientConfig>)` on the rustls feature path (`src/conn.rs:251-302`).
- **Edge case (confirmed in `src/conn.rs:574-612`):** `create_tls_stream` uses a caller-supplied `settings.config` *verbatim* and only builds its own config (the one that honours `set_no_tls_verify`) when `settings.config` is `None`. So a custom-CA `ClientConfig` set via `set_config` does **not** automatically get the no-cert verifier.
  - **Design decision for this plan:** rather than hand-implement a `rustls::client::danger::ServerCertVerifier` (~40 lines of `unsafe`-adjacent trait impl) for the degenerate "custom CA *and* `verify=false`" combination, we treat `verify=false` as authoritative: when verification is off we call `set_no_tls_verify(true)` and ignore any configured CA (the trust anchor is irrelevant once you accept every certificate). This is **behaviourally identical** to the outgoing native-tls path, where `danger_accept_invalid_certs(true)` already made the added root certificate moot. The custom-CA `ClientConfig` is built **only** when `verify=true`.
- `rustls-pemfile 2`'s `certs(&mut reader)` returns an iterator of `Result<CertificateDer<'static>, io::Error>`. It is **lenient**: non-PEM input (e.g. "this is not a certificate") yields **zero** certs rather than an error. So the old "garbage CA → parse error" test must change to "garbage CA → no certificates found" (one assertion update, anticipated by the design spec).
- `native_tls` is imported **only** in `src/ldap/tls.rs` (verified by grep) — no other module is affected.
- `TlsConfig::default()` has `verify: true` (`src/config/mod.rs:185-193`), so the two CA-file tests still take the CA-reading branch under the new control flow.

---

## Task 1: Swap the TLS backend in Cargo.toml and rewrite `build_settings`

This is a single atomic change: the crate will not compile with the new dependencies until `tls.rs` is rewritten, and it will not compile with the new `tls.rs` until the dependencies change. We therefore edit `Cargo.toml`, `tls.rs`, and the one affected test together, then verify green in one commit. The existing unit tests are our behaviour spec.

**Files:**
- Modify: `Cargo.toml:12-14` (dependency swap)
- Modify: `src/ldap/tls.rs` (rewrite `build_settings` + module doc + one test assertion)

- [ ] **Step 1: Update the affected test assertion first (red-anchor)**

Open `src/ldap/tls.rs`. Replace the `garbage_ca_file_is_a_parse_error` test (lines 91-102) with the version below. rustls-pemfile treats non-PEM bytes as "no certificates" rather than a parse error, so the function now returns the empty-CA error; the test is renamed and its assertion updated to match the new, documented behaviour:

```rust
    #[test]
    fn garbage_ca_file_yields_no_certificates() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"this is not a certificate").unwrap();
        let tls = TlsConfig {
            ca_cert: Some(f.path().to_path_buf()),
            ..TlsConfig::default()
        };
        let server = server_with_tls(tls, false);
        let err = build_settings(&server).err().unwrap();
        assert!(
            err.to_string().contains("no certificates found"),
            "got: {err}"
        );
    }
```

Then **add** the following to the same `#[cfg(test)] mod tests` block (place the `VALID_CA_PEM` const just below the `use` lines at the top of the module, and the test alongside the others). This test is the one that actually exercises `ClientConfig::builder()` + `set_config` — the four pre-existing tests all return before reaching that code, so without this test a crypto-provider misconfiguration would pass CI silently. The PEM is a hermetic, offline-generated self-signed cert (valid 100 years, no network, no new dependency):

```rust
    // A self-signed CA, generated offline with
    //   openssl req -x509 -newkey rsa:2048 -nodes -days 36500 \
    //     -subj "/CN=edaptor-test-ca" -keyout /dev/null -out ca.pem
    // Used only to drive the custom-CA branch (parse -> RootCertStore ->
    // ClientConfig::builder -> set_config), which the other tests skip.
    const VALID_CA_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIDFzCCAf+gAwIBAgIUJr70ZihROr85j8WdByc0RI3obicwDQYJKoZIhvcNAQEL
BQAwGjEYMBYGA1UEAwwPZWRhcHRvci10ZXN0LWNhMCAXDTI2MDYwMzIzNDEwOFoY
DzIxMjYwNTEwMjM0MTA4WjAaMRgwFgYDVQQDDA9lZGFwdG9yLXRlc3QtY2EwggEi
MA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQCvvh8SCvaMEVahBRlwK0CfqMeS
RVJw8PkIuKvUWwjmigpli1y5lmq+pOahTTF20aCHkyq6+L2k1zAkQmqUW8hRWpLd
pCH8j1uNo8uFPZZhFrDTJ/aSQhF+ZTjZEFNrm5XVHbJCTL2MUJ/WoAPFL0rszy5i
8J2EyEpoRe+GiWqYQa7TOQ2jI4Q1OsSxdi7ut7kErNmxhUZLOmC2aQTu8fvjzSgS
e4pyAQnVLrtD4Fn0Nfu9tuMH+u7RXZF3dk5cIOEmIM9KqrAa0V7tsg2KTZxk4c1Q
Nsy8NXSdP6+p+Q8EzZ/aBfOlyQnAdUJTRng9J4BQU5gDk5qV4yUpvgGJxq+vAgMB
AAGjUzBRMB0GA1UdDgQWBBQP4cnSJiU8JtMOlvztpyZzsHRCjzAfBgNVHSMEGDAW
gBQP4cnSJiU8JtMOlvztpyZzsHRCjzAPBgNVHRMBAf8EBTADAQH/MA0GCSqGSIb3
DQEBCwUAA4IBAQBTZRUmC+q2rOoJziMBPzAIcf8yONESlAN2dzYNgJFwEF8xZOYk
dcCBSInwr1bHDVc+t5AXZU+H7Th45kdQIUvlc8UTm+1BIje9zb7/ydThyzZZEkax
40h6V1ihwFfvc8FH2gxbdkkcY2xt7QxWymJGF/UM3oHXTApvjpiOuXfWhyfeGkAo
75OVgwUQTmxthrJc5DJ6LcgCEQ+qE8bp3eqi0NEjQox7uw9vw3FKlmakVEAT1mry
Ql5m5Vy9xP0uzl2aVtUGO6B0FrstTlMUQ0yDKwXzx+5ZL8IxJTSd8Bo+5+78ooey
7CqIroe4B39d5saUMPTPVUEAgMn+Ez4qBUPX
-----END CERTIFICATE-----
";

    #[test]
    fn builds_settings_with_valid_custom_ca() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(VALID_CA_PEM.as_bytes()).unwrap();
        let tls = TlsConfig {
            ca_cert: Some(f.path().to_path_buf()),
            ..TlsConfig::default()
        };
        let server = server_with_tls(tls, false);
        // Drives the full custom-CA path including ClientConfig::builder().
        // Panics here ("could not determine the process-level CryptoProvider")
        // mean the rustls dep was added with default features (aws-lc-rs) on
        // top of ldap3's ring — fix the Cargo.toml dep, not the test.
        assert!(build_settings(&server).is_ok());
    }
```

> If `cargo test` panics on `builds_settings_with_valid_custom_ca` with a CryptoProvider message, the `default-features = false` in Step 2 was omitted. The test passing is the proof the provider is unambiguous.

- [ ] **Step 2: Swap the dependencies in `Cargo.toml`**

Replace lines 12-14 of `Cargo.toml`:

```toml
ldap3 = "0.12"            # default features: sync + tls (native-tls)

native-tls = "0.2"       # build a custom TlsConnector for a configured CA
```

with:

```toml
ldap3 = { version = "0.12", default-features = false, features = ["sync", "tls-rustls-ring"] }
# default-features = false is REQUIRED: it drops rustls's default aws-lc-rs
# provider, leaving only ldap3's `ring` (via feature unification). Two providers
# would panic ClientConfig::builder() at runtime. See the background note above.
rustls = { version = "0.23", default-features = false }  # custom ClientConfig for a configured CA
rustls-pemfile = "2"     # parse PEM CA certificates into CertificateDer
```

(The `md4` line that followed `native-tls` stays where it is — do not move it.)

- [ ] **Step 3: Rewrite `src/ldap/tls.rs` `build_settings` and the module doc**

Replace the file header (lines 1-45, i.e. the doc comment through the end of `build_settings`, **leaving the `#[cfg(test)] mod tests` block untouched except for Step 1's edit**) with:

```rust
//! Turn a ServerConfig into ldap3 LdapConnSettings (rustls backend).
//!
//! M1 wires the configured CA, the verify flag, and the connect timeout.
//! Client-certificate identity (for SASL EXTERNAL) is added in the auth
//! milestone (M6); per-operation timeouts are tracked for a later milestone.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use ldap3::LdapConnSettings;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};

use crate::config::ServerConfig;

pub fn build_settings(server: &ServerConfig) -> Result<LdapConnSettings> {
    // Bound the TCP connect so an unreachable/black-hole server cannot hang the
    // worker thread indefinitely. (Per-operation timeouts come in a later milestone.)
    let mut settings =
        LdapConnSettings::new().set_conn_timeout(Duration::from_secs(server.timeout_secs));

    // StartTLS upgrades an ldap:// connection (do NOT combine with ldaps://).
    if server.start_tls {
        settings = settings.set_starttls(true);
    }

    if !server.tls.verify {
        // Verification disabled (testing only): accept any certificate. This
        // subsumes any configured CA — once every certificate is accepted the
        // trust anchor is irrelevant — matching the previous native-tls
        // `danger_accept_invalid_certs(true)` behaviour. ldap3 installs its own
        // no-cert verifier on its default config when this flag is set.
        settings = settings.set_no_tls_verify(true);
    } else if let Some(ca_path) = &server.tls.ca_cert {
        // Trust a custom CA: parse the PEM, load it into a RootCertStore, and
        // hand ldap3 a ClientConfig built around it. ldap3 uses a caller-supplied
        // config verbatim, so this is the only branch that builds one.
        let pem = std::fs::read(ca_path)
            .with_context(|| format!("reading CA cert {}", ca_path.display()))?;
        let mut reader = std::io::BufReader::new(&pem[..]);
        let certs = rustls_pemfile::certs(&mut reader)
            .collect::<std::result::Result<Vec<CertificateDer<'static>>, _>>()
            .with_context(|| format!("parsing CA cert {}", ca_path.display()))?;
        if certs.is_empty() {
            anyhow::bail!("no certificates found in CA cert {}", ca_path.display());
        }
        let mut store = RootCertStore::empty();
        for cert in certs {
            store
                .add(cert)
                .with_context(|| format!("adding CA cert {}", ca_path.display()))?;
        }
        let config = ClientConfig::builder()
            .with_root_certificates(store)
            .with_no_client_auth();
        settings = settings.set_config(Arc::new(config));
    }

    Ok(settings)
}
```

Note: the `#[cfg(test)] mod tests` block below is unchanged apart from Step 1. In particular `builds_settings_with_no_custom_ca`, `builds_settings_with_starttls_and_no_verify`, and `missing_ca_file_is_an_error` port unchanged — under the new control flow the missing-CA test still has `verify=true` (default) and a `Some(ca_cert)`, so it reaches the `std::fs::read` and fails with "reading CA cert".

- [ ] **Step 4: Format**

Run: `cargo fmt`
Expected: exits 0, no diff complaints.

- [ ] **Step 5: Build the library**

Run: `cargo build --all-targets`
Expected: compiles with no errors. If you see "use of unresolved module or unlinked crate `rustls`", the `Cargo.toml` edit in Step 2 was not saved.

- [ ] **Step 6: Run the tls tests**

Run: `cargo test -p edaptor tls`
Expected: PASS — `builds_settings_with_no_custom_ca`, `builds_settings_with_starttls_and_no_verify`, `missing_ca_file_is_an_error`, `garbage_ca_file_yields_no_certificates`, **and `builds_settings_with_valid_custom_ca`** all green. The last one proves the `ClientConfig::builder()`/`set_config` path works (single crypto provider); a CryptoProvider panic here means Step 2's `default-features = false` was dropped.

- [ ] **Step 7: Run the full library test suite**

Run: `cargo test -p edaptor`
Expected: ~309 lib tests pass; `live_*` integration tests SKIP (no `EDAPTOR_TEST_LDAP_URI`). No failures.

- [ ] **Step 8: Clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings. (Watch for an unused-import warning on `RootCertStore`/`CertificateDer` — both are used in the CA branch, so a warning here means a typo in Step 3.)

- [ ] **Step 9: Commit**

```bash
cargo fmt --check
git add Cargo.toml Cargo.lock src/ldap/tls.rs
git commit -m "$(cat <<'EOF'
refactor(tls): migrate TLS backend from native-tls to rustls

Swap ldap3 to default-features=false + tls-rustls-ring, drop native-tls,
add rustls + rustls-pemfile. Rewrite build_settings to build a rustls
ClientConfig for the custom-CA case via set_config; verify=false continues
to use ldap3's set_no_tls_verify (and now authoritatively subsumes any CA,
matching the old accept_invalid_certs semantics). Removes OpenSSL from the
dependency tree so static musl release builds need no vendoring.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Verify OpenSSL is gone and rustls is the only TLS backend

No code change — this task is the proof that the migration achieved its purpose. It is a real task because the entire reason for the migration is the dependency-tree shape, and that must be asserted, not assumed.

**Files:** none (verification only)

- [ ] **Step 1: Confirm no OpenSSL anywhere in the tree**

Run: `cargo tree -i openssl-sys`
Expected: errors out / prints nothing — `error: package ID specification 'openssl-sys' did not match any packages`. **If it prints a dependency tree, the migration is incomplete** — something still pulls native-tls; re-check that `default-features = false` is set on ldap3 in `Cargo.toml`.

- [ ] **Step 2: Confirm native-tls is gone**

Run: `cargo tree -i native-tls`
Expected: no match (same "did not match any packages" error).

- [ ] **Step 3: Confirm rustls resolved to 0.23.x**

Run: `cargo tree -i rustls`
Expected: shows `rustls v0.23.x` (≥ 0.23.31) with both `edaptor` and `ldap3` among its reverse dependencies — proving our direct dep unified with ldap3's.

- [ ] **Step 4: Re-confirm the facade boundary is intact**

Run: `! grep -rl "use ratatui\|use tui_" src | grep -v "^src/ui/"`
Expected: exits 0 (no offending files). This is unrelated to TLS but is the project's standing invariant — confirm the migration did not accidentally touch a UI file.

- [ ] **Step 5: Commit nothing; record the result**

This task produces no commit. Record in the task tracker / handover that `cargo tree -i openssl-sys` is empty. If you keep a checklist, tick "OpenSSL removed from tree — verified".

---

## Task 3 (optional, gated): Live `ldaps://` smoke check

This is the only thing the unit tests cannot prove: that a real TLS handshake still succeeds against a live server. It is **optional** and requires the podman test server; skip if no server is available, but do it before declaring the migration battle-tested.

**Files:** none (manual / gated)

- [ ] **Step 1: Start the test server**

Run:
```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
```

- [ ] **Step 2: Run the gated live tests (these open real ldap3 connections)**

Run: `cargo test -p edaptor live_`
Expected: `live_write`, `live_membership`, `live_seed`, `live_structure`, `live_templates`, `live_samba` now run (no longer SKIP) and pass — exercising the rustls-backed connection path against the live server.

- [ ] **Step 3: (If the server exposes ldaps://) manual custom-CA connect**

If the podman server is configured with a TLS listener and a CA, point a throwaway config's `[server]` `uri = "ldaps://localhost:1636"` with `tls.ca_cert = "<ca.pem>"` and run `cargo run -- --config <that>.toml` far enough to confirm the bind succeeds. This exercises the `set_config` custom-CA branch end to end. If the server has no TLS listener, note that the custom-CA branch is covered only by the unit tests + structural review and move on.

- [ ] **Step 4: Stop the server**

Run: `scripts/test-ldap.sh stop`

---

## Self-review (done while writing)

- **Spec coverage:** design-spec component 8 (Cargo.toml deps, `build_settings` rewrite preserving connect-timeout/StartTLS/custom-CA/`verify=false`, the 4 unit tests with the garbage-CA assertion updated, module doc comment, `cargo tree -i openssl-sys` empty) → all covered by Tasks 1-2; the live smoke check (spec "Verification") → Task 3. The `license = "MIT"` line the spec also lists under component 8 is intentionally **deferred to the build-system plan** (it owns `Cargo.toml` `[package]` metadata, LICENSE, and README) to avoid two plans editing the same field.
- **Placeholder scan:** no TBDs; every code step shows full code; every command has an expected result.
- **Type consistency:** `CertificateDer<'static>`, `RootCertStore`, `ClientConfig`, `Arc`, `set_config`/`set_no_tls_verify`/`set_starttls`/`set_conn_timeout` all match the verified ldap3 0.12.1 / rustls 0.23 surface. Test names `garbage_ca_file_yields_no_certificates` and `builds_settings_with_valid_custom_ca` are used consistently in Steps 1 and 6.
- **Crypto-provider trap closed:** the `rustls` dep is `default-features = false` (drops aws-lc-rs; keeps ldap3's `ring`), and `builds_settings_with_valid_custom_ca` is the only test that reaches `ClientConfig::builder()`, so a provider regression fails `cargo test` instead of hiding until a live CA loads.
