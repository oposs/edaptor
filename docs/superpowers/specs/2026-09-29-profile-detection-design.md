# Design: profile detection (config by exception)

**Date:** 2026-09-29 · **Branch:** `feat/profile-detection` · **Part 2 of 3** of the
"auto-adapt to the directory" effort. Part 1 (`base_dn` from `namingContexts`) and
Part 3 (reading `cn=config`: overlays, size limits) get their own specs.

## Problem

eDAPtor can browse and edit without profiles, because forms come from the subschema and
the built-in widget bundles (`src/config/builtin_schema.toml`). But **creating** an entry
needs a `[[profile]]` whose `search_base` matches the container, and the pickers
(`gidNumber`, `memberUid`, `member`) need a profile to resolve their candidates. Every
rule that makes a create correct — `uid = "{cn}"`, `homeDirectory = "/home/{uid}"`, the
`uidNumber` range, `gidNumber = uidNumber` with a user-private group — must be written
out by hand.

The directory already holds the answer. On services-argus every user had
`gidNumber = uidNumber` and a private `posixGroup` named after its `uid`; the config did
not say so, so creating a user failed until the rule was added by hand (2026-09-28).

## Goal

eDAPtor derives the profiles from the directory itself. The config shrinks to
connection settings plus **overrides** of what detection got wrong or should not do.
`edaptor profiles` shows what was detected and why, so every override has a visible
starting point.

Non-goals: reading `cn=config` (Part 3), making `base_dn` optional (Part 1), changing the
tree scan, changing how forms, pickers or companions work once a profile exists. The one
consumer that changes is number allocation, which learns detected ranges (§2C).

## Decisions (settled in brainstorming)

- All three knowledge sources are used — schema, server config (Part 3), directory
  content — and **content wins** where they disagree.
- Profiles are found by **grouping** entries; **known patterns** then add meaning. Entry
  kinds no pattern knows still get a basic profile.
- A rule applies when **more than half** of the sampled entries follow it **and** at
  least **3** entries were sampled. Entries that break an applied rule are listed as
  exceptions in the dump.
- Detected profiles are **named**; a config `[[profile]]` **merges over** the matching
  detected profile. Whole profiles or single detected parts can be suppressed.
- Detection is **on by default**, also for existing configs, and it **adds** detected
  parts (defaults, companion, widgets) to matched hand-written profiles too. This is
  deliberate: the goal is that users can drop their custom profiles. The dump makes every
  addition visible; one `suppress` line removes it; `[detect] enabled = false` restores
  today's behaviour exactly. (Pushback item 7, option a.)
- Detected profile names **always** carry their container: `<name>-<container RDN
  value>` (`user-people`, `posixgroup-groups`). Longer, but a name never changes when
  another container appears. (Pushback item 6, option a.)
- Number ranges detect both `MIN` and `MAX` (rule C below).
- **Detection never stops eDAPtor.** A detected part that fails validation is dropped
  with a note; only the user's own config can cause a load error.

## 1. Architecture and data flow

```
 sample (LDAP)  ──►  detect (pure)  ──►  merge with config (pure)  ──►  Vec<EntryProfile>
 containers +        DetectedProfile      config wins, suppress,         (unchanged type,
 per-container       + evidence           enabled=false                  unchanged consumers)
 samples + lookups
```

