# eDAPtor — working agreement for agents

A schema-driven TUI LDAP editor (Rust + tvision-rs). Read this before touching the repo.

## At the start of every session: pull first

This repo is developed across machines and commits often land directly on `main`.
**Before doing anything, sync:**

```bash
git pull --ff-only
```

If the pull is not a clean fast-forward, stop and surface it — do not start work on
a stale tree.

## Keep the changelog and docs in sync with every change

Treat these as part of "done", not as an afterthought:

1. **`CHANGES.md`** — every user-visible change gets an entry under the current
   unreleased section. Behaviour changes, config-format changes, new/removed
   features, and notable fixes all belong here.
2. **`README.md`** — keep it a *short overview*. It must reflect current behaviour,
   but it must **not duplicate** the reference docs. When a config format changes,
   update the small skeleton example and the pointers — never paste the full
   reference back in.
3. **`docs/src/` (the mdBook)** — the canonical, exhaustive documentation. This is
   where full config references, worked examples, and concepts live. A
   config-format or behaviour change is not finished until the relevant page here
   is updated.

The single source of truth for configuration detail is the mdBook, surfaced at
<https://oposs.github.io/edaptor>. README links into it; it does not restate it.
When in doubt about where something goes: details → mdBook, orientation → README,
"what changed" → CHANGES.md.

## Documentation layout

- `docs/src/` — mdBook sources (`SUMMARY.md` is the table of contents). Build with
  `make docs` (or `cd docs && mdbook build`).
- `docs/superpowers/` — design specs, implementation plans, research notes and
  handovers. Historical/process record, **not** user-facing docs.
- `examples/config.toml` — the annotated reference config that
  `docs/src/configuration/full-example.md` embeds. Keep the two consistent.

## Build, test, lint

`cargo`/`make` from the repo root. **Cap parallelism at 4 cores** (shared machine):

```bash
make check          # fmt + clippy (-D warnings) + tests — run before declaring done
cargo test -j4
cargo clippy --all-targets -- -D warnings
make run            # run the TUI against the podman demo server
```

## Local test server

`scripts/test-ldap.sh start` launches a podman OpenLDAP mirroring the
`oposs.openldap` role, seeded with ~600 users / ~25 groups. Containers here use
**podman**, not docker.

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -- --config examples/demo-config.toml
```

## Config model (current)

Rich field editors are declared as `[profile.widget.<attr>]` with a `kind`:
`password`, `choice`, `picker`, `membership`. The former `[profile.picker.<attr>]`
and `[profile.password]` layers were **removed** — do not reintroduce them. See
`docs/src/configuration/widgets.md`.
