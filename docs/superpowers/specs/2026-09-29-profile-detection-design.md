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
tree scan, changing how forms, pickers or companions work once a profile exists.
Consumers that do change, each for a stated reason: startup order and `load_profiles`
(§1.4), `tui-create` resolving by name, `passwd` searching account profiles only, the
profile chooser, the Samba trigger (§2B4), container scope and case-insensitive profile
names (§2A, §3), and number allocation, which learns detected ranges (§2C).

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
- Too little data (no or 1–2 users): assume useradd-style numbering starting at 10000
  and **private groups** (rule D below) — good practice, owner decision 2026-09-29.
- **Detection never stops eDAPtor.** A detected part that fails validation is dropped
  with a note; only the user's own config can cause a load error.

## 1. Architecture and data flow

```
 sample (LDAP)  ──►  detect (pure)  ──►  merge with config (pure)  ──►  Vec<EntryProfile>
 containers +        DetectedProfile      config wins, suppress,         (existing type +
 per-container       + evidence           enabled=false                  a `scope` field)
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
   - **Reverse private-group lookup:** for the sampled `posixGroup` entries, targeted
     searches for `posixAccount` entries with `uid = <group cn>`, batched the same way.
     Together with the forward lookup this classifies every sampled group as private or
     shared, whether or not its user was sampled (§2B2).
   - **Limits:** at most `MAX_CONTAINERS = 100` containers are sampled (in server order;
     the rest are listed in a note), and the whole sampling step has a budget of
     `DETECT_BUDGET = 10 s`. The worker sets no timeouts today, and a request blocks, so
     the budget is enforced **per search**: each sampling search carries a server time
     limit (`ldap3::SearchOptions::timelimit`, whole seconds, at least 1) and a client
     timeout, both set to the budget still left. The container search
     `(hasSubordinates=TRUE)` is unindexed and walks the tree on the server; it gets the
     same limit. A search that hits its limit returns what it has, marked `partial`;
     after the budget is used up, the remaining searches are skipped with a note.
     Sampling needs new worker requests (types-only search, per-search time limit).
   - **Progress:** the blocking startup prints one line `detecting profiles…` on stderr
     before the first sampling search, so a slow server does not look like a hang.
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
   profiles.** One shared function `load_profiles(worker, config) -> LoadedProfiles`
   (schema model, merged profiles, provenance, notes) fetches the subschema, samples,
   detects and merges. Every command that needs profiles calls it:
   - **TUI (`bootstrap`, `src/ui/state.rs`).** The order changes. Today `FetchSubschema`
     runs **after** `resolve_widgets` and the Samba discovery (`state.rs:1676-1702`);
     detection needs the schema first. New order: spawn worker → fetch subschema →
     `load_profiles` → `resolve_widgets`, `label_rules`, `structure_scan_attrs`, Samba
     `samba_needed` check → root DSE probe, structure scan, rest as today. All derived
     state is computed once from the merged profiles, so nothing goes stale, and the
     tree scan fetches the attributes the detected labels need. `bootstrap` is already
     blocking and runs before the screen takeover (`src/ui/mod.rs:84`), so there is no
     new async state and no second connection.
   - **`edaptor tui-create`.** Today `main.rs:103` resolves the profile to a list
     **index** from `config.profiles` before `bootstrap`, and `app.rs:393-397` uses that
     index into `st.profiles`. After the merge the list is reordered and extended, so
     `StartupAction::Create` carries the profile **name** instead, resolved
     (case-insensitively) against the merged profiles after `bootstrap`. An unknown
     name is still reported on the terminal: `bootstrap` returns before the screen
     takeover, so the resolution happens between the two.
   - **`edaptor passwd`.** `run_passwd` (`src/lib.rs:228`) does not use `bootstrap` and
     never fetches the schema; it calls `load_profiles` directly. `username_searches`
     (`src/passwd.rs:21-27`) searches **every** profile with a `search_base`, so a
     detected `posixgroup-groups` would find the user's private group and make the
     result `Ambiguous`. It is restricted to profiles that carry a `password` widget
     (after resolution, including the built-in bundle), i.e. account profiles.
   - **`edaptor profiles`.** Calls `load_profiles`, then runs rule C eagerly (§2C).
   - **`edaptor check`, `edaptor schema`.** Unchanged; they do not detect (see §3
     Validation for what they still check offline).
   Startup cost is bounded by the limits in step 1.

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
| `name` | `<base>-<container RDN value>`, where `<base>` is the pattern's name (`user`, `posixgroup`, `group`) or the structural class lowercased: `user-people`, `posixgroup-groups`, demo `user-users` + `user-people`. Always suffixed, so names are stable. See "Name rules" below. |

**Name rules.** The RDN value is lowercased and every run of characters outside
`[a-z0-9]` becomes one `-` (`ou=IT Staff` → `it-staff`); leading/trailing `-` are
dropped. If two detected profiles still get the same name (`ou=people,o=a` and
`ou=people,o=b` → `user-people`), **each** of them adds the next parent RDN value, then
the next, until the names differ (`user-people-a`, `user-people-b`). Names depend only
on DNs, so they stay stable. Names are compared case-insensitively everywhere.

**Infrastructure classes.** Detected profiles whose structural class is
`organizationalUnit`, `organization`, `domain`, `dcObject`, `sambaDomain` or
`pwdPolicy` are kept (you can still create an OU in its container) but are **hidden
from the profile chooser** (`ChooseThenCreate`, `app.rs:399-405`) unless the current
container is exactly theirs.

Groups below the 3-entry threshold still become profiles (a create template from one
example is useful). Pattern **guards and names** apply regardless of size (a 2-entry
`posixAccount` group is still `user-…`, and its B5 widgets still apply), but no
**value-inferring** rule (B1–B3, C) runs on fewer than 3 entries. The threshold counts
**sampled** entries for B1–B3 and **scanned** entries (the full allocation scan) for C.

**Container scope.** `EntryProfile` gets a new field `scope: ContainerScope` with two
values: `Boundary` (today's `profiles_for_container` rule: equal, ancestor or
descendant) and `Exact` (DN-equal). Config-only and **matched** profiles are
`Boundary`, as today; **detected-only** profiles are `Exact`. Otherwise a profile
detected at `base_dn` (e.g. `organizationalunit-…`, `sambadomain-…` on the demo) would
be offered on every New anywhere in the tree.

**Order.** Several lookups take the *first* matching profile (`profile_for`,
`profile_for_entry_where`, `label_rules`, the `_posix_group_`/`_any_` sentinels in
`resolver.rs`). Today file order is the documented tie-break ("declare the more specific
profile first", `create.rs:205-208`). So the merged list is: **all config profiles,
matched or not, in file order** (a matched one takes its config block's position), then
the **detected-only** profiles sorted by number of object classes, descending, then by
name. So among detected profiles `user-people` (with `sambaSamAccount`) is tried before
`user-users` (without), and an existing config keeps its order.

### B. Known patterns

Each pattern has a guard (the classes it needs) and adds values with evidence.

1. **Templated and fixed defaults** (any profile). For each attribute, test a fixed list
   of candidate templates against the sample; a template matched by the majority becomes
   `[profile.defaults]`: `uid = "{cn}"`, `cn = "{uid}"`, `cn = "{givenName} {sn}"`,
   `displayName = "{givenName} {sn}"`, `gecos = "{givenName} {sn}"`,
   `homeDirectory = "/home/{uid}"` (also any fixed prefix `P` with `P{uid}`). An attribute
   with no template but one value shared by the majority (e.g. `loginShell = /bin/bash`)
   becomes a literal default. Literal defaults are only inferred for **single-valued,
   non-membership** attributes: never `memberUid`, `member`, `uniqueMember`, `memberOf`
   (otherwise "admin is in most groups" would become a default), and never attributes
   unique per entry (`uidNumber`, `mail`, `sambaSID`, …).
   Templates must not form a cycle: when both `uid = "{cn}"` and `cn = "{uid}"` hold
   (argus, where `uid` equals `cn`), keep only the one whose **source** is the profile's
   `rdn_attr` (argus: `rdn_attr = "cn"` → `uid = "{cn}"`). The same rule applies to any
   pair of templates that feed each other.
2. **User-private group** (guard: `posixAccount`). **Private-group predicate:** a
   `posixGroup` G is the private group of a `posixAccount` U when `G.cn = U.uid` and
   `G.gidNumber = U.gidNumber = U.uidNumber`. It is evaluated in both directions
   (forward lookup from sampled users, reverse lookup from sampled groups, §1.1), so
   every sampled group is classified even if its user was not sampled. If the majority
   of sampled users have a private group: add `gidNumber = "{uidNumber}"` and
   a companion `{ object_classes = ["posixGroup"], rdn_attr = "cn", search_base = <the
   container most private groups are in>, attributes = { cn = "{uid}", gidNumber =
   "{uidNumber}" } }`, plus `memberUid = "{uid}"` if the majority of those groups
   contain it.
3. **Shared primary group** (guard: `posixAccount`, rule 2 did not apply). If the
   majority share one `gidNumber`, it becomes a literal default.
   *Not covered:* users with `gidNumber = uidNumber` but **no** private group (demo
   `ou=users`) match neither B2 nor B3 and get no `gidNumber` default. The dump notes
   `gidNumber = uidNumber for N/M, but no private groups found`; a config
   `gidNumber = "{uidNumber}"` covers it.
4. **Samba** (guard: `sambaSamAccount`). Add `sambaSID = "{auto:sambaSID}"`. (No
   `userPassword` widget: the built-in bundle already gives `sambaSamAccount` a Samba
   password widget, `builtin_schema.toml:45`.) The Samba-domain lookup at startup runs
   when **any merged profile's `object_classes` include `sambaSamAccount`**, in
   addition to today's triggers (a `sambaSID` profile widget or an `{auto:sambaSID}`
   default, `state.rs:1691-1694`). This closes the real gap: today the built-in
   `sambaSamAccount.sambaSID` widget (`builtin_schema.toml:55`) alone never triggers the
   lookup, so without `[samba] domain_sid` it degrades to plain text.
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
`DefaultValue::DetectedRange(RangeSpec)` instead of a fixed `{next:MIN-MAX}`:

```rust
pub struct RangeSpec {
    pub attr: String,              // "uidNumber" or "gidNumber" — the attribute allocated
    pub container: String,         // the profile's search_base: which entries are "its" values
    pub structural: String,        // the profile's structural class
    pub unified: bool,             // B2 applied: uidNumber + gidNumber form one space
    pub exclude_private: bool,     // posixGroup profile: private groups are not its values
}
```

**Create-time flow.** `plan_defaults` returns a new `Resolution::NeedsDetectedRange
{ attr, spec }` next to `NeedsAutonumber`; `apply_static_defaults` (`create.rs:244-257`)
and the form-open code (`app.rs:589-597`) pass it on. `AllocFlow` gets a second request
kind: `pending` today holds `(attr, min, max)` and `request` builds `({attr}=*)` with one
attribute (`alloc_flow.rs:48-66`); the new kind holds `(attr, RangeSpec)` and searches
`(|(uidNumber=*)(gidNumber=*))` under `base_dn` (subtree, no size limit as today),
fetching DN, `objectClass`, `cn`, `uid`, `uidNumber`, `gidNumber`. `uid` is needed to
evaluate the private-group predicate (§2B2) where `uid ≠ cn`. On the response, the pure
function `detect::range::allocate(spec, entries) -> Result<(u64, Evidence), String>`
applies the rule below and returns `max(in use in block) + 1` (or `MIN` for an empty
block). A truncated scan refuses exactly as today. The `{auto:sambaSID}` path, which
waits for `uidNumber` via the alloc hook (`state.rs` `apply_alloc_outcome` →
`recompute_computed_defaults`), is unchanged, because the new kind reports through the
same `AllocOutcome::Filled`. `edaptor profiles` runs the same scan and function and
prints the resulting `{next:MIN-MAX}`. A `{next:…}` in the config replaces the detected
range as usual. (The allocation search is not paged today; this spec does not change
that.)

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
- `MIN ≤ MAX` always holds: the next block's lowest value is more than 1000 above this
  block's highest, so its rounded `MIN` is above this block's `MIN`.
- `MAX` = one below the `MIN` of the next higher block in the space; if there is none,
  `max(60000, MIN + 9999)` (so blocks at or above 60000 — `nobody` = 65534, idmap or
  AD-synced ranges — never yield `MIN > MAX`).
- Emit `"{next:MIN-MAX}"`. If `max(in use) + 1 > MAX`, emit it anyway with the note
  `pool exhausted` (allocation will then refuse with its existing message).

Argus result: users `{next:5000-7999}`, shared groups `{next:8000-60000}`.

### D. Assumptions when there is too little data (useradd-style)

Detection can only infer from entries that exist. A new or nearly empty directory
gets the defaults Ubuntu's `useradd` uses (`/etc/login.defs`: `UID_MIN 1000`,
`UID_MAX 60000`, `USERGROUPS_ENAB yes`), with one change: LDAP numbers start at
**10000**, because every client machine hands out 1000 and up to its local users and
an LDAP account must not share a number with them.

These rules run **after the merge**, on every final profile whose `object_classes`
include `posixAccount` — also a profile written only in the config, since an empty
container has no children and yields no detected profile at all. They never replace a
value from the config or from detection; they only fill what is still missing, and
`suppress` removes them like any detected part. Provenance is `assumed`, and the dump
says why (`# assumed: no users yet; useradd-style private group`).

