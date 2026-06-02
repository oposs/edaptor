# edaptor — Rich User Templates (Design Spec)

**Date:** 2026-06-02
**Status:** Approved design (pre-implementation)
**Milestone:** Rich user templates (the "next milestone" in `docs/HANDOVER.md`)
**Supersedes:** the aspirational profile shape in `docs/superpowers/specs/2026-05-29-edaptor-design.md` §5 (this spec is the authoritative profile format going forward)

---

## 1. Problem

`EntryProfile.object_class` is a **single `String`**, so a created "user" only gets
`["top", inetOrgPerson]` — not a real `posixAccount`/`shadowAccount`. Consequently:

- `uidNumber`/`gidNumber`/`homeDirectory`/`loginShell` never appear in the create form (they belong to `posixAccount`/`shadowAccount`, which aren't on the entry).
- There is no way to **default** a field (e.g. `loginShell = /bin/bash`, `homeDirectory = /home/<uid>`).
- There is no way to **auto-allocate** an identifier (`uidNumber`).
- There is no way to **set a password** from the create form.
- There is no way to **look up** a value from another entry (e.g. pick a primary group and fill its `gidNumber`).

So no profile can express a usable user template today. This milestone makes profiles
rich enough to onboard a real posix (and optionally Samba) user in one create flow.

## 2. Scope

In scope (all confirmed with the maintainer):

0. **Unify create into pane 3 (foundational, built first).** Today NEW renders the
   shared `EditForm` in a modal `Overlay::CreateForm` while editing renders the *same*
   widget inline in pane 3. This collapses the two hosts into one: NEW becomes a pane-3
   **create-mode** form (Save → Add). Everything below then wires into a single host.
1. **Multi-objectClass profiles** — `object_class: String` → `object_classes: Vec<String>`.
2. **Unified defaults** — one `[profile.defaults]` block whose values may be literals, `{attr}` templates, or `{next:MIN-MAX}` autonumber functions.
3. **Inline password field** — a masked, confirm-twice field on the create/edit form; cleartext over TLS to the configured attribute; optional `sambaNTPassword`.
4. **uidNumber auto-allocation** — directory-scan `{next:MIN-MAX}` allocator (also usable for a group profile's `gidNumber`).
5. **Value-lookup picker** — a field may pick another entry and pull one of its attribute values into itself (the `gidNumber`-from-group case).
6. **Profile chooser at create** — F7 lets the operator choose *which* template to create (today it is hardcoded to the first profile), filtered by the current tree container.

Explicitly **out of scope** (recorded for follow-up):

- **Creating a matching private-group entry per user (true UPG).** The maintainer chose
  "fixed default" for a user's `gidNumber` (a literal pointing at an existing primary
  group), so no multi-entry create / fan-out is needed. The create flow stays
  **single-entry**. True UPG (allocate gid → ADD posixGroup → ADD user) is a future
  milestone.
- **A standalone "Set password" action on existing entries** outside the form. Password
  is set through the inline form field (on both create **and** edit) — there is no separate
  keybinding/menu action. M5's `edaptor passwd <dn>` CLI remains for headless use.

## 3. Decisions (and why)

| # | Decision | Rationale |
|---|---|---|
| D1 | `object_classes: Vec<String>` replaces `object_class`. **Breaking config change**, no back-compat alias. | Pre-1.0; a string-or-list alias adds parsing complexity for no real user base. Fail loudly with a clear parse error; update README + all fixtures. |
| D2 | Keep existing field names `rdn_attr` / `search_base` / `search_attrs`. | Spec §5's `container_dn`/`rdn_attribute`/`container` names were never implemented; only `object_class` actually changes. Minimise churn. |
| D3 | **One unified `[profile.defaults]`** with three value kinds (literal / template / autonumber). | Maintainer's choice; one mechanism, one place to read a profile's "fill-ins". |
| D4 | Defaults **only fill empty fields**; never overwrite operator input. | Predictable: the operator always wins. |
| D5 | A user's `gidNumber` is a **fixed default** (literal). Autonumber is reserved for a user's `uidNumber` and a group profile's `gidNumber`. | Maintainer's choice; avoids a dangling gid that points at no posixGroup. |
| D6 | Autonumber scans with **`size_limit: None`** and **refuses to allocate on a truncated/limited result**. | A `max()` over a partial scan would silently re-allocate an existing number (worse than a constraint violation). Fail closed. **Caveat:** `size_limit: None` drops only the *client* cap; slapd's own `sizelimit` (~500 default) still truncates unless bound as rootdn/high-limit — so on large directories auto-alloc effectively needs an admin bind. Paged scan / counter entry is the real fix (follow-up). |
| D7 | Password is sent **cleartext over TLS** to `ldap_attribute`; slapd hashes + enforces ppolicy. Masked in the LDIF preview. | Per design §5; the server is the single source of hashing/policy truth. Never render the cleartext. |
| D8 | The schema-generated `userPassword` field is **suppressed** when `[profile.password]` is declared; the synthetic field replaces it. | `userPassword` is a MAY on the person classes, so the generator already emits it — avoid a duplicate. |
| D9 | Value-lookup picker **reuses** the membership picker infrastructure (single-select variant). | `PickerState`, `build_member_filter`, `service_picker_search`, the search intercept, `candidate_label` already exist; don't fork. |
| D10 | F7 opens a **context-filtered profile chooser**. Filter = profiles whose `search_base` matches the current container at a DN-component boundary; 0 matches → offer all; exactly 1 → create directly (no chooser); >1 → chooser overlay. Placement still comes from the chosen profile's `search_base`. | Multi-template is the point of this milestone; F7→`NewEntry(0)` can only ever reach `profile[0]`. The `NewEntry(i)` plumbing already takes an index — only the selection UI is missing. Context filter keeps the list short and relevant; the all-profiles fallback guarantees F7 always works. |
| D11 | **Create is hosted in pane 3, not a modal.** `Overlay::CreateForm` is removed; `app.form` carries a `FormMode { Edit, Create{ profile_idx, container } }`. `FormSave` branches on the mode (Create → Add path; Edit → today's diff→Modify). A late base-read must not clobber an unsaved create form. | The popup/inline split is the inconsistency the maintainer flagged; the widget is already shared (`app.rs:77`). One host means the password field, defaults, lookup picker, and dirty-guard each wire in **once** instead of twice. |

## 4. Config format (authoritative)

```toml
[[profile]]
name           = "user"
object_classes = ["inetOrgPerson", "posixAccount", "shadowAccount"]
rdn_attr       = "uid"
search_base    = "ou=people,dc=example,dc=org"
show           = ["uid","cn","sn","givenName","mail",
                  "uidNumber","gidNumber","homeDirectory","loginShell"]

# Unified defaults: literal | template ({attr}) | autonumber ({next:MIN-MAX}).
[profile.defaults]
loginShell    = "/bin/bash"           # literal
homeDirectory = "/home/{uid}"         # template — interpolates the uid field
gecos         = "{cn}"                # template
gidNumber     = "10000"               # fixed primary-group gid (literal)
uidNumber     = "{next:10000-60000}"  # autonumber (directory scan, max+1 in range)

# Inline password field.
[profile.password]
ldap_attribute = "userPassword"       # default "userPassword"; cleartext over TLS
samba          = true                 # if entry is sambaSamAccount, also set sambaNTPassword

# Value-lookup picker: pick a posixGroup, fill its gidNumber into this field.
[profile.lookup.gidNumber]
object_class = "posixGroup"
search_base  = "ou=groups,dc=example,dc=org"
value_attr   = "gidNumber"            # attribute pulled from the selected entry
label        = "{cn} ({gidNumber})"   # candidate display
search_attrs = ["cn"]
```

**Parse rules**

- `object_classes` MUST be a non-empty list (a single-string value is a parse error).
- `[profile.defaults]`, `[profile.password]`, `[profile.lookup.*]` are all optional.
- An autonumber value `{next:MIN-MAX}` MUST be the entire string and `MIN <= MAX` (else a parse error). A `{next:…}` for a field that is not single-valued/numeric is the operator's responsibility (no schema cross-check at parse time).
- Template `{attr}` references are resolved at commit; an unresolved reference (empty source field) leaves the target field empty (the default simply doesn't apply).

## 5. Architecture

### 5.0 Create-host unification (foundational — built first)

Today the *same* `EditForm` widget has two hosts: pane 3 (`app.form`, editing an existing
entry) and the modal `Overlay::CreateForm` (NEW). This phase removes the modal and makes
NEW a pane-3 **create-mode** form. It is a pure refactor — **no new template features** —
landed and verified before any of §5.1–§5.4 is wired in, so the rest of the milestone
targets one host.

- **Mode on the form.** `app.form` gains a `mode: FormMode`:
  `Edit` (today's behaviour, has a `baseline` for the diff) or
  `Create { profile_idx, container }` (empty baseline; `dn` is composed from the RDN field at save).
  `Overlay::CreateForm`, `create_form_key`, and `render_create_form` are deleted; their
  logic moves into the pane-3 form key handler / `render_form` / `FormSave`.
- **Open.** `NewEntry(i)` builds the create-mode form (via `empty_form_for_profile` +
  `build_edit_form`) and installs it as `app.form` (focus 0). Pane 3 titles it "New <profile>".
  The tree selection is left where it was; the unsaved entry has no node yet.
- **Save.** `FormSave` (`app.rs:868`) branches on `mode`:
  `Edit` → today's `prepare_save`/`combined_save_overlay` → Modify;
  `Create` → the create pipeline in §5.2 → `Confirm{ PendingAction::Create }` → Add.
- **Clobber guard.** A late base-read installs into `app.form` only when
  `current && app.overlay.is_none()` (`app.rs:506`). Add `&& !app.form.is_new()` so an
  in-flight base-read from the prior selection cannot overwrite an unsaved create form.
- **Cancel & dirty-guard.** Esc/F3 on a create-mode form discards it (`app.form = None`),
  returning to the selected node's read view. The existing `GuardIntent`/`ResolveGuard`
  dirty-guard now also protects create (navigating away from an unsaved/edited new entry
  prompts save/discard/cancel) — for free, because it keys off `app.form` being dirty.
- **Splice.** On Add success, the existing `PostWrite::Created` path splices the node into
  the tree and selects it — unchanged.

Multi-value handling is **unchanged** from today: create still edits one value per field
inline (a second value is added post-create via the pane-3 value-editor popup). Unifying the
host does not by itself lift that limitation; doing so is a follow-up.

### 5.1 New / changed modules

| File | Change |
|---|---|
| `src/config/mod.rs` | `EntryProfile.object_classes: Vec<String>` (was `object_class`); new `defaults: ProfileDefaults`, `password: Option<PasswordSpec>`, `lookups: HashMap<String, LookupSpec>` (TOML key: `[profile.lookup.<attr>]`). |
| `src/config/defaults.rs` (new, pure) | `ProfileDefaults`, `DefaultValue { Literal(String), Template(Vec<Seg>), AutoNumber{min,max} }`, the value parser, `plan_defaults(&ProfileDefaults, &current_values) -> Vec<Resolution>`, and the pure `next_in_range(existing: &[u64], min, max) -> Result<u64>`. |
| `src/config/relation.rs` | `CandidateScope.object_class` / holder scope → `object_classes: Vec<String>`. |
| `src/ui/picker.rs` | `build_member_filter(object_classes: &[String], …)` ANDs the classes: `(&(objectClass=a)(objectClass=b)…)`. `PickerState` gains a single-select / value-pick mode (or a thin sibling) for the lookup picker. |
| `src/workflows/create.rs` | objectClass set `["top", oc1, oc2,…]` (ordered, deduped); `effective_attributes(&all_ocs)`. |
| `src/samba/password.rs` | reuse `build_password_mods` / `nt_hash`; add a small create-time helper that returns **attribute values** (not ModOps) for an Add: `userPassword` (cleartext) and, when samba, `sambaNTPassword` + `sambaPwdLastSet`. |
| `src/ui/edit_form.rs` | `EditForm.mode: FormMode` (§5.0); tag fields: password field (suppress schema `userPassword`, render masked-confirm) and lookup fields; carry `PasswordSpec`/`LookupSpec` onto `EditField`. |
| `src/ui/app.rs` | §5.0 unification (remove `Overlay::CreateForm`/`create_form_key`; `FormSave` mode-branch; clobber guard). The Create branch applies defaults (pure plan + worker autonumber scan) before `build_add_entry`. Password staging (mask in preview, cleartext in Add). Lookup-picker open + selection handler. `allocate_number(worker, base_dn, attr, min, max)` synchronous scan with truncation refusal. F7 → context-filtered profile chooser (`Overlay::ChooseProfile`); pure `profiles_for_container(profiles, container_dn) -> Vec<usize>` matcher. |
| `src/ui/view.rs` | `render_form` titles + renders the create-mode form (replacing `render_create_form`); masked-confirm password field; reuse the picker branch for the lookup picker; render the `ChooseProfile` select overlay. |

### 5.2 Defaults resolution + create-save flow (the heart)

This is the **Create branch of `FormSave`** (§5.0), reached by F2 on a pane-3 create-mode form.
At form open, only **literal** defaults are pre-filled (templates/autonumber reference values
not yet typed).

```
F2 on a Create-mode form  →  create-save (has `worker`)
  1. edited = form.to_edit_entry()
  2. plan_defaults(profile.defaults, &edited)  [PURE]
        → Fill(attr,value)         (literal already applied, or template resolved now)
        → NeedsAutonumber(attr,min,max)
     Apply Fill for any EMPTY target field.  (D4: never overwrite operator input)
  3. For each NeedsAutonumber on an EMPTY field:
        allocate_number(worker, base_dn, attr, min, max)   [WORKER, synchronous]
          - Search base=base_dn scope=subtree filter=(attr=*) attrs=[attr] size_limit=None
          - if truncated/limited → HARD ERROR (D6), abort the create
          - n = max(existing ∩ [min,max]); next = n+1 (or min if none)
          - if next > max → HARD ERROR (pool exhausted)
        Apply to the field.
  4. password (if [profile.password]):
        - take the masked field's confirmed value
        - if non-empty: stage cleartext for ldap_attribute; if samba && objectClasses∋sambaSamAccount,
          also stage sambaNTPassword + sambaPwdLastSet
  5. build_add_entry(profile, …)  →  attrs (multi-OC objectClass set)
  6. LDIF preview (render_add) with password attrs MASKED as ********
  7. Confirm → Request::Add carries the REAL cleartext + hashes
```

On `Add` constraintViolation (e.g. a `unique` overlay rejecting a duplicate `uidNumber`),
surface a clear, retryable error ("uidNumber already taken — retry to re-allocate").
(Automatic single retry is an optional nicety, not required for MVP.)

### 5.3 Value-lookup picker flow

```
Enter on a lookup field (profile.lookup.<attr> declared)
  → open single-select picker; candidates = search(object_class, search_base, search_attrs)
  → candidate_label uses the lookup `label` template
  → Enter on a candidate:
        read candidate.value_attr  (requested in the search attrs)
        set the field's single value to that scalar
        close the picker
  (no DN written; this is the read-only "pull a value" variant of the membership picker)
```

### 5.4 Profile chooser flow (context-filtered)

```
F7 (Leaf pane, writable)
  container = current node's DN (its parent if the node is a leaf)
  matches   = profiles_for_container(profiles, container)   [PURE]
                profile matches iff search_base == container
                OR one is a proper suffix of the other at a DN-component boundary
                (case-insensitive; "ou=people2,…" does NOT match "ou=people,…")
  if matches.is_empty():  matches = all profile indices   (fallback — F7 always works)
  match matches.len():
     0 → (no profiles configured) do nothing
     1 → NewEntry(matches[0])                              (skip the chooser)
    _  → open Overlay::ChooseProfile { indices: matches }
           ↑↓ select · Enter → NewEntry(selected) · Esc cancels
```

`NewEntry(i)` now installs a pane-3 **create-mode** form (§5.0), not the old modal.
Placement is unchanged: it uses `profile[i].search_base` as the container (falling back to
the tree root), exactly as today. The chooser itself is a small transient in-memory select
overlay — **not** the search picker — so it needs no worker round-trip.

## 6. Testing

**Pure unit tests (always on):**
- `defaults.rs` value parser: literal / template (single + embedded + multi-placeholder) / `{next:MIN-MAX}` (valid, `MIN>MAX` rejected, malformed rejected).
- `plan_defaults`: empty-only fill (operator input wins), unresolved template → no-op, autonumber surfaced as `NeedsAutonumber`.
- `next_in_range`: none→min, gap fill (max+1), out-of-range existing ignored, exhaustion (`next>max`) errors.
- `build_member_filter` with multiple object classes (AND form, RFC 4515 escaping unchanged).
- `build_add_entry`: multi-OC objectClass set (ordered, deduped, `top` first).
- password attribute staging: cleartext value present; samba on/off; preview masking.
- `profiles_for_container`: exact match, ancestor/descendant suffix match, component-boundary rejection (`ou=people2`), case-insensitivity, no-match → empty (caller falls back to all).

**Create-host unification (§5.0):**
- `EditForm` mode round-trips: a `Create`-mode form reports `is_new()`, has an empty baseline, and composes its `dn` from the RDN field at save.
- `FormSave` branch selection (Edit→Modify path, Create→Add path) chosen by `mode`.
- the clobber guard: a base-read matching a stale selection does **not** replace an in-flight create form.
- behaviour parity with the old overlay: the existing create tests (DN composition, MUST fields, RDN supply) pass against the pane-3 path.

**Gated live tests** (`tests/live_templates.rs`, `EDAPTOR_TEST_LDAP_URI`, base `dc=example,dc=org`):
- autonumber allocates the next free uidNumber; a seeded gap is filled correctly.
- truncation refusal: with a tight server size limit (or simulated), allocation refuses rather than duplicating.
- multi-OC create: a posixAccount/shadowAccount user is created with all MUST attrs (defaults + autonumber supply them) and passes server validation.
- password set: `userPassword` is set (bind as the new user succeeds) and, with `samba=true`, `sambaNTPassword` matches the `nt_hash` golden.
- lookup picker (logic-level where UI can't be driven): selecting a group yields its `gidNumber`.

## 7. Conventions

- Strict TDD; atomic commits; crate compiles after every commit; `cargo fmt` before commit.
- Facade boundary: only `src/ui/*` may `use ratatui`/`use tui_*`.
- Live tests gated by `EDAPTOR_TEST_LDAP_URI` (skip when unset).
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Subagent-driven execution. **`app.rs`-heavy tasks are the context risk** — the §5.0
  create-host unification (the structural refactor) and the lookup picker most of all;
  scope each tightly or resolve in-session. The pure `defaults.rs`/`picker.rs`/`create.rs`
  work fans out cleanly.

**Build order (the §5.0 unification gates everything):**
1. §5.0 create-host unification — pure refactor, lands green, parity tests pass.
2. `object_classes` list (config + `relation`/`picker`/`create` blast radius).
3. `defaults.rs` pure engine (parser, `plan_defaults`, `next_in_range`).
4. Wire defaults + autonumber scan into the create-save branch.
5. Inline password field (create + edit), masking, samba reuse.
6. Value-lookup picker.
7. Context-filtered profile chooser.

## 8. Open items / follow-ups (recorded, not in this milestone)

1. **gidNumber lookup is IN scope** (§5.3) — but a broader "value lookup from any entry" UX (multiple lookup fields, recents, validation that `value_attr` exists) can grow later.
1b. **Profile chooser is IN scope** (§5.4) — context-filtered. A richer chooser (descriptions, per-profile icons/keys, remembering the last choice) can grow later.
2. **True user-private-groups** (create a posixGroup per user) — deferred; needs a multi-entry create with partial-failure handling like the membership fan-out.
3. **Standalone "Set password" action** on existing entries (outside the form) — deferred; CLI `edaptor passwd` covers headless.
4. **Automatic retry on `uidNumber` constraint violation** — optional hardening.