1. **Sample** (`src/detect/sample.rs`, runs on the LDAP worker thread):
   - **Find containers:** one subtree search under `base_dn` with filter
     `(hasSubordinates=TRUE)`, attributes `1.1`. Verified on the demo server (6 of 640
     entries, admin and anonymous) and on services-argus (5 containers, 8 ms, root via
     `EXTERNAL`). If the server rejects the filter or returns an error, fall back to
     `(|(objectClass=organizationalUnit)(objectClass=organization)(objectClass=domain)(objectClass=container))`
     and record the fallback as a note.
   - **Sample each container:** one **one-level** search per container, client size
     limit `SAMPLE_SIZE = 200`, attributes: `objectClass` plus every attribute the rules
     below read (`uid`, `cn`, `sn`, `givenName`, `displayName`, `gecos`, `mail`,
     `uidNumber`, `gidNumber`, `homeDirectory`, `loginShell`, `memberUid`, `member`,
     `uniqueMember`, `sambaSID`, `description`). Never `userPassword` or other secrets.
     The server's size limit applies **per search**, so one large container cannot
     crowd out another. A truncated sample (client or server limit) is kept and marked
     `partial`.
   - **Attribute presence:** to rank optional attributes for `show`, a second search
     per container with the same scope and limit requests `*` with **types only**
     (`ldap3::SearchOptions::typesonly(true)`, ldap3 0.12): attribute names without
     values, so no binary payload (`jpegPhoto`) and no secret values are transferred.
   - **Cross-container lookups:** for rules that relate entries in different
     containers (private groups, `member` targets), targeted searches for exactly the
     sampled keys, e.g. `(&(objectClass=posixGroup)(|(cn=sw)(cn=hwang)…))` under
     `base_dn`, batched at 50 keys per filter.
   - **Limits:** at most `MAX_CONTAINERS = 100` containers are sampled (in server order;
     the rest are listed in a note), and the whole sampling step has a budget of
     `DETECT_BUDGET = 10 s`. On budget exhaustion detection continues with what it has,
     marked `partial`.
   - **No number scan at startup.** Rule C needs every value of `uidNumber`/`gidNumber`,
     which is a full-directory read. It runs at create time instead, inside number
     allocation (§2C); `edaptor profiles` runs it eagerly so the dump shows the range.
2. **Detect** (`src/detect/infer.rs` + `src/detect/patterns/*.rs`, pure): input is the
   `SchemaModel` and the `Sample`; output is `Vec<DetectedProfile>`. Every detected value
   carries `Evidence { matched: usize, sampled: usize, exceptions: Vec<Dn>, note:
   Option<String> }`. No LDAP, no UI — testable with fixture data.
3. **Merge** (`src/detect/merge.rs`, pure): detected profiles + config `[[profile]]`
   blocks → `Vec<EntryProfile>` (the existing type) plus a `Provenance` map used only by
   the dump. Everything downstream (forms, resolver, pickers, create, companion) keeps
   consuming `EntryProfile`.