1. **Range.** If the profile has no `uidNumber` default, it gets a `DetectedRange` whose
   allocation uses these rules at create time:
   - **no numbers in use** in the space → `{next:10000-60000}`;
   - **1 or 2 numbers in use** → the block rule of §2C applies unchanged to the values
     that exist (argus-like start at 5000 continues at 5001, not 10000). The 3-entry
     threshold of §2A counts only for **exceptions**, not for forming a block.
   The same applies to a posix-group profile's `gidNumber`.
2. **Private groups.** If B2 found **no contrary evidence** — fewer than 3 users were
   sampled and none of them lacks a private group (§2B2 predicate), or there are no
   users — the profile gets what B2 would add: `gidNumber = "{uidNumber}"` and the
   companion `{ cn = "{uid}", gidNumber = "{uidNumber}", memberUid = "{uid}" }`, and the
   number space is **unified** (§2C). The companion's `search_base` is the posix-group
   profile's `search_base` (§2B5 definition, over the merged profiles); if there is
   none, a container `ou=groups` directly under `base_dn` if it exists; otherwise the
   assumption is skipped with the note `no group container for private groups`.
   Users that **do** contradict (e.g. 2 of 2 share gid 100) block the assumption, and
   B3 applies as usual.
3. `[detect] enabled = false` disables these assumptions too.

