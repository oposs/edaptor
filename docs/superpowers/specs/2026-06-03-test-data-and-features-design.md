# Design: ansible-faithful test directory with rich seed data

**Date:** 2026-06-03
**Status:** Approved (design); implementation plan to follow
**Branch (suggested):** `feat-test-data`

## Problem

The local podman LDAP used for edaptor development (`scripts/test-ldap.sh`)
starts a bare Bitnami OpenLDAP: base `dc=example,dc=org`, an admin account, and
Bitnami's default `ou=users`/`user01`. It has **no** custom schemas, **no**
overlays, **no** password policies, and only a trivial amount of data. That is
not representative of the directories edaptor targets in production, which are
configured by the `oposs.openldap` ansible role
(`~/checkouts/oep-ansible/playbooks/config.d/roles/oposs.openldap`).

We want two things:

1. **Feature parity with the ansible role** on the podman server — the same
   custom schemas (Samba, mail), the same overlays (`memberof`, `refint`,
   `ppolicy`), the same password-policy data and service accounts.
2. **Rich, realistic seed data** — hundreds of users (~600, 120 each across
   **5 departments**) and tens of groups, so edaptor's pickers, paged subtree
   scan, membership editing, gidNumber lookup, and Samba lifecycle all have
   meaningful data to act on.

The user's priority, stated during brainstorming: *"the important thing is the
schemas"*; the image base is flexible.

## Constraints discovered

- **Existing live tests must keep passing untouched.** `tests/live_*.rs` are
  hardcoded to `dc=example,dc=org`, `cn=admin`/`adminpassword`, and self-seed
  under **`ou=users`** (the Bitnami default), cleaning up after themselves.
  `live_structure.rs` asserts `ou=users` is present and that the eager paged
  scan returns entries past the default size limit. Therefore we must:
  - keep `dc=example,dc=org` and `cn=admin`/`adminpassword`,
  - keep Bitnami's default tree creation (do **not** switch to
    `LDAP_CUSTOM_LDIF_DIR`, which would suppress `ou=users`/`user01`),
  - add `ou=people`/`ou=groups`/data **alongside** the default tree.

- **No runtime dependency on the `oep-ansible` checkout.** That is a separate
  repo. All provisioning assets are vendored into this repo, with a header
  comment recording provenance.

## Spike results (validated, not assumed)

Run against `docker.io/bitnamilegacy/openldap:2.6.9`:

- Setting `LDAP_CONFIG_ADMIN_ENABLED=yes` + `LDAP_CONFIG_ADMIN_USERNAME` +
  `LDAP_CONFIG_ADMIN_PASSWORD` yields a working `cn=admin,cn=config` identity
  that can write `cn=config` **over the network** (`-x -D cn=admin,cn=config`).
  This sidesteps the EXTERNAL/peercred-uid question entirely.
- The role's `samba.ldif` (already in `olcSchemaConfig` form,
  `dn: cn=samba,cn=schema,cn=config`) loads cleanly via the config admin.
- **Gotcha:** the pre-existing `cn=module{0}` has
  `olcModulePath: /opt/bitnami/openldap/libexec/openldap`, but the overlay
  `.so` files actually live in `/opt/bitnami/openldap/lib/openldap`
  (`memberof.so`, `refint.so`, `ppolicy.so` all present). Loading modules
  against the default path fails with `<olcModuleLoad> handler exited with 1`.
  Fix: add a **new** `cn=module{1},cn=config` entry with
  `olcModulePath: /opt/bitnami/openldap/lib/openldap`.
- The data backend is `olcDatabase={2}mdb,cn=config`. Adding the `memberof`
  and `refint` overlays there works, and **`memberOf` auto-populates** on a
  user when a `groupOfNames` referencing it is added.

## Architecture

Four pieces, all in the `ldapedit` repo.

### 1. Vendored provisioning assets — `scripts/ldap-provision/`

```
scripts/ldap-provision/
  schema/
    samba.ldif         # olcSchemaConfig; copied verbatim from the role's files/samba.ldif
    mail.ldif          # olcSchemaConfig; converted once from the role's files/schemas/mail.schema
  config/
    overlays.ldif      # cn=module{1} (correct lib path) + memberof/refint/ppolicy overlays on {2}mdb
  data/
    base.ldif          # ou=people, ou=groups, ou=services; sambaDomain entry; service accounts
    ppolicy.ldif       # ou=policies + cn=default / cn=serviceaccounts pwdPolicy entries
    testdata.ldif      # GENERATED — committed, regenerable (see generator)
  README.md            # what each file is, provenance, how to regenerate testdata.ldif
```

