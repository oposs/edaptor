# edaptor — Design Specification

**Date:** 2026-05-29
**Status:** Approved design (pre-implementation)
**Working directory:** `/home/oetiker/checkouts/ldapedit` (crate/binary name: `edaptor`)

## 1. Summary

`edaptor` is a terminal UI (TUI) for administering an OpenLDAP directory —
primarily **adding, modifying, and removing users and groups, and managing
group memberships**. It is written in Rust and built on the
[`turbo-vision`](https://crates.io/crates/turbo-vision) crate (a Rust port of
Borland Turbo Vision).

The defining idea: the client **derives the structure of the directory directly
from the LDAP server** via full schema introspection (`cn=subschema`), and
generates its edit forms dynamically from `objectClass` definitions. A config
file holds all connection properties plus a small set of *entry profiles* that
say what "a user" / "a group" mean in this particular directory.

The target server is the `oposs.openldap` Ansible role
(`~/checkouts/oep-ansible/playbooks/config.d/roles/oposs.openldap`), which
provides: OUs `people`/`groups`/`services`/`computers`; `groupOfNames` groups
with a `member` attribute; the **memberOf**, **refint**, and **ppolicy**
overlays; and the Samba schema. The role may be adjusted to better support
`edaptor` where useful.

## 2. Key Design Decisions

| Topic | Decision |
|---|---|
| Name | `edaptor` (crate + binary) |
| UI framework | `turbo-vision` crate, accessed through a thin in-house facade; vendor the crate if upstream stalls |
| Architecture | Layered, with a **background LDAP worker thread** so the UI never blocks on the network |
| Schema | **Full live introspection** of `cn=subschema`; forms generated from `objectClass` definitions |
| Form strategy | One **generic schema-driven form engine**; the guided Users/Groups/Membership tier sits over it (see Object model) — config-informed, adding password/membership/Samba understanding |
| Templates / profiles | Declared **in the config file** (for v1; directory-stored templates may come later) |
| Auth | Config-selectable: `simple` / SASL `external` / `gssapi`. Passwords never stored on disk |
| Commit model | **Immediate apply**, with a keystroke to preview the exact **LDIF** before sending |
| Labels | Human-readable labels (`cn` etc.) shown **everywhere**; raw DNs only on demand |
| Membership | **Symmetric** (group↔user); dual-pane transfer with **incremental search on both sides** |
| Samba | **Full `sambaSamAccount` lifecycle**; NT hash computed client-side; Unix+Samba passwords **synced** by default; domain SID **discovered** from the `sambaDomain` entry (config fallback) |
| Object model | **Two tiers:** a generic schema-driven object engine, with a richer **users & groups understanding layered over it**, informed by config profiles |
| Scope — also in v1 | OU/container management (the rich user create/edit flow is part of the users tier, not a separate wizard) |
| Scope — deferred | Multi-server switching; CSV/bulk import |

## 3. Architecture

Approach: layered engine with all network I/O isolated on a background worker
thread; UI communicates with it via request/response channels.

```
config (TOML) ──┐
                ▼
        ┌──────────────────┐   channel    ┌─────────────────────┐
        │  UI (turbo-vision │ ◄──────────► │  LDAP worker thread │
        │  behind a facade) │  req/resp    │  (ldap3): bind,     │
        └──────────────────┘              │  search, modify,    │
            ▲      ▲                       │  add, modrdn, delete│
            │      │                       └─────────────────────┘
      screens/   form generator ◄── schema model ◄───┘
      workflows  (schema → fields)   (parsed subschema)
```

**Boundaries**

- `ldap::worker` is the *only* component that touches the network. Everyone
  else sends a `Request` and receives a `Response`/event. This keeps the UI
  responsive (spinner + cancel) and the engine testable headless.
- `schema` and `form` are pure and headless — no UI, no network — so they unit
  test against captured fixtures.
- `workflows` never hard-code attribute names; they hand a profile to the
  `form` engine and inject special widgets (membership transfer, password
  action) based on profile metadata and attribute syntax.

### 3.1 Conceptual Model — Two Tiers

`edaptor` understands the directory at two levels, and the higher level sits
*over* the lower one:

1. **Generic object tier (foundation).** Understands *any* entry purely from
   live schema — MUST/MAY, syntaxes, cardinality. It performs single-entry
   edits (one `ADD`/`MODIFY`/`MODRDN`/`DELETE` against one DN). This is what the
   generic browser uses for arbitrary objects.

