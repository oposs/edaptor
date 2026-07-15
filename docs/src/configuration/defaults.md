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

### Live templating in create mode

When you create a new entry, a **template** default (one containing `{field}`
placeholders, e.g. `cn = "{givenName} {sn}"`) does more than fill once: it keeps
the target in sync with its sources **as you type**, for as long as you have not
edited the target yourself.

- The target fills the moment all its `{…}` sources have values, and re-computes
  whenever a source changes.
- If you type your own value into the target, eDAPtor stops tracking it — the
  field is yours.
- Clear the target back to empty and it **re-arms**: live tracking resumes.
- While any `{…}` source is still empty, the auto target is shown empty.

Literal defaults (`loginShell = "/bin/bash"`) and autonumber defaults
(`{next:MIN-MAX}`) are **not** live — they are applied once. Live templating
applies to **create mode only**; editing an existing entry never rewrites a field
from a template.

Example:

    [profile.defaults]
    cn          = "{givenName} {sn}"
    displayName = "{givenName} {sn}"

## Auto-number

The expression `"{next:MIN-MAX}"` allocates the **next free value in the inclusive
range `[MIN, MAX]`** across the whole directory:

```toml
uidNumber = "{next:10000-60000}"
```

To compute the next free value, eDAPtor scans the directory for existing values
of the attribute and picks the lowest unused number in range.

### Allocating during create (Enter to allocate)

By default an auto-numbered field is resolved at **save** time. In a create form
the field is initially empty and shows the affordance **`⟨Enter to allocate⟩`**;
pressing **Enter** runs the scan immediately and fills in the number. This is
useful when another field depends on the value before save — for example the
[`sambaSID` auto-generate](widgets.md#sambasid-auto-generate-auto-injected)
widget needs a concrete `uidNumber`. Skipping it is fine: the value is still
allocated automatically at save. The field stays editable, so you can also type a
number by hand to override.

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
