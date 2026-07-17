# edaptor — Session Handover

Carries the **current concern** into the next session. Not a project history — for
that see `git log`, the specs under `docs/superpowers/specs/`, the SDD ledger
(`.superpowers/sdd/progress.md`), and project memory (`…/memory/MEMORY.md`).

**Date:** 2026-07-17 · **Branch: `feat/inputline-fixes-v1.1.1`** (off `main` @ **v1.1.0**).
**Current concern: five post-deploy fixes → release `v1.1.1` → redeploy.** These came
from the user field-testing the **deployed** edaptor (v1.1.0 on the `ds-carbo-feh`
directory). All five are **agreed and in scope**; root causes are **confirmed** (via a
live tmux repro — see below). Nothing is implemented yet except the dependency bump.

The branch already has one commit: **`210f2dd` `build: bump tvision-rs to 0.12.1`**
(the new `InputLine::select_all_on_focus` opt-out; see enablers). `make`-build green.

---

## THE FIVE (do all five; then release v1.1.1 + redeploy)

Recommended: the **3 bugs first** (1–3), then the **2 features** (4–5). Bugs → use
`superpowers:systematic-debugging` (root causes below are Phase-1 done). Features →
`superpowers:brainstorming` then plan. Verify every one live in the **tmux harness**.

### 1. Form field order ignores `profile.show` 🐛 (the #1 user complaint)
**Confirmed root cause.** The create/edit **form** order is produced by
`order_fields()` in `src/workflows/edit_form.rs`: it pins `objectClass`, then buckets
(0 MUST · 1 populated/secret/widget-bound · 2 empty/orphaned) and sorts **alphabetically
within each bucket**. It has **no access to `profile.show`**, and `EditForm` does not
carry it. `build_create_form` → `empty_form_for_profile` *does* order by `show`
(`create.rs:501`) but then `sync_schema_fields` → `order_fields` **overwrites** it. So
the form is always alphabetical; `show` only orders the **browse/read view**
(`read_flow.rs:73`). *(This is why reordering `show` in the deployed config did nothing
for the create form.)*
**Fix.** Thread the profile's `show` into `order_fields` (or onto `EditForm`) so
show-listed fields keep show-order after `objectClass`, with the existing bucket logic
governing everything else. The user's desired grouping: identity fields
(`givenName, sn, uid, userPassword`) → autofilled (`gecos, displayName, cn, sambaSID`) →
other mandatory → the rest.

