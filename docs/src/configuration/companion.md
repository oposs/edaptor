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
cn        = "{uid}"         # the group is named after the user's login (uid)
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

## Server requirement: uniqueness rules must not forbid the shared value

A user-private group shares its `gidNumber` with the account that owns it. That is
the point of the pattern, and it means **two entries legitimately carry the same
`gidNumber`**.

If the server runs the OpenLDAP `unique` overlay with a rule such as

```
olcUniqueURI: ldap:///?gidNumber?sub
```

then that shared value is forbidden across the whole subtree, and **every create
through this profile fails** with a constraint violation. Restrict the rule to the
entries it is meant to protect instead:

```
olcUniqueURI: ldap:///?gidNumber?sub?(objectClass=posixGroup)
```

Group `gidNumber`s stay unique; the account's reference to its own private group is
left alone. The [test server](../reference/test-server.md) is provisioned this way,
and `tests/live_unique_overlay.rs` pins both halves of the behaviour.

Note that a rejection like this is **deferred to the transaction commit**, where slapd
reports the result code with no diagnostic message. eDAPtor recognises that case and
retries the entries outside the transaction to recover the server's real reason, then
shows it with the DN of the entry that failed — see
[LDAP Constraints](../concepts/ldap-constraints.md).
