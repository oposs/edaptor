# Server & Authentication

The `[server]`, `[server.tls]`, and `[auth]` tables tell edaptor where the
directory is, how to trust its TLS certificate, and how to bind.

## `[server]`

```toml
[server]
uri          = "ldaps://ldap.example.com"   # ldap:// or ldaps://
base_dn      = "dc=example,dc=com"
start_tls    = false                          # true upgrades an ldap:// connection; do NOT combine with ldaps://
read_only    = false                          # true disables all write actions in the TUI
timeout_secs = 10                             # bound the TCP connect so an unreachable server cannot hang
```

- **`uri`** — the directory URL. Use `ldap://` for a plaintext connection (often
  combined with `start_tls`) or `ldaps://` for implicit TLS on the LDAPS port.
- **`base_dn`** — the root of the subtree edaptor loads and browses.
- **`start_tls`** — when `true`, an `ldap://` connection is upgraded to TLS with
  StartTLS after connecting. **Do not combine `start_tls = true` with an
  `ldaps://` URI** — `ldaps://` is already TLS, so layering StartTLS on top is a
  configuration error.
- **`read_only`** — when `true`, all write actions in the TUI are disabled. Use
  this for a safe browse-only session. (Because OpenLDAP exposes no per-entry
  effective-rights signal, read-only is a global mode rather than something
  edaptor can decide per entry — see
  [LDAP Constraints](../concepts/ldap-constraints.md).)
- **`timeout_secs`** — bounds the TCP connect so an unreachable server cannot
  make the TUI hang.

## `[server.tls]`

This table is **optional**. Omit it entirely to use the system trust store with
full certificate verification — the right default for a server with a publicly
trusted or system-installed certificate.

```toml
[server.tls]
# ca_cert = "/etc/ssl/certs/my-ca.pem"        # trust a custom CA (PEM)
verify    = true                              # set false ONLY for testing — accepts any certificate
```

- **`ca_cert`** — path to a PEM file holding a custom CA certificate to trust, in
  addition to (or instead of) the system store. Use this when your directory's
  certificate is signed by a private/internal CA.
- **`verify`** — certificate verification. Leave it `true`.
  Setting `verify = false` makes edaptor **accept any certificate**, defeating
  the protection TLS provides; use it **only for testing** against a server with
  a self-signed certificate you cannot otherwise trust.

edaptor's TLS is built on the **rustls** backend, so no OpenSSL is required at
build or run time.

## `[auth]`

```toml
[auth]
method          = "simple"                    # simple bind (SASL EXTERNAL/GSSAPI are a later milestone)
bind_dn         = "cn=ldapmanager,dc=example,dc=com"
# The password is NEVER stored in this file. Choose a source:
#   "prompt"            -> ask interactively at startup (no echo)
#   "env:VAR"           -> read environment variable VAR
#   "command:some cmd"  -> run a command and read its stdout
password_source = "prompt"
```

- **`method`** — currently `"simple"` (a simple bind with a DN and password).
  SASL `EXTERNAL`/`GSSAPI` authentication is planned for a later milestone.
- **`bind_dn`** — the DN to bind as. To create users and auto-number `uidNumber`
  reliably, bind as an identity with a high (or unlimited) server size limit so
  the directory scan is not truncated — see [Defaults](defaults.md).
- **`password_source`** — where the bind password comes from. **The password is
  never stored in this file.** Choose one of:
  - `"prompt"` — ask interactively at startup (no echo).
  - `"env:VAR"` — read the environment variable named `VAR`.
  - `"command:some cmd"` — run `some cmd` and read its standard output (e.g. a
    secret-manager helper such as `command:pass show ldap/manager`).