4. **When it runs: synchronously during startup, before anything is derived from the
   profiles.** In the TUI that is inside `bootstrap` (`src/ui/state.rs`), after the
   worker is spawned and before `resolve_widgets`, `label_rules`,
   `structure_scan_attrs` and the Samba `samba_needed` check. All four are then computed
   from the merged profiles once, so nothing goes stale, and the tree scan fetches the
   attributes the detected labels need. `bootstrap` is already blocking, so there is no
   new async state and no second connection. The same path serves `edaptor profiles`,
   `edaptor tui-create` and `edaptor passwd` (which resolves a bare user name through
   the profiles' `search_base`), so errors print on the terminal before any screen
   takeover. Startup cost is bounded by the limits in step 1.

## 2. Detection rules

### A. Grouping (any directory)

Within each container sample, entries are grouped by their **structural object class**,
taken from the schema (`ldap_types` object-class kind `STRUCTURAL`; when an entry lists
several structural classes along one SUP chain, the most specific one). Each group with
at least one entry becomes a `DetectedProfile`:

| Field | Rule |
|---|---|
| `object_classes` | classes present in **> half** the group; entries lacking one are exceptions |
| `search_base` | the container DN |
| `rdn_attr` | the most common RDN attribute in the group |
| `show` | `rdn_attr`, then MUST attributes, then MAY attributes present in > half the group ordered by frequency; operational (`NO-USER-MODIFICATION`) and binary-syntax attributes excluded |
| `search_attrs` | `rdn_attr` plus those of `cn`, `uid`, `sn`, `mail`, `description` present in > half the group |
| `label` | `{cn} ({uid})` when both are present and differ in > half the group, else `{<rdn_attr>}` |
| `name` | `<base>-<container RDN value>`, where `<base>` is the pattern's name (`user`, `posixgroup`, `group`) or the structural class lowercased: `user-people`, `posixgroup-groups`, demo `user-users` + `user-people`. Always suffixed, so names are stable. |

Groups below the 3-entry threshold still become profiles (a create template from one
example is useful). Pattern **guards and names** apply regardless of size (a 2-entry
`posixAccount` group is still `user-…`, and its B5 widgets still apply), but no
**value-inferring** rule (B1–B3, C) runs on fewer than 3 entries.

**Container scope.** A detected profile applies to **its own container only**
(DN-equal), not to ancestors or descendants as `profiles_for_container` does for config
profiles today. Otherwise a profile detected at `base_dn` (e.g. `organizationalunit-…`,
`sambadomain-…` on the demo) would be offered on every New anywhere in the tree. Config
profiles keep today's boundary match.

**Order.** Several lookups take the *first* matching profile (`profile_for`,
`label_rules`, the `_posix_group_`/`_any_` sentinels in `resolver.rs`). The merged list
is ordered: config-only profiles in file order, then matched and detected profiles by
**number of object classes, descending**, then by name. So `user-people`
(with `sambaSamAccount`) is tried before `user-users` (without) for an entry that has it.

### B. Known patterns

Each pattern has a guard (the classes it needs) and adds values with evidence.

1. **Templated and fixed defaults** (any profile). For each attribute, test a fixed list
   of candidate templates against the sample; a template matched by the majority becomes
   `[profile.defaults]`: `uid = "{cn}"`, `cn = "{uid}"`, `cn = "{givenName} {sn}"`,
   `displayName = "{givenName} {sn}"`, `gecos = "{givenName} {sn}"`,
   `homeDirectory = "/home/{uid}"` (also any fixed prefix `P` with `P{uid}`). An attribute
   with no template but one value shared by the majority (e.g. `loginShell = /bin/bash`)
   becomes a literal default. Attributes that are unique per entry (`uidNumber`, `mail`,
   `sambaSID`, …) are never literal defaults.
   Templates must not form a cycle: when both `uid = "{cn}"` and `cn = "{uid}"` hold
   (argus, where `uid` equals `cn`), keep only the one whose **source** is the profile's
   `rdn_attr` (argus: `rdn_attr = "cn"` → `uid = "{cn}"`). The same rule applies to any
   pair of templates that feed each other.
2. **User-private group** (guard: `posixAccount`). For the sampled users, look up
   `posixGroup` entries with `cn = <uid>`. If the majority have one whose
   `gidNumber = <user gidNumber> = <user uidNumber>`: add `gidNumber = "{uidNumber}"` and
   a companion `{ object_classes = ["posixGroup"], rdn_attr = "cn", search_base = <the
   container most private groups are in>, attributes = { cn = "{uid}", gidNumber =
   "{uidNumber}" } }`, plus `memberUid = "{uid}"` if the majority of those groups
   contain it.
3. **Shared primary group** (guard: `posixAccount`, rule 2 did not apply). If the
   majority share one `gidNumber`, it becomes a literal default.
4. **Samba** (guard: `sambaSamAccount`). Add `sambaSID = "{auto:sambaSID}"` and
   `[widget.userPassword] kind = "password", samba = true`. The Samba-domain lookup at
   startup must run when a **merged** profile carries either — this also fixes the
   existing gap where the built-in `sambaSID` widget alone never triggers the lookup.
5. **Picker targets.** "The posix-user profile" is the detected `posixAccount` profile
   with the most sampled entries (ties: name order); same for "the posix-group profile".
   `posixGroup.memberUid` → picker over the posix-user profile,
   `store = "uid"`. `groupOfNames.member` / `groupOfUniqueNames.uniqueMember` → picker
   over the profile whose container holds the majority of sampled member DNs.
   `posixAccount.gidNumber` → `lookup` over the posix-group profile, `store =
   "gidNumber"`, `label = "{cn}"`.

### C. Number ranges

Rule C runs **at create time, inside number allocation**, not at startup (it needs every
value). A detected profile gets the default `uidNumber`/`gidNumber` =
`DefaultValue::DetectedRange { space }` instead of a fixed `{next:MIN-MAX}`. When a
create form opens, the allocation step scans the space's attributes (DN, `objectClass`,
`cn`, `uidNumber`, `gidNumber`) under `base_dn`, computes the blocks with the pure rule
below, and allocates `max(in use in block) + 1`. A truncated scan refuses exactly as
today. `edaptor profiles` runs the same scan and prints the resulting `{next:MIN-MAX}`.
A `{next:…}` in the config replaces the detected range as usual. (The allocation search
itself is not paged today, `src/workflows/alloc_flow.rs:57-64`; this spec does not
change that.)

