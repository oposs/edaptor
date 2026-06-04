# Pickers

A `[profile.picker.<attr>]` table declares how the field for attribute `<attr>`
is populated from a **live candidate search** against another profile. Instead
of typing a raw DN or scalar, the operator opens a picker, searches, and selects
one or more candidate entries; eDAPtor writes the right value(s) for you.

## The four knobs

```toml
[profile.picker.gidNumber]
candidate = "posixgroup"
store     = "gidNumber"
select    = "single"
```

- **`candidate`** *(required)* — the [`name`](entry-profiles.md) of a
  `[[profile]]` that supplies the candidate search scope. The picker searches
  that profile's `search_base` and matches on its `search_attrs`.
- **`store`** *(default `"dn"`)* — what to write per pick. `"dn"` stores the
  candidate's distinguished name; **any other value is treated as an attribute
  name** whose scalar value (read from the picked candidate) is stored instead.
- **`select`** *(default `"auto"`)* — cardinality. `"auto"` derives single vs.
  multi from the attribute's schema arity; `"single"` or `"multi"` override it.
- **`fanout_attr`** *(optional)* — when set, the field is **NOT written to this
  entry at all**. Instead, this entry's DN is added to (or removed from)
  `fanout_attr` on each **picked candidate**. This is how a back-reference is
  maintained by writing the forward reference.

## Worked examples

### `gidNumber` — single-select, stores a scalar

```toml
[profile.picker.gidNumber]
candidate = "posixgroup"
store     = "gidNumber"
select    = "single"
```

A single-select picker over `posixgroup` entries. Because `store = "gidNumber"`,
it writes the **chosen group's `gidNumber` scalar** into the user's `gidNumber`
field — not the group's DN.

### `memberOf` — DN store with fan-out

```toml
[profile.picker.memberOf]
candidate   = "group"
store       = "dn"
fanout_attr = "member"
```

This is a synthetic back-reference. Ticking a group does **not** write
`memberOf` on the user; instead, `fanout_attr = "member"` causes the user's DN
to be added to (or removed from) the `member` attribute of each picked `group`.
The `memberOf` attribute itself is **overlay-maintained** by OpenLDAP's memberOf
overlay, so **eDAPtor never writes it directly** (see
[LDAP Constraints](../concepts/ldap-constraints.md)).

### `member` / `memberUid` — multi-select, DN vs. scalar

```toml
# on the "group" profile
[profile.picker.member]
candidate = "user"
```

```toml
# on the "posixgroup" profile
[profile.picker.memberUid]
candidate = "user"
store     = "uid"
```

Both are multi-select pickers over the `user` profile (cardinality comes from
schema, typically multi). They differ in what they store:

- **`member`** uses the default `store = "dn"`, so each picked user's **DN** is
  written into the `groupOfNames` `member` attribute.
- **`memberUid`** sets `store = "uid"`, so each picked user's **`uid` scalar** is
  written into the `posixGroup` `memberUid` attribute — not a DN.

This symmetry — a group's `member` and a user's `memberOf` are two views of the
same relationship — is what makes [membership editing](../usage/membership.md)
work from either side.
