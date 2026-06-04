# Defaults

The optional `[profile.defaults]` table seeds new entries. It **fills EMPTY
fields on create only** — values an operator has already typed are never
overwritten, and defaults never apply when editing an existing entry.

```toml
[profile.defaults]
loginShell    = "/bin/bash"
homeDirectory = "/home/{uid}"
uidNumber     = "{next:10000-60000}"
```

Each entry maps an attribute name to a default *value expression*. There are
three kinds.

## Literal

A fixed string, written verbatim into the field:

```toml
loginShell = "/bin/bash"
```

Every newly created entry of this profile that leaves `loginShell` empty gets
`/bin/bash`.

## Template

A string containing `{attr}` placeholders. Each placeholder is substituted from
another field's value on the same entry at create time:

```toml
homeDirectory = "/home/{uid}"
```

If the operator enters `uid = bob`, the empty `homeDirectory` becomes
`/home/bob`. Literal text outside the braces is kept as-is.

## Auto-number

The expression `"{next:MIN-MAX}"` allocates the **next free value in the inclusive
range `[MIN, MAX]`** across the whole directory:

```toml
uidNumber = "{next:10000-60000}"
```

To compute the next free value, eDAPtor scans the directory for existing values
of the attribute and picks the lowest unused number in range.

### Size-limit caveat

The auto-number scan is only safe if it sees **every** existing value. OpenLDAP
imposes a server size limit (`olcSizeLimit`, default 500) that truncates large
result sets for ordinary identities. If eDAPtor's scan is **truncated by a
server size limit**, it cannot guarantee the chosen number is actually free, so
it **refuses to allocate** rather than risk a collision.

The fix is to **bind with a high-limit identity** — a DN whose size limit is
high enough (or unlimited, such as the directory's root DN) to return all
existing values in one scan. See [`bind_dn`](server-auth.md) under Server &
Authentication, and [LDAP Constraints](../concepts/ldap-constraints.md) for the
underlying limit.
