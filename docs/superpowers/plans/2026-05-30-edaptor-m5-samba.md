# edaptor M5 — Samba lifecycle (headless) + synced password action

> **For agentic workers:** Implement task-by-task with **strict TDD**: write a failing test → run it to confirm the failure → implement → run `cargo test` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt --check` → commit. **The crate MUST compile after every task's commit.** Do **NOT** consult an advisor, do **NOT** stop to ask questions, do **NOT** pause to "verify the approach" — the blockers are pre-solved below; write code now and commit each task atomically. Commit with:
> ```
> git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf '<subject>\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
> ```

**Goal:** Implement the full Samba lifecycle logic (spec §9) as a **headless, fully unit-tested** `samba/` module, plus a **synced password action** (Unix `userPassword` + Samba `sambaNTPassword`, TLS-only, ppolicy) driven end-to-end by a new `edaptor passwd <dn>` CLI subcommand. **No turbo-vision work in M5** — TV `InputLine` has no password masking, so the interactive masked dialog is deferred to the M6 users tier (which is where the "Set Password" user action naturally lives). M5 is the largest bespoke-logic surface in the project; the risk is **silent correctness** (crypto + SID math + flag strings), so every value below is pinned with a concrete golden assertion.

**Architecture boundary:** `samba/` is pure logic — no terminal, no network. It produces/consumes plain types (`Vec<ModOp>`, `BTreeMap<String,Vec<String>>`, `String`). The worker (`ldap/worker.rs`) already has everything needed: the synced password action is just a `Vec<ModOp>` sent through the **existing** `Request::Modify` path — **no worker-API change**. The `passwd` subcommand lives in `main.rs` and reuses the existing `WorkerHandle` (connect/bind/search/modify). No `ldap3` type leaks past the worker.

**Tech stack:** Rust 2021, existing deps + **one new crate: `md4 = "0.11"`** (NT hash). `rpassword` (already a dep) reads the new password without echo in the `passwd` subcommand.

---

## Pre-solved blockers & verified facts (read before coding)

1. **NT hash — algorithm + crate API are compile-verified.** `sambaNTPassword = uppercase_hex(MD4(UTF-16LE(password)))`.
   ```rust
   use md4::{Md4, Digest};
   fn nt_hash(password: &str) -> String {
       let utf16le: Vec<u8> = password.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
       let mut h = Md4::new();
       h.update(&utf16le);
       h.finalize().iter().map(|b| format!("{:02X}", b)).collect()
   }
   ```
   **Golden vectors (assert these exactly):** `nt_hash("") == "31D6CFE0D16AE931B73C59D7E0C089C0"`, `nt_hash("password") == "8846F7EAEE8FB117AD06BDD830B7586C"`. (Both verified against the standard NT-hash test vectors.) Also assert the output is always 32 chars, all uppercase hex.

2. **`sambaPwdLastSet` = Unix epoch **seconds** as a decimal string.** Inject the timestamp — `fn samba_pwd_last_set(now_unix_secs: u64) -> String { now_unix_secs.to_string() }` — so it is unit-testable. The caller (`passwd` subcommand) passes `SystemTime::now()` converted to secs; the pure fn never calls `SystemTime::now()` itself.

3. **SID / RID algebra (spec §9, lines 306-310).** Algorithmic RID base default = **1000**. Users get **even** RIDs, groups **odd**:
   - user RID = `uidNumber * 2 + rid_base`
   - group RID = `gidNumber * 2 + rid_base + 1`
   - `sambaSID = format!("{domain_sid}-{rid}")`
   **Golden (assert exactly), domain `S-1-5-21-1-2-3`, base 1000:**
   - `user_sid("S-1-5-21-1-2-3", 1000, 1000) == "S-1-5-21-1-2-3-3000"` (RID 3000, even)
   - `group_sid("S-1-5-21-1-2-3", 1000, 1000) == "S-1-5-21-1-2-3-3001"` (RID 3001, odd)
   - `user_rid(0, 1000) == 1000`; `group_rid(0, 1000) == 1001`.

4. **`sambaAcctFlags` is exactly 13 chars: `[` + 11 interior + `]`.** Enabled normal user = `"[U          ]"` (`U` then **10 spaces**). **Assert both the exact literal AND `flags.len() == 13`** — the space count is the bug magnet. Provide `samba_acct_flags(disabled: bool) -> String`: enabled → `[U          ]`, disabled → `[UD         ]` (`U`,`D` then 9 spaces, still len 13). Build the string by left-justifying the flag letters in an 11-wide field then wrapping in brackets, so the width is structural, not a hand-counted literal.

5. **`sambaDomain` discovery (spec §9).** Search `(objectClass=sambaDomain)` under base; read `sambaSID` (the **domain** SID, e.g. `S-1-5-21-...`) and `sambaAlgorithmicRidBase` (string→u32, default 1000 if absent). Parse function takes an already-fetched `&LdapEntry`-like map (`&BTreeMap<String,Vec<String>>`) → `SambaDomainInfo { domain_sid, algorithmic_rid_base }`, so it is unit-testable without a server. Config `[samba]` is a **fallback only**: if no `sambaDomain` entry exists, use `config.samba.domain_sid` / `config.samba.algorithmic_rid_base`.

6. **`sambaGroupMapping`** (group ↔ samba group): attributes `sambaSID` (= `group_sid(...)`), `gidNumber`, `sambaGroupType = "2"` (SID_NAME_DOM_GRP / domain group), `displayName` (= the group cn). Provide `build_group_mapping_attrs(...) -> BTreeMap<String,Vec<String>>` plus the `objectClass` addition `sambaGroupMapping`.

7. **TLS gate (spec §10 "refuse-by-policy").** Pure fn on `ServerConfig`: `fn is_secure(server: &ServerConfig) -> bool { server.uri.trim_start().to_ascii_lowercase().starts_with("ldaps://") || server.start_tls }`. Password actions are **refused** when `!is_secure(...)` with an explanation; this is checked **before** building or sending any mods.

8. **Synced password = ONE atomic `MODIFY replace` (deliberate choice over the RFC 3062 exop).** `ldap3` 0.12 *does* expose `PasswordModify` (`ldap3::exop::PasswordModify`), but MODIFY-replace is chosen because it sets `userPassword` (cleartext → **server hashes** per `password-hash`, ppolicy intercepts MODIFY and returns `constraintViolation` on policy reject — already mapped by `result_code_message`) **atomically together with** `sambaNTPassword` + `sambaPwdLastSet`, keeping Unix+Samba in sync in a single operation, and reuses the existing `Request::Modify` path with **zero** worker change. (The exop only covers `userPassword` and would need a second non-atomic op + a new worker variant.) The exop remains a documented future alternative.

9. **`ModOp` shape (from `src/form/changeset.rs`, do not redefine):** `ModOp::Replace { attr: String, values: Vec<String> }` (also `Add`/`Delete`). The password builder returns `Vec<ModOp>`.

10. **Config struct (from `src/config/mod.rs`):** `Config { server, auth, profiles }`; add `#[serde(default)] pub samba: SambaConfig` where `SambaConfig { #[serde(default)] domain_sid: Option<String>, #[serde(default = "default_rid_base")] algorithmic_rid_base: u32 }`, `default_rid_base() -> u32 { 1000 }`. `#[derive(Debug, Deserialize)]` + a `Default` impl (or derive) so configs without `[samba]` still parse.

