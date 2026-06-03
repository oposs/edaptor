# Test-server provisioning assets

These files turn the bare Bitnami OpenLDAP started by `../test-ldap.sh` into a
directory that mirrors the `oposs.openldap` ansible role (schemas, overlays,
password policy) and seeds it with realistic data.

`test-ldap.sh start` loads them in this order:

| File | Bind identity | Purpose |
|------|---------------|---------|
| `schema/samba.ldif` | `cn=admin,cn=config` | Samba schema (verbatim from the role) |
| `schema/mail.ldif`  | `cn=admin,cn=config` | Mail schema (`olcSchemaConfig` form of the role's `mail.schema`) |
| `config/overlays.ldif` | `cn=admin,cn=config` | `memberof` + `refint` + `ppolicy` modules/overlays on `{2}mdb` |
| `data/ppolicy.ldif` | `cn=admin,dc=example,dc=org` | `ou=policies` + default/serviceaccounts policies |
| `data/base.ldif` | `cn=admin,dc=example,dc=org` | `ou=people`/`ou=services`, `sambaDomain`, service accounts (`ou=groups` is auto-created by the Bitnami image, so it is not added here) |
| `data/testdata.ldif` | `cn=admin,dc=example,dc=org` | 600 users / 25 groups (generated) |

**Module path note:** on the Bitnami image the overlay `.so`s live in
`/opt/bitnami/openldap/lib/openldap`, not the default `libexec/` path — hence the
fresh `cn=module{1}` in `overlays.ldif`.

**Shared user password:** every generated user (and the service accounts) has
password `test123`.

## Regenerating `testdata.ldif`

```bash
cargo run --bin gen-testdata            # 600 users, dc=example,dc=org
cargo run --bin gen-testdata -- --users 150
cargo run --bin gen-testdata -- --out - # to stdout
```

Output is deterministic (no RNG, fixed timestamp), so regenerating with the same
flags produces an identical file.
