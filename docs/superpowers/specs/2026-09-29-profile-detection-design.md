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
tree scan, changing how forms, pickers, create or companions work once a profile exists.

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
- Detection is **on by default**, also for existing configs. `[detect] enabled = false`
  restores today's behaviour exactly.
- Number ranges detect both `MIN` and `MAX` (rule C below).

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
   - **Number blocks:** for rule C, one search per numeric attribute
     (`(uidNumber=*)`, `(gidNumber=*)`) fetching that attribute only, paged. This is the
     same scan number allocation already does at create time; if it is truncated, the
     range is marked `uncertain` (allocation still refuses on its own truncated scan).
2. **Detect** (`src/detect/infer.rs` + `src/detect/patterns/*.rs`, pure): input is the
   `SchemaModel` and the `Sample`; output is `Vec<DetectedProfile>`. Every detected value
   carries `Evidence { matched: usize, sampled: usize, exceptions: Vec<Dn>, note:
   Option<String> }`. No LDAP, no UI — testable with fixture data.
3. **Merge** (`src/detect/merge.rs`, pure): detected profiles + config `[[profile]]`
   blocks → `Vec<EntryProfile>` (the existing type) plus a `Provenance` map used only by
   the dump. Everything downstream (forms, resolver, pickers, create, companion) is
   unchanged.
4. **When it runs:**
   - **TUI:** after bind and schema load, on the worker, in parallel with the tree scan.
     Until it finishes, `New` reports `Detecting profiles…` in the status line and
     pickers fall back as they do today without a profile. When it lands, the merged
     profiles replace `st.profiles`.
   - **`edaptor profiles`, `edaptor tui-create`:** synchronous, before any screen
     takeover, so errors print on the terminal (same rule `tui-create` follows today).

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
| `name` | the pattern's name (`user`, `posixgroup`, `group`) or the structural class lowercased; if two containers yield the same name, **all** of them get `-<container RDN value>` appended (demo: `user-users`, `user-people`) |

Groups below the 3-entry threshold still become profiles (a create template from one
example is useful) but no pattern rule (B/C) is applied to them.

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
5. **Picker targets.** `posixGroup.memberUid` → picker over the posix-user profile,
   `store = "uid"`. `groupOfNames.member` / `groupOfUniqueNames.uniqueMember` → picker
   over the profile whose container holds the majority of sampled member DNs.
   `posixAccount.gidNumber` → `lookup` over the posix-group profile, `store =
   "gidNumber"`, `label = "{cn}"`.

### C. Number ranges

For each profile carrying `uidNumber` (posix users) or `gidNumber` (posix groups that
are **not** private groups):

- **Number space.** When rule B2 applied, `uidNumber` and all `gidNumber` values form
  **one** space (they must not collide); otherwise each attribute is its own space.
  Private groups are excluded from the shared-group profile's values.
- **Blocks.** Sort all values of the space; split into blocks where two neighbours are
  more than 1000 apart.
- **This profile's block** is the one holding the majority of its values. Values of the
  profile outside it are exceptions (argus: `staff` at 5001 for the shared groups).
- `MIN` = the block's lowest value rounded down to a multiple of 1000.
- `MAX` = one below the `MIN` of the next higher block in the space, else `60000`.
- Emit `"{next:MIN-MAX}"`. If `max(in use) + 1 > MAX`, emit it anyway with the note
  `pool exhausted` (allocation will then refuse with its existing message).

Argus result: users `{next:5000-7999}`, shared groups `{next:8000-60000}`.

## 3. Merge, suppression, dump

### Matching

A config `[[profile]]` matches a detected profile when the **names** are equal
(case-insensitive), **or** both `search_base` (DN-boundary equal) and the structural
class are equal. The second rule keeps existing configs from producing duplicates (demo
config `user` ≙ detected `user-people`). The merged profile keeps the **config's name**.
An unmatched config profile is added unchanged. An unmatched detected profile is added
as detected.

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
name     = "user"
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
name        = "user"                     # detected: 12 entries in ou=people
search_base = "ou=people,dc=cloud,dc=argus-space,dc=ch"
rdn_attr    = "cn"                       # detected: 12/12
[profile.defaults]
uid         = "{cn}"                     # detected: 12/12
gidNumber   = "{uidNumber}"              # detected: 12/12 have a private group
uidNumber   = "{next:5000-7999}"         # detected: in use 5000-5016; next block at 8000
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

## 6. Documentation

- New mdBook page `docs/src/configuration/detection.md` (added to `SUMMARY.md`): what is
  detected, the threshold, merge rules, `suppress`, `[detect]`, `edaptor profiles`.
- `overview.md`: the minimal config is connection settings only; profiles are optional
  overrides.
- `README.md` skeleton example shortened accordingly.
- `examples/config.toml` + `full-example.md`: `[detect]` and `suppress` shown, kept
  identical.
- `CHANGES.md`: new feature, plus a note that detection is on for existing configs and
  `[detect] enabled = false` restores the old behaviour.
