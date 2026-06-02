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

1. **Multi-objectClass profiles** — `object_class: String` → `object_classes: Vec<String>`.
2. **Unified defaults** — one `[profile.defaults]` block whose values may be literals, `{attr}` templates, or `{next:MIN-MAX}` autonumber functions.
3. **Inline password field** — a masked, confirm-twice field on the create/edit form; cleartext over TLS to the configured attribute; optional `sambaNTPassword`.
4. **uidNumber auto-allocation** — directory-scan `{next:MIN-MAX}` allocator (also usable for a group profile's `gidNumber`).
5. **Value-lookup picker** — a field may pick another entry and pull one of its attribute values into itself (the `gidNumber`-from-group case).

Explicitly **out of scope** (recorded for follow-up):

- **Creating a matching private-group entry per user (true UPG).** The maintainer chose
  "fixed default" for a user's `gidNumber` (a literal pointing at an existing primary
  group), so no multi-entry create / fan-out is needed. The create flow stays
  **single-entry**. True UPG (allocate gid → ADD posixGroup → ADD user) is a future
  milestone.
- **A standalone "Set password" action on existing entries** outside the form. Password
  is set through the inline form field only (M5's `edaptor passwd <dn>` CLI remains for headless use).

## 3. Decisions (and why)

| # | Decision | Rationale |
|---|---|---|
| D1 | `object_classes: Vec<String>` replaces `object_class`. **Breaking config change**, no back-compat alias. | Pre-1.0; a string-or-list alias adds parsing complexity for no real user base. Fail loudly with a clear parse error; update README + all fixtures. |
| D2 | Keep existing field names `rdn_attr` / `search_base` / `search_attrs`. | Spec §5's `container_dn`/`rdn_attribute`/`container` names were never implemented; only `object_class` actually changes. Minimise churn. |
| D3 | **One unified `[profile.defaults]`** with three value kinds (literal / template / autonumber). | Maintainer's choice; one mechanism, one place to read a profile's "fill-ins". |
| D4 | Defaults **only fill empty fields**; never overwrite operator input. | Predictable: the operator always wins. |
| D5 | A user's `gidNumber` is a **fixed default** (literal). Autonumber is reserved for a user's `uidNumber` and a group profile's `gidNumber`. | Maintainer's choice; avoids a dangling gid that points at no posixGroup. |
| D6 | Autonumber scans with **`size_limit: None`** and **refuses to allocate on a truncated/limited result**. | A `max()` over a partial scan would silently re-allocate an existing number (worse than a constraint violation). Fail closed. |
| D7 | Password is sent **cleartext over TLS** to `ldap_attribute`; slapd hashes + enforces ppolicy. Masked in the LDIF preview. | Per design §5; the server is the single source of hashing/policy truth. Never render the cleartext. |
| D8 | The schema-generated `userPassword` field is **suppressed** when `[profile.password]` is declared; the synthetic field replaces it. | `userPassword` is a MAY on the person classes, so the generator already emits it — avoid a duplicate. |
| D9 | Value-lookup picker **reuses** the membership picker infrastructure (single-select variant). | `PickerState`, `build_member_filter`, `service_picker_search`, the search intercept, `candidate_label` already exist; don't fork. |

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

### 5.1 New / changed modules

| File | Change |
|---|---|
| `src/config/mod.rs` | `EntryProfile.object_classes: Vec<String>` (was `object_class`); new `defaults: ProfileDefaults`, `password: Option<PasswordSpec>`, `lookups: HashMap<String, LookupSpec>` (TOML key: `[profile.lookup.<attr>]`). |
| `src/config/defaults.rs` (new, pure) | `ProfileDefaults`, `DefaultValue { Literal(String), Template(Vec<Seg>), AutoNumber{min,max} }`, the value parser, `plan_defaults(&ProfileDefaults, &current_values) -> Vec<Resolution>`, and the pure `next_in_range(existing: &[u64], min, max) -> Result<u64>`. |
| `src/config/relation.rs` | `CandidateScope.object_class` / holder scope → `object_classes: Vec<String>`. |
| `src/ui/picker.rs` | `build_member_filter(object_classes: &[String], …)` ANDs the classes: `(&(objectClass=a)(objectClass=b)…)`. `PickerState` gains a single-select / value-pick mode (or a thin sibling) for the lookup picker. |
| `src/workflows/create.rs` | objectClass set `["top", oc1, oc2,…]` (ordered, deduped); `effective_attributes(&all_ocs)`. |
| `src/samba/password.rs` | reuse `build_password_mods` / `nt_hash`; add a small create-time helper that returns **attribute values** (not ModOps) for an Add: `userPassword` (cleartext) and, when samba, `sambaNTPassword` + `sambaPwdLastSet`. |
| `src/ui/edit_form.rs` | tag fields: password field (suppress schema `userPassword`, render masked-confirm) and lookup fields; carry `PasswordSpec`/`LookupSpec` onto `EditField`. |
| `src/ui/app.rs` | `commit_create`: apply defaults (pure plan + worker autonumber scan) before `build_add_entry`; thread `worker` in. Password handling on commit (mask in preview, cleartext in Add). Lookup-picker open + selection handler. `allocate_number(worker, base_dn, attr, min, max)` synchronous scan with truncation refusal. |
| `src/ui/view.rs` | render the masked-confirm password field; reuse the picker branch for the lookup picker. |

### 5.2 Defaults resolution flow (the heart)

```
form open (empty_form_for_profile)
  └─ pre-fill LITERAL defaults only (templates/autonumber need values not yet typed)

operator edits fields …

F2 → commit_create (now takes `worker`)
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

## 6. Testing

**Pure unit tests (always on):**
- `defaults.rs` value parser: literal / template (single + embedded + multi-placeholder) / `{next:MIN-MAX}` (valid, `MIN>MAX` rejected, malformed rejected).
- `plan_defaults`: empty-only fill (operator input wins), unresolved template → no-op, autonumber surfaced as `NeedsAutonumber`.
- `next_in_range`: none→min, gap fill (max+1), out-of-range existing ignored, exhaustion (`next>max`) errors.
- `build_member_filter` with multiple object classes (AND form, RFC 4515 escaping unchanged).
- `build_add_entry`: multi-OC objectClass set (ordered, deduped, `top` first).
- password attribute staging: cleartext value present; samba on/off; preview masking.

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
- Subagent-driven execution. **`app.rs`-heavy tasks (commit-time defaults wiring, the lookup picker) are the context risk** — scope each tightly or resolve in-session; the pure `defaults.rs`/`picker.rs`/`create.rs` work fans out cleanly.

## 8. Open items / follow-ups (recorded, not in this milestone)

1. **gidNumber lookup is IN scope** (§5.3) — but a broader "value lookup from any entry" UX (multiple lookup fields, recents, validation that `value_attr` exists) can grow later.
2. **True user-private-groups** (create a posixGroup per user) — deferred; needs a multi-entry create with partial-failure handling like the membership fan-out.
3. **Standalone "Set password" action** on existing entries (outside the form) — deferred; CLI `edaptor passwd` covers headless.
4. **Automatic retry on `uidNumber` constraint violation** — optional hardening.