For each profile carrying `uidNumber` (posix users) or `gidNumber` (posix groups that
are **not** private groups):

- **Number space.** When rule B2 applied, `uidNumber` and all `gidNumber` values form
  **one** space (they must not collide); otherwise each attribute is its own space, and
  a `posixAccount`'s `gidNumber` (a reference, not an allocation) is not counted.
- **Private and shared groups in one profile.** On argus both live in `ou=groups` with
  the same structural class, so they form one `posixgroup-groups` profile. That profile
  is used to create **shared** groups (private groups come from the companion), so its
  B1 defaults and its rule-C block are computed from the **non-private** groups only; the
  private groups still count as used numbers in the space.
- **Blocks.** Sort all values of the space; split into blocks where two neighbours are
  more than 1000 apart.
- **This profile's block** is the one holding the majority of its values. Values of the
  profile outside it are exceptions (argus: `staff` at 5001 for the shared groups).
- `MIN` = the block's lowest value rounded down to a multiple of 1000.
- `MAX` = one below the `MIN` of the next higher block in the space; if there is none,
  `max(60000, MIN + 9999)` (so blocks at or above 60000 — `nobody` = 65534, idmap or
  AD-synced ranges — never yield `MIN > MAX`).
- Emit `"{next:MIN-MAX}"`. If `max(in use) + 1 > MAX`, emit it anyway with the note
  `pool exhausted` (allocation will then refuse with its existing message).

Argus result: users `{next:5000-7999}`, shared groups `{next:8000-60000}`.

## 3. Merge, suppression, dump

### Override type

A `[[profile]]` block is parsed into a new `ProfileOverride` type in which **every
field is optional** (`Option<…>`), so "not set" differs from "empty" and a block with
only `name` + `enabled`/`suppress` parses. Today `object_classes` is required
(`src/config/mod.rs:195`) and the others default to empty. After the merge the result
converts to `EntryProfile`. An **unmatched** override becomes a profile only if it has
`object_classes` (as today); otherwise it is dropped with a warning
`profile "<name>" matches no detected profile`.

### Matching

A config `[[profile]]` matches a detected profile when the **names** are equal
(case-insensitive), **or** both `search_base` (DN-equal) and the structural class are
equal. The second rule keeps existing configs from producing duplicates (demo config
`user` ≙ detected `user-people`). The merged profile keeps the **config's name**.
An unmatched config profile is added unchanged. An unmatched detected profile is added
as detected.

**Renames.** When a match renames a detected profile, every detected reference to the
old name (B2 companion, B5 picker/lookup `candidate`) is rewritten through a rename map
during the merge. Profile-name comparison becomes **case-insensitive everywhere**,
including candidate resolution (today exact: `src/config/widget.rs:90`,
`src/config/resolver.rs:194`).

**Validation.** The merged profiles are validated as today, but per origin: a **detected**
part that fails (unknown candidate, `{next:…}` with `MIN > MAX`, companion without its
RDN attribute) is dropped and noted in the dump and status line; only a failing
**config** part is a load error. `bootstrap`'s `widget config error` therefore can no
longer be caused by detection.

### Merge rules

| Key | Rule |
|---|---|
| `object_classes`, `rdn_attr`, `search_base`, `show`, `search_attrs`, `label` | config value replaces detected value |
| `defaults`, `widget` | merged **per attribute**; config entry replaces the detected one for that attribute |
| `companion` | config companion replaces the detected one as a whole |

### New config keys

```toml
[detect]
enabled = true            # default; false = today's behaviour, no sampling at all

[[profile]]
name     = "user-people"
enabled  = false          # drop this (detected or matched) profile entirely
suppress = ["companion", "defaults.loginShell", "widget.gidNumber"]
```

`suppress` paths: `companion`, `defaults.<attr>`, `widget.<attr>`, and the scalar keys
(`label`, `show`, `search_attrs`). An unknown path, or a path naming something detection
did not produce, is a **warning** at startup and in the dump — never a load error.

The existing load-time validation (e.g. no `{next:…}` in a companion) runs on the
**merged** profiles.

### `edaptor profiles`

Prints the merged profiles as **valid TOML**, each value followed by a provenance
comment; suppressed parts appear commented out:

