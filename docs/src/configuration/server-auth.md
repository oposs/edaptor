# Server & Authentication

The `[server]`, `[server.tls]`, and `[auth]` tables tell eDAPtor where the
directory is, how to trust its TLS certificate, and how to bind.

## `[server]`

```toml
[server]
uri          = "ldaps://ldap.example.com"   # ldap://, ldaps://, or ldapi:///
base_dn      = "dc=example,dc=com"
start_tls    = false                          # true upgrades an ldap:// connection; do NOT combine with ldaps://
read_only    = false                          # true disables all write actions in the TUI
timeout_secs = 10                             # bound the TCP connect so an unreachable server cannot hang
```

- **`uri`** — the directory URL:
  - `ldap://` — plaintext TCP, usually combined with `start_tls = true`.
  - `ldaps://` — implicit TLS on the LDAPS port (636).
  - `ldapi:///` — Unix domain socket on the local host (OpenLDAP only). Connects
    to the default slapd socket at `/var/run/slapd/ldapi`. The socket path can be
    URL-encoded into the URI if it differs from the default, e.g.
    `ldapi://%2Ftmp%2Fslapd.sock`. Use with `auth.method = "external"` for
    password-free root access.
- **`base_dn`** — the root of the subtree eDAPtor loads and browses.
- **`start_tls`** — when `true`, an `ldap://` connection is upgraded to TLS with
  StartTLS after connecting. **Do not combine `start_tls = true` with an
  `ldaps://` URI** — `ldaps://` is already TLS, so layering StartTLS on top is a
  configuration error.
- **`read_only`** — when `true`, all write actions in the TUI are disabled. Use
  this for a safe browse-only session. (Because OpenLDAP exposes no per-entry
  effective-rights signal, read-only is a global mode rather than something
  eDAPtor can decide per entry — see
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
  Setting `verify = false` makes eDAPtor **accept any certificate**, defeating
  the protection TLS provides; use it **only for testing** against a server with
  a self-signed certificate you cannot otherwise trust.

eDAPtor's TLS is built on the **rustls** backend, so no OpenSSL is required at
build or run time.

## `[auth]`

```toml
[auth]
method          = "simple"                    # "simple" or "external"
bind_dn         = "cn=ldapmanager,dc=example,dc=com"
# The password is NEVER stored in this file. Choose a source:
#   "prompt"            -> ask interactively at startup (no echo)
#   "env:VAR"           -> read environment variable VAR
#   "command:some cmd"  -> run a command and read its stdout
password_source = "prompt"
```

- **`method`** — how to authenticate after connecting:
  - `"simple"` — a simple bind with a DN and password. Requires `bind_dn` and
    `password_source`.
  - `"external"` — SASL EXTERNAL bind. Used with `ldapi:///` for password-free
    root access; the identity is taken from the OS user (or the server's
    `olcAuthzRegexp` mapping). No `bind_dn` or `password_source` needed.
  - GSSAPI/Kerberos is not yet supported.
- **`bind_dn`** — the DN to bind as (`"simple"` only). To create users and
  auto-number `uidNumber` reliably, bind as an identity with a high (or
  unlimited) server size limit so the directory scan is not truncated — see
  [Defaults](defaults.md).
- **`password_source`** — where the bind password comes from (`"simple"` only).
  **The password is never stored in this file.** Choose one of:
  - `"prompt"` — ask interactively at startup (no echo).
  - `"env:VAR"` — read the environment variable named `VAR`.
  - `"command:some cmd"` — run `some cmd` and read its standard output (e.g. a
    secret-manager helper such as `command:pass show ldap/manager`).

### Unix domain socket example

When running on the slapd host as root (or a user mapped to rootdn), skip TLS
and passwords entirely:

```toml
[server]
uri     = "ldapi:///"
base_dn = "dc=example,dc=com"

[auth]
method = "external"
```
