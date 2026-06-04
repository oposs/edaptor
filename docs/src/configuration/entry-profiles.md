# Entry Profiles

A `[[profile]]` table declares one *kind* of entry eDAPtor manages — a "user", a
"group", a "posixgroup", and so on. Because `[[profile]]` is an array of tables,
you repeat it once per kind. Each profile says what its entries are made of and
where they live; the edit form itself is generated from the live schema for the
declared object classes.

## Profile keys

```toml
[[profile]]
name           = "user"
object_classes = ["inetOrgPerson", "posixAccount", "shadowAccount"]
rdn_attr       = "uid"
search_base    = "ou=people,dc=example,dc=com"
show           = ["uid", "cn", "sn", "givenName", "mail", "uidNumber", "gidNumber", "homeDirectory"]
search_attrs   = ["cn", "uid", "mail"]        # picker searches these attributes
label          = "{cn} ({uid})"               # e.g. "Bob Baker (bob)"
```

- **`name`** — the profile's identifier. It is referenced by pickers in other
  profiles (a picker's `candidate` names a profile by this key).
- **`object_classes`** — the object classes an entry of this kind carries.
  eDAPtor introspects these against `cn=subschema` to build the edit form, and
  emits `top` plus all listed classes (deduplicated) on create. The list also
  forms the search filter that finds entries of this profile.
- **`rdn_attr`** — the attribute that forms the entry's RDN (relative
  distinguished name). For users this is typically `uid`; for groups, `cn`.
- **`search_base`** — the subtree under which entries of this profile live and
  are searched.
- **`show`** — the attributes displayed (and editable) for this profile.
- **`search_attrs`** — the attributes a picker's substring search matches
  against when this profile is a candidate. It follows a **fallback chain**: if
  `search_attrs` is omitted, eDAPtor falls back to `show`; if `show` is also
  absent, it falls back to `["cn"]`.
- **`label`** — how an entry of this profile is rendered in the membership
  picker. `{attr}` is substituted by that attribute's value and literal text is
  kept, so `"{cn} ({uid})"` renders as e.g. `Bob Baker (bob)`. **`label`
  defaults to `cn`** when omitted.

## Worked examples

### A full user account

The `user` profile above is a complete posix (and optionally Samba) account
template: multiple object classes, defaulted/templated/auto-numbered fields, an
inline password field, and picker bindings that pull values from — or fan out to
— other profiles. Those sub-tables are documented separately:

- [`[profile.defaults]`](defaults.md) — fill empty fields on create.
- [`[profile.password]`](passwords.md) — the inline masked password field.
- [`[profile.picker.<attr>]`](pickers.md) — populate an attribute from a live
  candidate search.

### A `groupOfNames` group

```toml
[[profile]]
name           = "group"
object_classes = ["groupOfNames"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=com"
show           = ["cn", "description"]
label          = "{cn}"
```

Here `search_attrs` is omitted, so the picker substring search falls back to
`show` (`cn`, `description`). The group's `member` attribute is populated by a
[picker](pickers.md) over the `user` profile.

### A `posixGroup`

```toml
[[profile]]
name           = "posixgroup"
object_classes = ["posixGroup"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=com"
show           = ["cn", "gidNumber", "memberUid", "description"]
label          = "{cn}"
```

A `posixGroup` carries a numeric `gidNumber` and lists its members by `uid` in
`memberUid` (a scalar, not a DN). Both `gidNumber` (consumed by the user
profile's picker) and `memberUid` are wired through [pickers](pickers.md).