- **`schema/samba.ldif`** — verbatim copy of the role's `files/samba.ldif`.
- **`schema/mail.ldif`** — produced once from `files/schemas/mail.schema` via
  `slaptest`-based conversion (the same technique the role's `schemas.yaml`
  task uses) in a throwaway container, then committed. Header notes provenance.
- **`config/overlays.ldif`** — mirrors the role's `templates/overlays.ldif.j2`
  but: (a) emits a fresh `cn=module{1}` with
  `olcModulePath /opt/bitnami/openldap/lib/openldap` and module names
  `memberof.so`/`refint.so`/`ppolicy.so`; (b) targets `olcDatabase={2}mdb`;
  (c) sets `olcPPolicyDefault: cn=default,ou=policies,dc=example,dc=org`.
- **`data/base.ldif`** — the three OUs the role creates (`people`, `groups`,
  `services`; `computers` omitted as edaptor does not use it), representative
  service-account entries under `ou=services` (`cn=sambamanager`,
  `cn=nssclient`, and a `cn=ldapmanager` placeholder — note Bitnami's functional
  rootDN is `cn=admin`, which the demo config binds as; these entries are
  illustrative, not active bind identities), and one `sambaDomain` entry:
  - `dn: sambaDomainName=EXAMPLE,dc=example,dc=org`
  - `objectClass: sambaDomain`
  - `sambaSID: S-1-5-21-1234567890-987654321-1122334455` (fixed test SID)
  - `sambaAlgorithmicRidBase: 1000`
- **`data/ppolicy.ldif`** — `ou=policies` plus the `cn=default` and
  `cn=serviceaccounts` `pwdPolicy` entries from the role's
  `templates/ppolicy_default.ldif.j2`, with literal defaults inlined.

### 2. Generator — `src/bin/gen-testdata.rs`

A cargo `[[bin]]` in the `edaptor` crate. Reuses the crate's own pure Samba
logic so generated values exactly match what edaptor writes:

- `edaptor::samba::nthash::nt_hash` — `sambaNTPassword`.
- `edaptor::samba::sid` — algorithmic RID → `sambaSID` (use the crate's RID
  formula so test data is consistent with edaptor's own create path; confirm
  the public surface during planning).
- `edaptor::samba::account` — `sambaAcctFlags`.

Properties:

- **Deterministic.** No RNG / no clock: every value derives from a stable index
  and fixed in-code name pools (first names, last names, department names).
  `sambaPwdLastSet` uses a fixed constant timestamp, not "now".
- **Output:** writes LDIF to a path given as argv (default
  `scripts/ldap-provision/data/testdata.ldif`), or `-` for stdout.
- **Default counts (configurable via flags):** `--users 600 --depts 5`
  (≈120 users/dept). All users go under a **flat `ou=people`**.

Per-user attributes (`uid=<first><lastinitial><n>` style, unique):

- objectClasses `inetOrgPerson`, `posixAccount`, `shadowAccount`,
  `sambaSamAccount`