### 2. Password dialog "badly broken" 🐛
**Confirmed root cause.** `src/ui/pw_editor.rs` `PasswordDialog` uses **disabled**
display cells (`ro_cell`, `disabled=true`) + internal `new_buf`/`confirm_buf` + an
`active_field` flag, with **no visible focus/caret**. Symptoms: Tab flips `active_field`
invisibly (looks dead; real focus is stuck on OK); every keystroke `set_value()`s a
disabled cell which renders the bullets as a fully-**selected** block (Turbo Vision
select-all). Unit tests pass because they drive `handle_event` directly and only assert
`staged_commit`.
**Fix.** Rebuild the dialog on **real focusable masked `InputLine`s** with
`set_select_all_on_focus(false)` (0.12.1) — normal Tab/caret, no phantom select-all.
Keep the New+Confirm-match → `StageSecret` staging. Samba sync is unchanged (it's applied
at save via the widget's `samba` flag in `fold_create_password`, not by the dialog).

### 3. `cn` autofill lost on cursor-focus 🐛
**Status.** User confirmed they only **moved the cursor over `cn`** (never typed) and it
stopped auto-updating — so it is a real bug, NOT the operator-edit latch. `cn` is **not**
special-cased (identical `{givenName} {sn}` path as `displayName`/`gecos`, with passing
tests). Prime suspect: the same **select-all-on-focus** + `sync_into_form()` write-back
on focus change feeding a mangled value into the live-template latch
(`recompute_live` in `src/config/defaults.rs`; `FormPane::apply_live_templates` in
`src/ui/panes/form.rs`). **Likely fixed by #2's opt-out**, but **reproduce in tmux first**
to confirm the mechanism (not yet reproduced live).

### 4. `{auto:sambaSID}` computed default ✨ (decided syntax)
Today `sambaSID` is Enter-to-generate (auto-injected `SambaSid` widget; computed as
`domain_sid-(uidNumber*2 + rid_base)` — see `src/workflows/samba_compute.rs`,
`samba_sid_for_form`). Add a **computed default** token **`{auto:sambaSID}`** (name
chosen by the user) so it auto-populates on create like `{next:…}` does. Design it in
`src/config/defaults.rs` (a new `DefaultValue` variant, e.g. `Computed(kind)`), resolved
after the sibling `uidNumber` is available. Beware the async ordering (uidNumber is an
autonumber allocated by a background scan) — the compute must fire once `uidNumber`
resolves. Consider generalizing (only `sambaSID` needed now; keep the token extensible).

### 5. Placeholder text for derived/readonly fields ✨
Derived readonly fields (e.g. `sambaNTPassword`) render empty/broken-looking. Show an
informative affordance like **`⟨updated automatically when you set the password⟩`**.
(The value *is* written on save — it's just invisible until re-read.) Small UX change in
the form field rendering / the `readonly` widget presentation.

---

## ENABLERS

### tvision-rs 0.12.1 — `select_all_on_focus` opt-out (shipped)
Upstreamed this session. `InputLine::select_all_on_focus: bool` (default `true`) +
`set_select_all_on_focus(bool)`; when `false`, focus **gain** no longer selects-all
(caret/selection left as-is; typing inserts), focus **loss** still clears. PR
`oetiker/tvision-rs#18` **merged + released as 0.12.1**; edaptor already bumped
(`210f2dd`). Local rstv checkout: **`~/checkouts/rstv`** (a.k.a. `../rstv`).
- Known **pre-existing rstv test flake** (NOT ours): `input_line::tests::ins_toggles_cursor_ins`
  races the **process-global keymap** (`keymap.rs` `OnceLock<RwLock<Keymap>>`,
  `set_global`) under parallel tests. A CI re-run goes green. Worth a **separate rstv PR**
  to serialize keymap-mutating tests — offered, not yet done.

### tmux repro harness (WORKS — use it to reproduce + verify every fix)
The demo LDAP is driveable headlessly via tmux `send-keys` / `capture-pane`:
```bash
scripts/test-ldap.sh start                    # podman demo LDAP on :1389 (~600 users)
cargo build                                   # debug binary: /home/oetiker/scratch/cargo-target/debug/edaptor
tmux kill-session -t ed 2>/dev/null; tmux new-session -d -s ed -x 210 -y 52
tmux send-keys -t ed "EDAPTOR_TEST_ADMIN_PW=adminpassword <bin> --config <cfg>" Enter
tmux capture-pane -t ed -p                    # dump screen (plain; highlight is colour-only, invisible)
tmux send-keys -t ed Down Down Down Down      # tree: dc=example → ou=people (4 downs)
tmux send-keys -t ed M-n                       # Alt-N create (send TWICE — first often eaten by branch-reconcile)
```
`capture-pane -p` strips colour so the focus highlight isn't visible — reason about
position, or use `-e` for escapes. `M-n` = Alt-N.
**Repro config:** `examples/demo-config.toml` already has the `userPassword` password
widget (+samba) and the objectClasses. To also repro **#3 (cn)** add
`cn = "{givenName} {sn}"` (and `displayName`) to its `[profile.defaults]`, and put
`givenName`/`userPassword`/`sambaSID` in `show`. (This session used such a copy in the
scratchpad — recreate it; scratchpad is per-session.)

---

## DEPLOYMENT CONTEXT (`ds-carbo-feh`, carbo-link.com directory)
edaptor v1.1.0 is **deployed and in production** on host **`ds-carbo-feh-adm`** (ssh
alias; **confirm before every ssh** per user policy). Facts established this session:
- Ubuntu 26.04, x86_64, glibc 2.43; ssh user `oetiker_adm` (NOT root, **passwordless
  `sudo` works**). Binaries are **musl-static** (`x86_64-unknown-linux-musl`, built by the
  GitHub release workflow — download the release asset, don't hand-build).
- `/usr/local/bin/edaptor` = v1.1.0 (backup `edaptor.bak-2026-07-16`).
- `/etc/edaptor/ds-carbo-feh.toml` = the **reordered/companion/lookup** config (backup
  `ds-carbo-feh.toml.bak-2026-07-16`). Its `show` reorder only helps the **browse** view
  (see #1). ldapi `external` auth; **the directory advertises RFC 5805 txn** (companions
  are atomic here).
- **Run edaptor as root** on this host (`sudo edaptor …`): as `oetiker_adm` the ldapi
  SASL-EXTERNAL bind maps to the unprivileged peercred identity and can't read the base.
- **Redeploy recipe** (v1.1.1): `gh release download <tag> --pattern '*x86_64-unknown-linux-musl.tar.gz'`
  → scp to remote `/tmp` → `sudo cp -a` backup + `sudo install -m755/-m644` the new
  binary/config → verify with `sudo edaptor --config … check`. Include the field-order
  config once #1 lands (a reordered `show` **does** help once the code respects it).

---

## SHIPPED (v1.1.0 — for reference, all merged to `main`, `git log`)
The usability batch: **PgUp/PgDn form paging**, **live templated defaults** (create-mode
autofill), **`tui-create` + the "Create where?" container rule** (item c) + mouse-staging
fix, **companion entry on create** (item b, RFC 5805 atomic, proven live). PR #2 merged;
auto-released as **v1.1.0**. Specs/plans under `docs/superpowers/`.

**Non-blocking follow-ups** from the item-b reviews (own cycle, not urgent): DN-escape the
RDN value in `build_add_entry` **and** `plan_companion` together (pre-existing,
unreachable for uid-keyed entries); `debug_assert!(!entries.is_empty())` in
`run_add_atomic`; refresh the `do_create` doc-comment for the companion-plan borrow.

---

## Working agreement / how to resume
- **Pull first** (`git pull --ff-only`); this repo lands work across machines.
- **Ask before any `ssh`/remote command**; **never** run destructive commands without
  explicit confirmation (`rm`, `git reset --hard`, etc.).
- **SDD:** `SKILL=~/.claude/plugins/cache/claude-plugins-official/superpowers/6.1.1/skills/subagent-driven-development`
  → `scripts/task-brief PLAN N`, `scripts/review-package BASE HEAD`. Fresh implementer
  subagent per task → review package → task-reviewer → fix loop → final whole-branch
  review (most capable model). Ledger: `.superpowers/sdd/progress.md`.
- **Build/test (cap parallelism at 4 cores):**
  ```bash
  make check          # fmt + clippy -D warnings + tests — the gate
  cargo test -j4 ; make docs
  scripts/test-ldap.sh start ; export EDAPTOR_TEST_ADMIN_PW=adminpassword
  ```
- **Docs one-home:** config detail → mdBook (`docs/src/`); README orientation only;
  `CHANGES.md` every user-visible change; process/design → `docs/superpowers/`.
- **Facade boundary:** only `src/ui/**` may `use tvision_rs`; `ldap3` only in `src/ldap/**`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Finish:** one PR for `feat/inputline-fixes-v1.1.1` (remote `origin` =
  `git@github.com:oposs/edaptor.git`) → tag `v1.1.1` → redeploy.

## Project state
edaptor is a Rust TUI (**tvision-rs 0.12.1**) for administering OpenLDAP: introspects live
schema, generates edit forms from `objectClass` defs; TOML config declares connection +
*entry profiles* + a **widget palette** (`[profile.widget.<attr>]` kinds
`choice`/`password`/`picker`/`membership`/`lookup`/`readonly`/`x_ordered`),
`[profile.defaults]` (literal / `{attr}` template / `{next:MIN-MAX}` autonumber; live in
create mode), and `[profile.companion]`. `Cargo.toml` version **1.1.0**. Sole binary
`edaptor`; UI in `src/ui/`.