11. **Live test gating:** the bitnami `openldap:2.6.9` test image does **not** ship the Samba schema by default. The live test MUST probe the subschema (or attempt the write and detect `undefinedAttributeType` rc 17) for `sambaNTPassword`; if absent, **assert the Unix-only path** (`userPassword` replace round-trip, e.g. re-bind with the new password) and **`log`/`eprintln!` that the Samba assertions were skipped (schema absent)** — no silent skip. The pure unit tests pin all Samba correctness regardless. Live tests stay env-gated by `EDAPTOR_TEST_LDAP_URI` (+ `EDAPTOR_TEST_ADMIN_PW`), matching `tests/live_write.rs`.

---

## Task breakdown (3 sequential subagents; each TDD, atomic commits)

### S1 — Samba foundation: scaffold + nthash + sid + config
- [ ] **S1.0 scaffold.** Add `md4 = "0.11"` to `Cargo.toml`. Create `src/samba/mod.rs` with `pub mod nthash; pub mod sid; pub mod account; pub mod groupmap; pub mod password;` and the shared type `pub struct SambaDomainInfo { pub domain_sid: String, pub algorithmic_rid_base: u32 }`. Create empty stub files `src/samba/{account,groupmap,password}.rs` (each `// implemented in S2` + a trivial passing test or nothing) so the module tree compiles. Add `pub mod samba;` to `src/lib.rs`. Add `SambaConfig` + `samba` field to `src/config/mod.rs` (blocker #10) with a test that a config without `[samba]` parses and yields `algorithmic_rid_base == 1000`, and one with `[samba] domain_sid=... algorithmic_rid_base=...` parses. `cargo build` green. Commit.
- [ ] **S1.1 nthash.** `src/samba/nthash.rs`: `nt_hash(&str) -> String`, `samba_pwd_last_set(u64) -> String` (blockers #1, #2). Tests assert the two golden vectors, 32-char uppercase output, and `samba_pwd_last_set(1_700_000_000) == "1700000000"`. Commit.
- [ ] **S1.2 sid.** `src/samba/sid.rs`: `user_rid(uid: u64, base: u32) -> u64`, `group_rid(gid: u64, base: u32) -> u64`, `user_sid(domain_sid, uid, base) -> String`, `group_sid(domain_sid, gid, base) -> String`, and `parse_samba_domain(&BTreeMap<String,Vec<String>>) -> Option<SambaDomainInfo>` (reads `sambaSID` + `sambaAlgorithmicRidBase`, defaulting base to 1000 when absent). Tests assert all golden values in blocker #3, plus a `parse_samba_domain` round-trip from a synthetic attr map. Commit.

### S2 — account + groupmap + synced-password builder
- [ ] **S2.1 account.** `src/samba/account.rs`: `samba_acct_flags(disabled: bool) -> String` (blocker #4, assert literal + len 13), `build_samba_account_attrs(domain: &SambaDomainInfo, uid: u64, primary_gid: u64, disabled: bool, now_unix_secs: u64) -> BTreeMap<String,Vec<String>>` producing `sambaSID` (user_sid), `sambaPrimaryGroupSID` (group_sid of primary_gid), `sambaAcctFlags`, `sambaPwdLastSet`, and the `objectClass` value `sambaSamAccount` (as an Add-to-objectClass note for the caller). Tests pin every produced value against a fixed domain/uid/gid/now. Commit.
- [ ] **S2.2 groupmap.** `src/samba/groupmap.rs`: `build_group_mapping_attrs(domain, gid, display_name) -> BTreeMap<String,Vec<String>>` (blocker #6) + objectClass `sambaGroupMapping`. Tests pin `sambaSID == group_sid(...)`, `sambaGroupType == "2"`, `gidNumber`, `displayName`. Commit.
- [ ] **S2.3 password builder.** `src/samba/password.rs`: `is_secure(&ServerConfig) -> bool` (blocker #7); `build_password_mods(password: &str, is_samba_account: bool, now_unix_secs: u64) -> Vec<ModOp>` returning `Replace{userPassword,[cleartext]}` always, plus `Replace{sambaNTPassword,[nt_hash]}` + `Replace{sambaPwdLastSet,[now]}` when `is_samba_account`. Tests: non-samba → 1 mod (userPassword only); samba → 3 mods with the exact NT hash for a known password; `is_secure` true for `ldaps://`/`start_tls`, false for plain `ldap://`. Commit.

### S3 — `passwd` CLI subcommand (end-to-end) + live test
- [ ] **S3.1 subcommand.** Add a `passwd { dn: String }` subcommand to the clap CLI in `src/main.rs` (mirror the existing `schema` subcommand wiring). Flow: load config → **refuse if `!is_secure(&config.server)`** with a clear message and non-zero exit → prompt for the new password twice via `rpassword` (confirm match) → spawn `WorkerHandle` (bind) → `Search` base the target `dn` to read its `objectClass` (detect `sambaSamAccount`) → if samba, discover `SambaDomainInfo` (search `(objectClass=sambaDomain)`, else config fallback) → `build_password_mods(pw, is_samba, now)` → send `Request::Modify` → on `WriteOk` re-read & print confirmation ("no silent success"); on `WriteError` print the human message. Keep `main.rs` turbo_vision-free. Manual `--help` smoke is fine; no TV.
- [ ] **S3.2 live test.** Extend `tests/live_write.rs` (or a new `tests/live_samba.rs`), env-gated by `EDAPTOR_TEST_LDAP_URI`: create a user, run the synced-password mods, re-bind as that user with the new password to prove the Unix side; probe for `sambaNTPassword` schema and either assert the samba attrs were written or `eprintln!`-log the skip (blocker #11). Clean up the created entry.

---

## Definition of done
- `samba/` module: nthash, sid, account, groupmap, password — all with golden-pinned unit tests.
- `edaptor passwd <dn>` works end-to-end against the podman OpenLDAP (Unix side proven live; Samba side proven if schema present, else logged-skip).
- TLS gate refuses password actions on non-TLS.
- `cargo test` green, `cargo clippy --all-targets -- -D warnings` clean, `cargo fmt --check` clean.
- Facade boundary intact: `grep -rl 'use turbo_vision\|turbo_vision::' src/` lists **only** `src/ui/facade.rs` (M5 adds no TV usage).
- No `ldap3` type leaks past `src/ldap/`.

## Deferred to M6 (record, do not build in M5)
- Interactive masked password dialog in the TUI (needs a custom masked `InputLine` — its own TV spike).
- Rich users-tier create flow (onboarding: entry + synced password + initial groups + optional Samba-enable in one guided sequence).
- Samba-enable as a user action surfaced in the browser/edit UI.
- Loading the Samba schema into the test container (so the live samba assertions run unconditionally).