- `cn`, `sn`, `givenName`, `mail` (`first.last@example.org`)
- `uidNumber` (10000 + index), `gidNumber` (= the user's department group gid)
- `homeDirectory` (`/home/<uid>`), `loginShell` (`/bin/bash`)
- `departmentNumber` and `ou` set to the department name
- `userPassword` — a single fixed `{SSHA}` value (one shared known test
  password; documented in the provisioning README)
- `sambaSID` (domain SID + algorithmic RID from `uidNumber`),
  `sambaNTPassword`, `sambaPwdLastSet`, `sambaAcctFlags`

Groups (~25), under `ou=groups`. **Important schema constraint:** `groupOfNames`
and `posixGroup` are *both STRUCTURAL* in the stock `nis`/RFC 2307 schema that
Bitnami (and the role) load — an entry may have only one structural object
class, so they **cannot** be combined on one entry. The faithful representation
(matching what the role produces with the `nis` schema) is **separate** entries,
which also cleanly exercises edaptor's two distinct group consumers:

- **5 department membership groups** — `cn=<Dept>,ou=groups`, structural
  `groupOfNames`, `member` = DNs of every user in that department. These are
  what the `group` profile and the `group-membership` relation act on.
- **5 department posix groups** — `cn=<dept>-unix,ou=groups`, structural
  `posixGroup`, `gidNumber` (5000..5004), `memberUid` = the uids in that
  department, plus `description`. These are what `profile.lookup.gidNumber`
  picks from; each user's `gidNumber` equals their department's `posixGroup`
  gid. (Distinct `cn`s — `cn` matching is case-insensitive, so the groupOfNames
  `cn=Engineering` and posixGroup `cn=engineering-unix` must not collapse to the
  same RDN.)
- **~15 functional/role groups** — `cn=all-staff`, `cn=managers`,
  `cn=vpn-users`, project teams, etc., structural `groupOfNames` spanning
  multiple departments, each with a deterministic non-empty member subset, so
  reverse `memberOf` views are non-trivial.

Ordering inside the LDIF: OUs already exist (from `base.ldif`); emit all users
first, then groups (so group `member` DNs resolve and `memberOf` populates).

### 3. Updated `scripts/test-ldap.sh`

`start` changes:

1. Add `-e LDAP_CONFIG_ADMIN_ENABLED=yes -e LDAP_CONFIG_ADMIN_USERNAME=admin
   -e LDAP_CONFIG_ADMIN_PASSWORD=configpassword` to the `podman run`.
2. After the existing readiness loop, run provisioning (each step
   `podman cp`/`podman exec` the relevant file, fail loud on error):
   - `schema/samba.ldif`, `schema/mail.ldif` → `ldapadd -x -D cn=admin,cn=config`
   - `config/overlays.ldif` → `ldapadd -x -D cn=admin,cn=config`
   - `data/ppolicy.ldif`, `data/base.ldif`, `data/testdata.ldif`
     → `ldapadd -x -D cn=admin,dc=example,dc=org`
3. Update the printed "Ready" hints to also mention the config-admin password
   and the demo config path.

Because the container is `--rm` and recreated on every `start`, provisioning
always runs against a clean DB — no idempotency logic needed. Existing behavior
(port 1389, `dc=example,dc=org`, `cn=admin`/`adminpassword`, the default
`ou=users` tree) is preserved.

### 4. Demo config — `examples/demo-config.toml`

A complete edaptor config pointing at this server: `dc=example,dc=org`,
`ou=people` / `ou=groups`, the `user` and `group` profiles, the
`profile.lookup.gidNumber` against `posixGroup`, the `group-membership`
relation, and the Samba settings (domain `EXAMPLE`). Lets the user run
`edaptor --config examples/demo-config.toml` and explore the full dataset.
Bind as `cn=admin,dc=example,dc=org` (high-limit identity, so the autonumber
scan is not truncated).

## Testing

- **Generator unit tests** (in `gen-testdata.rs` or a sibling module):
  - exactly `--users` user entries and the expected group count emitted
    (5 membership + 5 posix + functional);
  - `uidNumber` and `sambaSID` are unique across all users;
  - every user's `gidNumber` equals exactly one department posixGroup's
    `gidNumber`;
  - every membership group's `member` DNs all exist among emitted users.
- **New gated `tests/live_seed.rs`** (skips cleanly without
  `EDAPTOR_TEST_LDAP_URI`):
  - `ou=people` subtree returns > 100 entries (exercises paged scan past the
    500 size limit at 600 users);
  - at least 5 `posixGroup` entries expose a `gidNumber`, and at least 5
    `groupOfNames` entries expose `member`;
  - the `sambaDomain` entry is discoverable and yields a `sambaSID`.
- The existing `live_membership` / `live_samba` / `live_write` /
  `live_structure` / `live_templates` suites must still pass unchanged.

## Out of scope

- TLS / LDAPS on the test server (edaptor TLS is covered elsewhere).
- Replication, backups, the `computers` OU, NSS/PAM specifics — role features
  not exercised by edaptor.
- Changing the production ansible role.

## File inventory (created/modified)

Created:
- `scripts/ldap-provision/README.md`
- `scripts/ldap-provision/schema/samba.ldif`
- `scripts/ldap-provision/schema/mail.ldif`
- `scripts/ldap-provision/config/overlays.ldif`
- `scripts/ldap-provision/data/base.ldif`
- `scripts/ldap-provision/data/ppolicy.ldif`
- `scripts/ldap-provision/data/testdata.ldif` (generated, committed)
- `src/bin/gen-testdata.rs`
- `examples/demo-config.toml`
- `tests/live_seed.rs`

Modified:
- `scripts/test-ldap.sh`
- `Cargo.toml` (new `[[bin]]`)
- `README.md` (mention the rich test server + demo config)
