# Profile Detection

eDAPtor works out its entry profiles from the directory. At startup it samples
every container, groups the entries it finds by structural object class, and
turns each group into a profile named `<kind>-<container>`: `user-people`,
`posixgroup-groups`, `organizationalunit-example`. A config with only `[server]`
and `[auth]` can browse, edit and create.

## What is detected

| Part | Rule |
|---|---|
| object classes | classes carried by more than half of the group |
| `rdn_attr` | the most common RDN attribute |
| `show`, `search_attrs`, `label` | MUST attributes and the optional attributes most entries carry; `label = "{cn} ({uid})"` when the two differ |
| defaults | templates such as `uid = "{cn}"`, `cn = "{givenName} {sn}"`, `homeDirectory = "/home/{uid}"`, and shared values such as `loginShell = "/bin/bash"` |
| user-private groups | when users have a `posixGroup` named after them with `gidNumber = uidNumber`: `gidNumber = "{uidNumber}"` and a companion group |
| shared primary group | when most users share one `gidNumber`, that value |
| Samba | `sambaSID = "{auto:sambaSID}"` for `sambaSamAccount` profiles |
| pickers | `memberUid`, `member`, `uniqueMember` pickers and a `gidNumber` lookup, pointing at the matching detected profiles |
| number ranges | `uidNumber` / `gidNumber` ranges, computed when you create an entry (see below) |

A rule applies when **more than half** of the sampled entries follow it and at
least **3** entries were sampled. Entries that break an applied rule are listed
as exceptions by `edaptor profiles`.

### Number ranges

When you create an entry, eDAPtor reads every `uidNumber` and `gidNumber` in the
directory, splits the numbers into blocks wherever two neighbours are more than
1000 apart, takes the block that holds most of the profile's own numbers, and
uses `MIN` = that block's lowest number rounded down to a multiple of 1000 and
`MAX` = one below the next block (or `max(60000, MIN + 9999)`). The new number is
one above the highest number in use in the block. With user-private groups, user
and group numbers share one space. A `{next:MIN-MAX}` in the config replaces the
detected range.

### When there is too little data

A new or nearly empty directory gets what Ubuntu's `useradd` does, with one
change: LDAP numbers start at **10000**, because every client machine hands out
1000 and up to its own local users, and an LDAP account must not share a number
with them.

- With no numbers in use, users and groups are numbered from `{next:10000-60000}`.
  With one or two numbers in use, the block rule above continues from them.
- Every user profile, also one written only in your config, gets a private
  group (`gidNumber = "{uidNumber}"` and a companion `posixGroup` named after the
  user, in the posix-group profile's container or else in `ou=groups` directly
  under `base_dn`), unless the users already in the directory show otherwise
  (for example two users sharing group 100).

These values never replace anything from your config or from detection.
`edaptor profiles` marks them `# assumed: <reason>`; `suppress` removes them like any
detected part, and `[detect] enabled = false` turns them off.

### Limits

Sampling reads at most 200 entries per container and 100 containers, and stops
after 10 seconds; a cut-short sample is marked `partial`. It never reads
`userPassword` or other secrets.

## Overriding detection

A `[[profile]]` block merges over the detected profile with the same `name`
(case-insensitive), or with the same `search_base` and structural class. The
merged profile keeps the config's name. Keys you set replace the detected ones;
`[profile.defaults]` and `[profile.widget.*]` merge per attribute; a
`[profile.companion]` replaces the detected one as a whole.

```toml
[[profile]]
name     = "user-people"
suppress = ["companion", "defaults.loginShell", "widget.gidNumber"]

[profile.defaults]
uidNumber = "{next:10000-19999}"

[[profile]]
name    = "organizationalunit-example"
enabled = false
```

`suppress` removes single detected parts: `companion`, `defaults.<attr>`,
`widget.<attr>`, `label`, `show`, `search_attrs`. `enabled = false` removes a
whole profile. A block that matches no detected profile and has no
`object_classes` is ignored with a warning.

Detection also adds its parts to hand-written profiles it matches, so a create
may write a companion group your config never mentioned. `edaptor profiles`
shows every such addition.

Two details help when you suppress parts:

- `suppress = ["defaults.gidNumber"]` removes only the detected `gidNumber`
  default. The companion group (the user-private group) and the shared
  `uidNumber`/`gidNumber` range stay, so a create still writes the private group
  and numbers it from the unified range. Add `"companion"` to drop the group.
- A widget that points at a detected profile must use the name the profile has
  after the merge. When a config block renames a detected profile, references
  by the detected name no longer resolve; write the config name in `candidate`.

To switch detection off:

```toml
[detect]
enabled = false
```

eDAPtor then behaves exactly as before: every `[[profile]]` needs `name` and
`object_classes`, no Samba domain lookup happens automatically, and nothing is
assumed.

## Startup messages

Startup prints `detecting profiles…` and, for anything worth knowing, lines
starting with `warning:` on stderr: a container that could not be read, a
sample cut short by the limits, or a `[[profile]]` block that matches nothing.
When detection failed, or detected parts were dropped, the TUI also says so in
the status line; the message stays until the first key press or mouse click.
With detection enabled, a profile that includes `sambaSamAccount` also triggers
the Samba domain lookup on its own, so `sambaSID` gets its generator without
`[samba] domain_sid`.

## `edaptor profiles`

Prints the profiles in effect as TOML, ready to paste into a config. Each value
carries a comment saying where it came from (`# detected: 12/12`, `# config`,
`# config (detected "/bin/bash", 11/12)`); suppressed parts and exceptions are
listed as comments. `--detected-only` shows detection before the merge. Notes
and warnings go to stderr.

An excerpt from the demo server:

```toml
[[profile]]
name = "user-people"  # detected: 200 entries in ou=people,dc=example,dc=org (partial sample)
rdn_attr = "uid"  # detected: 200/200
label = "{cn} ({uid})"  # detected: 200/200
[profile.defaults]
homeDirectory = "/home/{uid}"  # detected: 200/200
loginShell = "/bin/bash"  # detected: 200/200
uidNumber = "{next:10000-60000}"  # detected at dump time: in use 10000-10599; no higher block
```

## `edaptor passwd`

`edaptor passwd <user>` prints `detecting profiles…` on stderr on every run.
It searches only profiles that have a password widget or a password-bearing
object class, so a private group with the user's name is never a second match.
