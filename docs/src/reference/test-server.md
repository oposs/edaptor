# Test Server

eDAPtor ships a script, `scripts/test-ldap.sh`, that stands up a fully
provisioned throwaway OpenLDAP server in a **podman** container (not docker). It
is the fastest way to try the TUI against realistic data and is the backing
server for the gated live test suite.

## What it provisions

`scripts/test-ldap.sh start` launches an OpenLDAP container and then provisions
it to mirror the [`oposs.openldap`](https://github.com/oposs) Ansible role:

- **Schemas:** the Samba and mail schemas.
- **Overlays:** `memberOf`, referential integrity (`refint`), password policy
  (`ppolicy`) and attribute uniqueness (`unique`). The uniqueness rules are
  **filtered** so a user-private group may share its account's `gidNumber` —
  see [Companion Entries](../configuration/companion.md).
- **Password policies** loaded into the directory.
- **Seed data:** roughly **600 users across 5 departments** and about **25
  groups**, generated deterministically (see `scripts/ldap-provision/`).

The base DN is `dc=example,dc=org`, and **all generated users share the password
`test123`**.

## Lifecycle

```bash
scripts/test-ldap.sh start   # run + provision + seed (idempotent)
scripts/test-ldap.sh stop    # stop and remove the container
```

`start` is idempotent: it removes any leftover container from a prior run, waits
for the server to accept connections, then provisions and seeds it.

## Connecting

The server listens on `ldap://localhost:1389`. Set these environment variables
to point eDAPtor and the live tests at it:

```bash
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
```

To explore the seed data interactively, run the TUI against the bundled demo
config (which targets exactly this server):

```bash
edaptor --config examples/demo-config.toml
```

See the [Quick Start](../getting-started/quick-start.md) for the end-to-end walk
through.

## How the live test suite uses it

eDAPtor's `live_*` tests talk to a real directory and are **gated by the
`EDAPTOR_TEST_LDAP_URI` environment variable**: when it is unset, those tests
skip, so `cargo test` is safe with no server running. When it is set (to the URI
above), the live membership, template, seed, structure, and write tests run
against the provisioned server:

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo test -p edaptor        # live_* tests now run
scripts/test-ldap.sh stop
```