```toml
# detection: 4 containers sampled (up to 200 entries each); notes: none
[[profile]]
name        = "user-people"              # detected: 12 entries in ou=people
search_base = "ou=people,dc=cloud,dc=argus-space,dc=ch"
rdn_attr    = "cn"                       # detected: 12/12
[profile.defaults]
uid         = "{cn}"                     # detected: 12/12
gidNumber   = "{uidNumber}"              # detected: 12/12 have a private group
uidNumber   = "{next:5000-7999}"         # detected at dump time: in use 5000-5016; next block at 8000
loginShell  = "/bin/sh"                  # config (detected "/bin/bash", 11/12)
# exceptions: cn=legacy,ou=people,… loginShell=/bin/tcsh
# suppressed by config: widget.gidNumber
```

Flags: `--detected-only` (before merge). Output goes to stdout; warnings and notes to
stderr so the TOML stays pasteable.

## 4. Errors

Detection never stops eDAPtor.

| Situation | Behaviour |
|---|---|
| `hasSubordinates` filter rejected | objectClass fallback; note in dump |
| a container sample truncated | use it; `# partial` note on that profile |
| a cross-container lookup fails (ACL, timeout) | skip the dependent rule; note says why |
| number-block scan truncated | emit the range, marked `uncertain` |
| sampling exceeds `DETECT_BUDGET` or `MAX_CONTAINERS` | continue with what was sampled; `partial` note |
| a detected part fails validation | drop that part; note in dump + status line |
| a config block matches nothing and has no `object_classes` | warning, block dropped |
| detection fails as a whole | config profiles only; status line `Profile detection failed: <reason>`; `edaptor profiles` prints the reason and exits non-zero |
| anonymous or ACL-restricted view | detect from what is visible; nothing visible = no detected profiles, not an error |
| bad `suppress` path / nothing to suppress | warning, not an error |

## 5. Testing

1. **Unit tests** on `detect::infer` with fixture `(SchemaModel, Sample)` pairs built in
   Rust: argus-like (`cn` RDN, `uid = cn`, gid = uid private groups, `staff` exception);
   demo-like (users in two containers, `groupOfNames` + `posixGroup`); untidy (10/12
   follow a rule → applied with 2 exceptions; 1/2 → not applied, below threshold);
   shared primary group (all gid 100); range blocks incl. an exhausted pool and a
   neighbouring block.
2. **Merge tests:** match by name; match by `search_base` + structural class; per-
   attribute `defaults`/`widget` merge; companion replace; `suppress` each path kind;
   `enabled = false`; `[detect] enabled = false` yields exactly the config profiles.
3. **Live test** against the podman demo server (`tests/live_profile_detection.rs`,
   same harness as the existing `tests/live_*.rs`): `--detected-only` yields the expected
   profiles; with `examples/demo-config.toml` there are no duplicates.
4. **Golden file** of `edaptor profiles` on the demo server, so any dump-format change
   shows in review.
5. **Samba lookup:** a profile with only the detected `sambaSID` default triggers the
   domain lookup.
6. **Pushback regressions:** override block with only `name` + `enabled = false`
   parses; a rename rewrites detected picker candidates (no `widget config error`); a
   detected profile at `base_dn` is not offered in `ou=people`; merged order puts
   `user-people` before `user-users`; a block starting at 65534 yields a valid range;
   `edaptor passwd <uid>` resolves with a connection-only config; case-insensitive
   candidate names.

## 6. Documentation

- New mdBook page `docs/src/configuration/detection.md` (added to `SUMMARY.md`): what is
  detected, the threshold, merge rules, `suppress`, `[detect]`, `edaptor profiles`.
- `overview.md`: the minimal config is connection settings only; profiles are optional
  overrides.
- `README.md` skeleton example shortened accordingly.
- `examples/config.toml` + `full-example.md`: `[detect]` and `suppress` shown, kept
  identical.
- `CHANGES.md`: new feature, plus a note that detection is on for existing configs,
  that it **adds** detected defaults and a detected companion to hand-written profiles
  (so a create may write a second entry), how to see it (`edaptor profiles`), how to
  remove one part (`suppress`), and that `[detect] enabled = false` restores the old
  behaviour.
