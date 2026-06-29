# Changelog

All notable changes to eDAPtor are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project aims to follow
[Semantic Versioning](https://semver.org/).

## Unreleased

### New

- The samba domain SID is now discovered live from a `sambaDomain` directory
  entry at startup (when a `sambaSID` widget is configured), falling back to the
  configured `[samba] domain_sid`. When discovery succeeds, the algorithmic RID
  base is also taken from the directory (`sambaAlgorithmicRidBase`, default 1000),
  overriding any `[samba] algorithmic_rid_base` in config.

- X-ORDERED attributes (`kind = "x_ordered"`) are now editable in the UI: an
  ordered list editor (add / delete / Alt+↑/↓ reorder) that hides the `{n}`
  ordering prefix and regenerates it from row order on save.

- **The TUI is now Turbo-Vision–based.** The user interface was migrated from
  ratatui to [tvision-rs](https://github.com/oetiker/tvision-rs): a frameless
  full-screen three-pane browser/editor with modal rich-field editors. The old
  ratatui implementation and the transitional `edaptor-tv` dev binary were
  removed, along with the `ratatui` / `tui-tree-widget` / `tui-prompts` /
  `crossterm` dependencies.

- **tvision UI: config picker at startup.** When more than one config is
  discovered in `~/.config/edaptor/` or `/etc/edaptor/`, a Turbo-Vision
  "Select configuration" dialog now lists them (name + description + path) so
  you can choose one; a single discovered config or an explicit `--config`
  skips the picker. (Startup-flow groundwork for the M5 cutover.)

- **tvision UI:** Save now performs the combined membership write end-to-end. When
  a form's membership (fan-out) field has staged changes, Save plans a combined
  save — the user's own MODIFY plus one MODIFY per touched group — previews the
  full multi-stanza LDIF in the confirm dialog, and on OK submits the multi-entry
  fan-out batch. The user's own `memberOf` is never written directly (it is
  overlay-maintained); only each group's `member` is modified. Forms without a
  membership change keep the existing single-entry Save path. A rename combined
  with a membership change is refused with a clear message (do them as separate
  saves).

- Removing the last member of a required-membership group (`groupOfNames` /
  `groupOfUniqueNames`) is now blocked client-side before saving, with a clear
  message; emptying a `posixGroup` (`memberUid` is optional) is still allowed.

- **tvision UI:** membership editor — a two-column "mover" dialog for fan-out
  `picker` fields (a `picker` with a `fanout` attribute, e.g. a user's group
  memberships). The left **Available** column is a search box over live LDAP
  candidates (groups) that refreshes as you type; the right **Members** column is
  the staged set, seeded from the field's current values. Insert / → moves the
  highlighted Available row into Members (de-duplicated by DN, case-insensitive);
  Delete / ← removes the highlighted Members row; Tab flips which column Up/Down
  navigate; Space types into the search box; Enter confirms the dialog. OK stages
  the chosen member set (the per-group fan-out write is produced by the
  combined-save path); Cancel discards it. The underlying write flow correlates
  the user's own MODIFY plus one MODIFY per touched group as a single batch that
  reports success only once every leg has landed.

- **tvision UI:** picker widget with live LDAP search. A `picker`-bound field
  (e.g. `gidNumber`, `member`, `memberUid`) opens a modal with a search box over
  a candidate list that refreshes as you type (server-side capped at 100).
  Single-select uses radio markers (a pick replaces); multi-select uses
  checkboxes (Insert toggles), with the current selection shown selected-first.
  Space types into the search box (so multi-word substring searches work); Enter
  confirms the dialog. A pick stores the candidate DN (`store = "dn"`) or a chosen
  scalar attribute (e.g. the group's `gidNumber`).

- **tvision UI:** activating a `sambaSID` field auto-generates the SID
  immediately (no popup) from the entry's `uidNumber` and the configured Samba
  domain. A missing `uidNumber` or unconfigured domain SID shows an error dialog
  and leaves the field manually editable.

- **tvision UI:** `choice` widgets (`loginShell`, `sambaAcctFlags`, …) are now
  editable via a modal radio / checkbox list. Single-select (Plain format)
  replaces the selection; multi-select (Bracketed format) toggles tokens while
  preserving any unmanaged letters (lossless merge). The dialog seeds the
  pre-checked state from the field's current value and stages the result live.

- Passwords can be set via a masked New/Confirm editor (create and edit),
  writing the configured attribute (plus the Samba NT hash when enabled); the
  editor refuses on an unencrypted connection.
- Create-form numeric fields with a `{next:MIN-MAX}` default auto-allocate the
  next free value via a background directory scan.
- New entries can be created from a profile in the tvision UI (Alt+N): a profile
  chooser (or single-profile fast path), a create-mode form with
  auto-injected/editable objectClass and live DN, validated and submitted as an
  LDAP ADD.
- **tvision UI:** multi-valued attributes are now editable via a modal editor
  (a row list plus an edit line): add (Alt+a / Insert), delete (Alt+d /
  Delete), reorder (Alt+Up / Alt+Down), and edit the selected row. Empty rows
  are dropped on commit. This unblocks editing plain multi-valued attributes
  (the form previously only allowed single-valued ones). X-ORDERED attributes
  remain read-only in the tvision UI for now — editing them needs the `{n}`
  ordering-prefix strip/reconstruct, which is deferred.
- **tvision UI:** the `objectClass` field is now editable via a schema-seeded
  multi-select picker (search + tick). Changing the set regenerates the form's
  fields live — newly-allowed attributes appear, now-disallowed ones are marked
  orphaned (dropped on save) — driven by a typed resync outcome.
- tvision UI (preview, `edaptor-tv`): the entry form is now editable for plain
  single-value attributes, with an LDIF-preview save confirmation, async writes
  (MODIFY + rename/MODRDN), and dirty-change guards on navigation and quit.
- tvision UI (preview): keyboard navigation — Tab/Shift-Tab switch panes;
  within a pane the arrow keys navigate (tree branches, leaf list while the
  search box keeps focus, and form fields). `edaptor-tv` also accepts
  `--config <path>` (matching the main binary).
- tvision UI (preview): the main window now runs frameless full-screen — the
  three panes fill the terminal edge-to-edge (no window border or title bar),
  while the menu bar and status line are kept.
- tvision UI (preview): the three panes now fill their area and the entry form
  scrolls — a vertical scrollbar appears when an entry has more attributes than
  fit, and every attribute is reachable (the former 32-row display cap is gone).

### Changed

- tvision UI (preview): now builds against the released `tvision-rs` 0.3.0; the
  temporary git-pin for `exec_view_focused` has been removed.

### Fixed

- tvision UI (preview): the Save-confirm and unsaved-changes guard dialogs opened
  with the wrong button focused (Cancel / Stay), so pressing Enter cancelled the
  save instead of confirming it — a save could not be completed by keyboard. The
  dialogs now open with the primary action (Save) focused. (Uses tvision-rs's
  `exec_view_focused`, released in 0.3.0.)
- tvision UI (preview): switching to another entry while a form had unsaved
  changes did nothing when driven by the **mouse** — no save prompt, and the form
  got stuck. The unsaved-changes guard now fires consistently for keyboard *and*
  mouse: a dirty form is **pinned** (no other entry is shown) until you choose
  Save / Discard / Stay, and **Stay** snaps the list highlight back to the form on
  screen. When the form is clean it still follows the highlight as you browse.
- tvision UI (preview): cancelling the save-confirm raised by an unsaved-changes
  guard now snaps the list highlight back to the form being edited; and changing
  branch in the tree while the form is dirty now raises the same guard (Stay
  reverts the tree, Discard/Save behave as on a leaf change).

## 0.4.0 - 2026-06-12

### New

- **Space toggles/selects in choice widgets.** In a fixed checkbox/radio list
  (e.g. `loginShell`, `sambaAcctFlags`), pressing Space now toggles a multi-select
  option or radio-selects a single-select option — the same as Enter. Search
  pickers are unchanged: Space there is still a literal search character.
- **`edaptor passwd` accepts a bare username**, not just a full DN. A username
  (any argument without an `=`) is resolved to a DN by searching every configured
  profile's `search_base` for `(<rdn_attr>=<username>)`. The lookup runs **before**
  the password prompt, so an unknown or ambiguous username fails immediately
  (with the matching DNs listed on ambiguity) instead of after typing the
  password twice. The resolved DN is printed before the prompt for confirmation.
- `examples/oposs-openldap.toml` — ready-to-use config template for directories
  managed by the [oposs.openldap](https://github.com/oposs/oposs.openldap)
  Ansible role (POSIX users, groupOfNames + posixGroup, Samba and mailAccount
  optional blocks).
- **`auth.method = "external"`** — SASL EXTERNAL bind over a Unix domain socket
  (`ldapi:///`). Allows password-free root access when running on the slapd
  host; no TLS required. `ldapi://` connections are also treated as secure for
  password-change operations.
- **Config auto-discovery** — edaptor now searches `~/.config/edaptor/*.toml`
  and `/etc/edaptor/*.toml` at startup. A single config is used silently;
  multiple configs trigger a ratatui picker. The `--config` flag bypasses
  discovery as before.
- **`[meta]` table** in config files — optional `name` and `description` fields
  displayed in the startup picker.
- **ObjectClass-driven attributes**: When the `objectClass` field is edited via
  the new schema-seeded picker, the edit form immediately injects fields for all
  MUST and MAY attributes introduced by the new class, and marks attributes no
  longer permitted by any remaining class as _orphaned_ (shown crossed out). All
  changes — objectClass modification, new attribute values, and attribute deletions
  — are sent as a single atomic LDAP `ModifyRequest`.
- **`sambaSID` auto-generate**: an empty `sambaSID` field shows
  `⟨Enter to auto-generate⟩`. Pressing Enter computes the SID from the entry's
  `uidNumber` and the domain context; missing prerequisites (no domain SID,
  empty/non-numeric `uidNumber`) surface a specific error. The field stays
  editable for manual override. The domain SID is discovered at startup from a
  live `sambaDomain` entry, falling back to `[samba].domain_sid` in the config.
- **Next-number allocation on demand**: in a create form, a field whose default
  is `{next:MIN-MAX}` (e.g. `uidNumber`, `gidNumber`) now shows
  `⟨Enter to allocate⟩`. Pressing Enter scans the directory and fills the next
  free number immediately, so dependent widgets (like `sambaSID` auto-generate)
  can use it before save. Skipping it still works — the value is allocated at
  save time as before. The field stays editable for manual override.
### Changed

- Widget configuration is now auto-applied for standard LDAP schemas
  (posixAccount, posixGroup, shadowAccount, sambaSamAccount, groupOfNames,
  groupOfUniqueNames, inetOrgPerson, OpenLDAP cn=config). A typical deployment
  no longer needs `[profile.widget]` entries for these well-known attributes.
- Attributes flagged `NO-USER-MODIFICATION` in the server's subschema are
  automatically rendered read-only, even without explicit widget config.
- `userPassword` is now automatically treated as a password field for entries
  with `person`, `inetOrgPerson`, or `posixAccount` objectClasses.
- New widget kind `readonly`: marks an attribute display-only (excluded from
  the changeset). Available in user config for custom schemas.
- New widget kind `x_ordered`: handles OpenLDAP X-ORDERED attributes
  (`{n}` prefix management). Available in user config for custom schemas.
- `memberOf`, `sambaNTPassword`, `sambaLMPassword` are now read-only by
  default via the built-in schema bundle (previously hardcoded).
- `j`/`k` accepted as aliases for ↓/↑ in the tree pane, profile-chooser overlay,
  and config-discovery picker.

### Fixed

- `auth.method = "external"` (SASL EXTERNAL / ldapi) no longer forces read-only
  mode. Only a `simple` bind with no `bind_dn` is treated as anonymous.

## 0.3.0 - 2026-06-09

### New

- **Configurable widget palette** via `[profile.widget.<attr>]` — bind an
  attribute to a richer editor than a plain text box:
  - `kind = "choice"` — pick from a fixed vocabulary and (de)serialize a single
    value. Shipped for `sambaAcctFlags` (multi-select account flags, losslessly
    preserving flags the UI does not surface) and `loginShell` (single-select
    from a configured shell list). Read-only fields show a human-readable summary;
    Enter opens a checklist/radio popup.
  - `kind = "password"` — turns the attribute into a masked **set-password
    field**. Enter on it (or on any derived Samba password attribute) opens a
    New+Confirm popup that updates `userPassword` and, when `samba = true`,
    `sambaNTPassword` + `sambaPwdLastSet` in one atomic change.
  - `kind = "picker"` — populate an attribute from a live candidate search and
    store the picked value(s) in this entry (value lookup like `gidNumber`, or a
    DN/scalar list like `member`/`memberUid`). `candidate` is a `[[profile]]`
    name or an inline `{ base, object_classes, … }` scope.
  - `kind = "membership"` — fan this entry's DN into a back-reference attribute
    (`via`) on each picked candidate (e.g. `memberOf` writes `member` on each
    chosen group).
### Changed

- **Pickers are now configured with `[profile.widget.<attr>] kind = "picker"` /
  `"membership"`** instead of `[profile.picker.<attr>]`, which has been removed.
- **Passwords are now configured with `[profile.widget.<attr>] kind = "password"`**
  instead of `[profile.password]`, which has been removed.
- **Password and hash attributes (`userPassword`, `sambaNTPassword`,
  `sambaLMPassword`) are read-only inline** and changed only through the
  set-password popup — preventing a typed cleartext password from being written
  verbatim into a hash attribute.
- **Password changes require an encrypted connection** (`ldaps://` or
  `start_tls = true`); the popup refuses to open on a plaintext connection.

### Fixed

- Password-profile entries no longer appear permanently "dirty" (which popped a
  spurious Save/Discard/Stay guard on every navigation between entries).
- Secret values (passwords, NT hashes) are no longer shown in clear in the change
  (LDIF) preview — they are masked as `********` regardless of how they enter the
  change.

## 0.2.0 - 2026-06-08

### New

- Configurable, presence-keyed, width-aware DIT tree labels via `[[tree.label]]`
  rules. The structural RDN is now always shown by default (e.g.
  `ou=people (People)`), and narrow panes degrade gracefully while keeping the RDN.
### Changed

- DIT tree branch labels now always include the RDN by default (previously a
  node with a `description` showed only the description).

### Fixed

## 0.1.0 - 2026-06-04

### New

- Schema-driven TUI for administering an OpenLDAP directory: browse, create,
  edit, rename, and delete users and groups with forms generated from live
  `objectClass` definitions (`cn=subschema`).
- TOML configuration with entry profiles, defaults (literal / template /
  `{next:MIN-MAX}` auto-number), inline passwords, the full Samba lifecycle, and
  unified `[profile.picker.<attr>]` candidate pickers.
- Three-pane ratatui interface with symmetric membership editing and on-demand
  LDIF preview of the exact change before it is applied.
- rustls TLS backend (custom CA, optional StartTLS, connect timeout).
- Provisioned podman test server (`scripts/test-ldap.sh`) and `edaptor passwd <dn>` CLI.
### Changed

### Fixed