## 3. Merge, suppression, dump

### Override type

A `[[profile]]` block is parsed into a new `ProfileOverride` type in which **every
field is optional** (`Option<…>`), so "not set" differs from "empty" and a block with
only `name` + `enabled`/`suppress` parses. Today `object_classes` is required
(`src/config/mod.rs:195`) and the others default to empty. After the merge the result
converts to `EntryProfile`. An **unmatched** override becomes a profile only if it has
`object_classes` (as today); otherwise it is dropped with a warning
`profile "<name>" matches no detected profile`.

With `[detect] enabled = false` the old rules hold exactly: `name` and
`object_classes` are required, and a block without them is a load error, as today
(`src/config/mod.rs:195`). With detection on, `name` stays required.

### Matching

A config `[[profile]]` matches a detected profile when the **names** are equal
(case-insensitive), **or** both `search_base` (DN-equal) and the structural class are
equal. The second rule keeps existing configs from producing duplicates (demo config
`user` ≙ detected `user-people`). The merged profile keeps the **config's name**.
An unmatched config profile is added unchanged. An unmatched detected profile is added
as detected.

**Each detected profile is matched at most once.** Matching runs in two passes: first
all name matches, then `search_base` + structural-class matches among the detected
profiles still free. Within a pass, config blocks are taken in file order. A config
block that would match an already-taken detected profile stays unmatched (and is added
as its own profile if it has `object_classes`), with the warning `profile "<a>" also
matches detected "<d>", already merged into "<b>"`. This covers two config blocks for
the same container and class that differ only in auxiliary classes.