2. **Users & groups tier (over the foundation), informed by config profiles.**
   This layer adds the understanding a raw object lacks:
   - a **user** has a password — Unix *and* Samba, kept in sync;
   - a **user** belongs to **groups**, whose membership lives on *other*
     entries (the group's `member` attribute);
   - a **user** can be **Samba-enabled** (objectClass + computed `sambaSID`);
   - a user/group has a known container OU, RDN attribute, and display label.

This understanding is **pervasive across the entire lifecycle** — not just
creation. Wherever special knowledge applies, edaptor acts on it naturally, so
the admin never has to think in raw-LDAP terms:

| Operation | Users — acts naturally | Groups — acts naturally |
|---|---|---|
| **View** | Human labels; group memberships (from `memberOf`); password/Samba status — not a raw attribute dump. | Members shown by label with a count; Samba group-mapping status; (owners/managers by label where present). |
| **Create** | One guided sequence: entry in the right OU/RDN + (synced) password + initial groups + optional Samba-enable. (This *is* the "onboarding"; no separate wizard.) | Knows `groupOfNames` needs **≥1 member**, so the create flow collects initial members up front (no illegal empty group); optional Samba group-mapping (`sambaGroupMapping` + SID from `gidNumber`). |
| **Edit** | Password is a proper action (Unix+Samba synced, ppolicy, TLS-only); membership edits write the *group* entries; RDN change → MODRDN; Samba fields stay consistent. | Symmetric membership editing via the dual-pane transfer; description and other fields schema-driven; Samba mapping stays consistent. |
| **Delete** | Warns the user will be removed from N groups; handles the last-member rule on affected groups; accounts for Samba mappings — not a bare delete + cryptic error. | Confirms; members' `memberOf` is cleaned by the overlay; warns if the group is referenced (e.g. nested/owned); removes any Samba group mapping. |
| **Rename** | MODRDN with refint repairing references; labels and membership views update accordingly. | MODRDN on `cn`; refint repairs references (nested membership, owner/manager); labels update everywhere. |

There is therefore **no separate "onboarding wizard" feature** — that
orchestration is simply what the users tier does at create time, and equivalent
domain-aware behaviour applies to view/edit/delete/rename. The bare
single-entry form (no special knowledge) is reserved for one-off objects edited
through the generic browser.

The guided Users/Groups/Membership screens are this upper tier; the form engine
and object operations are the foundation they delegate to.

## 4. Module Structure

Single binary crate, `edaptor`:

```
edaptor/
├── src/
│   ├── main.rs              # CLI args (--config), bootstrap, run app
│   ├── config/              # TOML config: load, validate, profiles
│   │   └── mod.rs           #   ServerConfig, AuthMethod, EntryProfile, PasswordCfg
│   ├── ldap/                # background worker + protocol
│   │   ├── worker.rs        #   owns ldap3 conn; req/resp over channels; cancel
│   │   ├── request.rs       #   Request enum (Bind, Search, Add, Modify, ModRdn, Delete)
│   │   ├── response.rs      #   Response/Event enum (Results, Error, Progress)
│   │   └── ldif.rs          #   render a ChangeSet as LDIF (preview)
│   ├── schema/              # subschema introspection → typed model
│   │   ├── model.rs         #   ObjectClass, AttributeType, Syntax, MatchingRule
│   │   └── parse.rs         #   parse cn=subschema entries
│   ├── form/                # generic schema-driven form engine
│   │   ├── generator.rs     #   (objectClasses + profile) → FieldSpec list
│   │   ├── field.rs         #   FieldSpec: widget kind, validation, MUST/MAY, cardinality
│   │   └── changeset.rs     #   diff(original, edited) → ChangeSet (modify/add/modrdn/delete)
│   ├── samba/               # Samba-specific logic (the largest bespoke chunk)
│   │   ├── nthash.rs        #   MD4(UTF-16LE(pw)) → sambaNTPassword; sambaPwdLastSet
│   │   ├── sid.rs           #   SID/RID algebra; algorithmic RID; sambaDomain discovery
│   │   ├── account.rs       #   sambaAcctFlags, account lifecycle, primary group SID
│   │   └── groupmap.rs      #   sambaGroupMapping (posix group ↔ samba group SID + type)
│   ├── ui/                  # thin facade over turbo-vision
│   │   ├── facade.rs        #   our widget vocabulary; isolates the dependency
│   │   └── widgets.rs       #   membership transfer pane, entry tree, dynamic form view
│   ├── workflows/           # guided screens = thin views over the form engine
│   │   ├── users.rs         #   list/add/edit/delete user; rich create = entry +
│   │   │                     #   synced password + initial groups + optional Samba
│   │   ├── groups.rs        #   list/add/edit/delete group
│   │   ├── membership.rs    #   symmetric dual-pane editor
│   │   ├── ou.rs            #   OU / container management
│   │   └── browser.rs       #   generic DIT browser/editor
│   └── app.rs               # app state, screen routing, glue between UI and worker
```

## 5. Config File Format

A single TOML file (`--config <path>`, default `~/.config/edaptor/config.toml`)
holding all connection properties plus entry profiles. The `[server]` block is
singular in v1; the structure permits adding multi-server support later without
breaking changes.

```toml
# edaptor configuration

[server]
uri          = "ldaps://ldap.example.com"   # ldaps:// or ldap:// (+ start_tls)
base_dn      = "dc=example,dc=com"
start_tls    = false                         # upgrade an ldap:// connection
timeout_secs = 10

[server.tls]
ca_cert     = "/etc/ssl/certs/ca.crt"        # optional; system trust store if omitted
client_cert = "/etc/ssl/client.crt"          # for SASL EXTERNAL
client_key  = "/etc/ssl/client.key"
verify      = true

[auth]
method  = "simple"            # "simple" | "external" | "gssapi"
bind_dn = "cn=ldapmanager,dc=example,dc=com"
# Password is NEVER stored here. Resolution is configurable:
password_source = "prompt"    # "prompt" | "env:EDAPTOR_PW" | "command:pass ldap/mgr"

# Optional Samba fallback if no sambaDomain entry exists in the directory:
[samba]
domain_sid          = "S-1-5-21-..."   # fallback only; discovery from sambaDomain preferred
algorithmic_rid_base = 1000

# ---- Entry profiles: what "a user"/"a group" mean in THIS directory ----
[[profile]]
name           = "user"
container_dn   = "ou=people,dc=example,dc=com"
object_classes = ["inetOrgPerson", "posixAccount", "shadowAccount"]
rdn_attribute  = "uid"
label             = "{cn} ({uid})"                 # how entries show in lists/pickers; falls back to RDN
search_attributes = ["cn", "uid", "mail"]          # what incremental search matches on
show              = ["uid", "cn", "sn", "givenName", "mail",
                     "uidNumber", "gidNumber", "homeDirectory", "loginShell"]

[profile.password]
ldap_attribute = "userPassword"   # sent cleartext over TLS; slapd hashes + enforces ppolicy
samba          = true             # if entry is sambaSamAccount, also set sambaNTPassword

[[profile]]
name           = "group"
container_dn   = "ou=groups,dc=example,dc=com"
object_classes = ["groupOfNames"]
rdn_attribute  = "cn"
label             = "{cn}"
search_attributes = ["cn", "description"]
show              = ["cn", "description"]
member_attribute  = "member"      # drives the membership transfer widget
```

**Rules**

- `show` only controls **ordering and emphasis**; it never overrides what the
  live schema declares as MUST. Attributes not in `show` are still editable.
- `label` and `search_attributes` apply to **every** list, picker, and tree
  node in the application (see §7).
- `member_attribute` and `[profile.password]` are how a guided workflow knows to
  inject the membership pane and the password action without hard-coding
  attribute names.
- Additional `[[profile]]` blocks (e.g. `service`, `computer`) appear as extra
  guided lists, reusing the same engine.

## 6. Data Flow

### Startup

1. Load and validate config; resolve the bind password via `password_source`.
2. Spawn the `ldap::worker` thread; it connects (TLS) and binds per `[auth]`.
3. Worker fetches `cn=subschema` once; `schema::parse` builds the typed model
   (objectClasses, attributeTypes, syntaxes, MUST/MAY, cardinality).
4. App opens the main screen: a Turbo Vision shell (menu + status bar) listing
   the profile views (Users, Groups, …), the onboarding wizard, OU management,
   and the generic browser.

### Edit flow (the generic engine + thin guided views)

```
select profile/entry → worker SEARCH → entry attributes
        │
        ▼
form::generator(objectClasses ∪ profile, schema) → FieldSpec[]
   • widget kind chosen from attribute SYNTAX
     (boolean→checkbox, integer→numeric, DN→picker,
      generalizedTime→date, binary/photo→skip-or-note, else→text)
   • MUST → required; single- vs multi-valued → single field vs list editor
   • profile.show → ordering and which MAY attributes surface by default
        │
        ▼
user edits → on save: form::changeset.diff(original, edited) → ChangeSet
        │
        ├─ (preview key) → ldap::ldif.render(ChangeSet) → confirm dialog
        ▼
worker MODIFY / ADD / MODRDN / DELETE → on success, re-read entry → refresh view
```

## 7. Labels Everywhere

Humans never stare at cryptic DNs unless they ask to. Driven by per-profile
`label` + `search_attributes`:

- **Lists** (Users, Groups, …): rows show the label (e.g. `Tobias Oetiker
  (oetiker)`), filtered by incremental search over `search_attributes`.
- **Membership transfer:** both panes (members | non-members) have their own
  incremental search box and show labels, `cn` first. The non-members side uses
  server-side paged search so it scales to large `ou=people`.
- **DN pickers** in generic forms: show labels, search by `search_attributes`.
- **Generic tree browser:** each node shows its label (`cn`, or `description`
  for OUs) with the raw RDN secondary/dimmed; incremental search jumps to a node.
- Raw DNs appear only on demand: the LDIF preview, or a "show DN" toggle.

## 8. LDAP Semantics Built In

- **`memberOf` is read-only** (maintained by the overlay). The tool *displays*
  it on users but never writes it. All membership edits write the `member`
  attribute on the **group**, then refresh. The user-side "groups" view is a
  convenient front-end that edits group entries.
- **`groupOfNames` requires ≥1 `member`.** Removing the last member is illegal;
  the tool blocks it with a clear message and offers to delete the group or keep
  a placeholder. (The role could later switch to a `member`-optional object
  class; v1 handles vanilla `groupOfNames`.)
- **Rename = MODRDN.** Editing the RDN attribute (e.g. `uid`) issues a modrdn,
  not a modify; `refint` then repairs references automatically. The tool detects
  RDN changes and issues the correct operation.
- **`userPassword`:** sent as cleartext **over TLS**; slapd hashes it
  (`olcPPolicyHashCleartext: TRUE`) and enforces ppolicy. Policy rejections
  surface as readable messages. Password actions are **refused on a non-TLS
  connection**.
- **Large containers:** searches use the **paged-results control**; lists load
  incrementally with a filter box.
- **Deletes:** require confirmation; `refint` cleans dangling references; the
  tool warns when an entry is referenced.

## 9. Samba Support (Full Lifecycle)

Samba passwords are not hashed by the server, so the client owns this logic.

- **NT hash:** `sambaNTPassword = uppercase_hex(MD4(UTF-16LE(password)))`
  (the `md4` crate). On set, also update `sambaPwdLastSet`. ppolicy does **not**
  apply to the Samba hash.
- **Synced passwords (default):** one password prompt sets both `userPassword`
  (server-hashed) and `sambaNTPassword` (client-hashed) so Unix and Samba stay
  in sync.
- **SID provisioning:** `sambaSID` is computed algorithmically from `uidNumber`
  (users: `uidNumber·2 + rid_base`) / `gidNumber` (groups: `gidNumber·2 +
  rid_base + 1`). The **domain SID and `sambaAlgorithmicRidBase` are discovered
  from the `sambaDomain` entry** in the directory; `[samba]` in config is a
  fallback only.
- **Account lifecycle:** create/edit full `sambaSamAccount` entries —
  `sambaSID`, `sambaPrimaryGroupSID`, `sambaAcctFlags` (e.g. `[U          ]`),
  password timestamps. Groups can be Samba-mapped via `sambaGroupMapping`.
- **Security:** NT hashes are unsalted and weak; `sambaNTPassword` must be
  ACL-protected like `userPassword` (the role is expected to enforce this).

## 10. Error Handling

- **Every LDAP result code maps to a human message:** `constraintViolation` →
  "Password rejected by policy: …"; `objectClassViolation` → "Missing required
  attribute: `sn`"; plus `namingViolation`, `insufficientAccess`, etc. A "show
  technical detail" toggle reveals the raw code/DN.
- **Client-side validation first:** MUST fields, syntax, and single-valued
  constraints are checked before sending, so most errors never reach the wire.
- **Connection/bind/TLS failure** → modal with the specific cause and
  Retry / Quit.
- **Worker isolation:** a panic or hang in the worker is reported in the UI
  (operations are cancellable) and never freezes or crashes the screen.
- **Refuse-by-policy:** password actions are blocked on non-TLS connections,
  with an explanation.
- **No silent success:** after every write the entry is re-read and the view
  refreshed — "saved" always means "confirmed on the server."

## 11. Testing Strategy

- **Headless core (the bulk):** `schema`, `form`, `samba`, `changeset`, and
  LDIF rendering are pure and unit-tested against fixtures (captured
  `cn=subschema`, sample entries). Includes **NT-hash known-answer vectors**,
  **SID/RID computation** tests, and **golden-file LDIF** tests.
- **Integration against real slapd:** tests spin up OpenLDAP in a **podman**
  container seeded with the `oposs.openldap` structure (OUs, overlays, ppolicy,
  Samba schema), then exercise bind → search → add/modify/modrdn/delete →
  membership → Samba password end-to-end. This verifies the real-world gotchas
  (memberOf read-only, last-member rule, refint cascades, ppolicy enforcement).
- **UI:** kept thin; because logic lives below the facade, screens are
  smoke-tested for construction rather than pixel-asserted.

## 12. User Stories

### A. Connection & startup
- **A1.** Launch `edaptor` (optionally `--config path`); it connects, binds per
  configured auth, and shows the main screen.
- **A2.** When a password is needed, prompt once (or read from env/keyring/
  command); never write it to disk.
- **A3.** On connection/bind/TLS failure, show a clear, specific error and allow
  retry or quit.

### B. Browsing & finding
- **B1.** See Users / Groups / … lists (one per profile) plus a generic DIT tree
  browser.
- **B2.** Type to filter any list live (by `search_attributes`), scaling to
  thousands of entries via paged search.

### C. Users
- **C1.** "Add User": a guided create built from the user profile's
  objectClasses (required fields marked, types enforced) that, in one sequence,
  creates the entry in the right OU/RDN **and** optionally sets the (synced)
  password, selects initial groups, and Samba-enables the account — because the
  users tier understands all of these (see §3.1). No separate wizard.
- **C2.** Open a user and edit any attribute, respecting single/multi-valued,
  MUST/MAY, and syntax.
- **C3.** Set/reset a user's password; policy violations come back readable;
  refused on non-TLS; syncs the Samba hash when the user is a `sambaSamAccount`.
- **C4.** Rename a user (change uid) via MODRDN; references stay intact via
  refint.
- **C5.** Delete a user after confirmation; warned if referenced.

### D. Groups
- **D1.** Add/edit/delete groups the same schema-driven way.
- **D2.** Attempting to remove the last member of a `groupOfNames` is stopped
  with options (delete group / keep placeholder).

### E. Membership (core)
- **E1.** From a group, open a dual-pane transfer (members | non-members), each
  pane with its own incremental search, rows labelled `cn` first; multi-select
  add/remove writes the group's `member`.
- **E2.** From a user, see and edit which groups they belong to (shows
  `memberOf`, edits the groups' `member`), groups shown by label.
- **E3.** Both views stay consistent after a change (re-read via overlay).

### F. Safety & transparency
- **F1.** A keystroke shows the exact LDIF that will be sent before applying.
- **F2.** Destructive actions confirm; long operations show progress and can be
  cancelled.

### G. v1 guided extras
- **G1. Rich user create (not a separate wizard):** because the users tier
  understands passwords/groups/Samba (§3.1), the Add User flow can, in one
  sequence, create the entry → set the (synced) password → add to N groups →
  optionally Samba-enable. This *is* the onboarding path; it lives inside the
  user create flow, not as a standalone feature.
- **G2. OU management:** create / rename / delete organizational units (shape
  the tree, not only populate it).

## 13. Out of Scope (Deferred)

- Multi-server / profile switching (config structured to allow it later).
- CSV / bulk import.
- Directory-stored templates (v1 keeps templates in config).
- Audit log / change history.

## 14. Risks & Mitigations

- **`turbo-vision` is young / single-maintainer.** Mitigation: keep a thin UI
  facade so the dependency sits behind one boundary; vendor the crate if
  upstream stalls. Pin the version.
- **Samba lifecycle is the largest bespoke surface.** Mitigation: isolate in the
  `samba` module with strong unit tests (hash vectors, SID algebra); treat as
  its own implementation phase.
- **Directory-specific assumptions** (object classes, OUs). Mitigation: profiles
  in config make these explicit; live schema introspection adapts forms to the
  actual server.
