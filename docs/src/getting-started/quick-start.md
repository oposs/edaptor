# Quick Start

The fastest way to try eDAPtor is the bundled podman test server. It launches an
OpenLDAP instance that mirrors a realistic deployment — Samba and mail schemas,
the memberOf / refint / ppolicy overlays, password policies — and seeds it with
sample data.

## Start the test server and run eDAPtor

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -- --config examples/demo-config.toml
```

The seed data contains roughly **600 users across several departments** and
about **25 groups**, all under the base DN `dc=example,dc=org`. Every generated
user shares the password `test123`, so you can log in and experiment freely.

When you are done, stop and remove the container with:

```bash
scripts/test-ldap.sh stop
```

## What you will see

On launch, eDAPtor presents a three-pane layout: the directory tree (DIT) on the
left, the entries within the selected node in the middle, and the selected
entry's attributes on the right. The focused pane is drawn with a double border.

```
┌─ DIT ───────┐┌─ Entries ────────┐╔═ Entry — uid=bob,ou=people,… ═╗
│ dc=example  ││ /                │║ uid           bob             ║
│ ├─ people   ││ ‹self› people    │║ cn            Bob Baker       ║
│ └─ groups   ││ Bob Baker (bob)  │║ sn            Baker           ║
│             ││ Babs Carr (babs) │║ givenName     Bob             ║
│             ││ Carl Diaz (carl) │║ mail          bob@example.org ║
│             ││ …                │║ uidNumber     10001           ║
│             ││                  │║ …                             ║
└─────────────┘└──────────────────┘╚═══════════════════════════════╝
 ↑↓ Field · ↵ Edit · Alt+S Save · Alt+C Cancel · Alt+X Quit
```

Move focus between panes with **Tab** and **Shift+Tab**. The key model is
Alt-based: **Alt+R** refresh, **Alt+N** new, **Alt+D** delete, **Alt+S** save,
**Alt+C** cancel, and **Alt+X** quit.

## Next steps

- [Configuration](../configuration/overview.md) — connect eDAPtor to your own
  directory and tailor its entry profiles.
