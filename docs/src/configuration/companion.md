# Companion Entries

A profile may declare **one companion entry** that eDAPtor creates alongside the primary
whenever you create through that profile. The classic use is a **user-private group**: a
`posixGroup` whose `cn` is the user's `uid` and whose `gidNumber` mirrors the user's.

```toml
[profile.companion]
object_classes = ["posixGroup"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=org"

[profile.companion.attributes]
cn        = "{cn}"          # templates resolve against the primary's final attributes
gidNumber = "{gidNumber}"   # mirrors the user's already-allocated gid
memberUid = "{uid}"
```

- `attributes` values use the same literal / `{attr}` template syntax as
  [Defaults](defaults.md); they resolve against the **primary's** composed attributes
  (including its RDN, defaults, and allocated autonumbers). `{next:…}` autonumbers are
  **not** allowed in a companion.
- `objectClass` comes from `object_classes` (with `top` added); `rdn_attr` must be one of
  the `attributes` keys.

## Atomicity

When the server advertises **LDAP transactions (RFC 5805)**, the primary and the
companion are created in **one atomic transaction** — either both are created or neither
is. Against a server without transaction support, eDAPtor falls back to creating the
**companion first, then the primary**; if the companion fails, the primary is not created.
Both entries are shown in the create confirmation before anything is written.