**Renames.** When a match renames a detected profile, every detected reference to the
old name (B2 companion, B5 picker/lookup `candidate`) is rewritten through a rename map
during the merge. Profile-name comparison becomes **case-insensitive everywhere**,
including candidate resolution (today exact: `src/config/widget.rs:90`,
`src/config/resolver.rs:194`).

**Validation.** Two stages:
- **Offline, in `Config::load`** (as today, `src/config/mod.rs:344-381`): everything
  that can be checked on the config alone — companion `rdn_attr` in `attributes`, no
  `{next:…}` in a companion, `{next:MIN-MAX}` syntax. So `edaptor check` and
  `edaptor schema`, which do not detect, still report these errors.
- **After the merge, per origin:** a **detected** part that fails (unknown candidate,
  companion without its RDN attribute) is dropped and noted in the dump and status
  line; only a failing **config** part is a load error. `bootstrap`'s
  `widget config error` therefore can no longer be caused by detection.

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

The offline checks run on the config at load; the merged profiles are validated
again per origin (see Validation above).

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
| allocation scan truncated, at create time | refuse, exactly as today |
| allocation scan truncated, in `edaptor profiles` | print the range, marked `uncertain` |
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
5. **Samba lookup:** a merged profile whose `object_classes` include `sambaSamAccount`,
   with no `sambaSID` widget and no `{auto:sambaSID}` default, triggers the domain
   lookup (the real gap; the `{auto:sambaSID}` trigger already exists).
6. **Pushback regressions:** override block with only `name` + `enabled = false`
   parses; a rename rewrites detected picker candidates (no `widget config error`); a
   detected profile at `base_dn` is not offered in `ou=people`; merged order puts
   `user-people` before `user-users`; a block starting at 65534 yields a valid range;
   `edaptor passwd <uid>` resolves with a connection-only config; case-insensitive
   candidate names.
7. **Round-two regressions:** `tui-create user-people` resolves after the merge (name,
   not index); `passwd sw` on an argus-like fixture is not `Ambiguous` (the group
   profile is not searched); an existing config's order is kept; reverse lookup
   classifies a group whose user was not sampled; `RangeSpec` allocation with
   `uid ≠ cn`; a search hitting its time limit yields `partial`; name collision
   `ou=people,o=a` / `ou=people,o=b`; `ou=IT Staff` → `…-it-staff`; two config blocks
   matching one detected profile; `[detect] enabled = false` + a block without
   `object_classes` is a load error; `edaptor check` still rejects a companion with a
   `{next:…}`; `memberUid` is never a literal default.

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
