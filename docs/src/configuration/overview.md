# Configuration Overview

edaptor is driven by a single [TOML](https://toml.io/) file. It declares the
LDAP connection, how to authenticate, and a set of *entry profiles* describing
what a "user" or a "group" means in your directory.

Pass it with:

```bash
edaptor --config /path/to/config.toml
```

When `--config` is omitted, edaptor looks for `~/.config/edaptor/config.toml`.

## The config declares intent, not field layouts

edaptor introspects the live schema (`cn=subschema`) and generates its edit
forms dynamically from the `objectClass` definitions it finds there. This means
the config file never enumerates fields or describes form layouts — those adapt
automatically to whatever your directory's schema says.

What the config *does* declare is **intent**: which server to talk to, how to
authenticate, and what an entry of each kind (a "user", a "group") is made of —
its object classes, where it lives, how it is named, and which related entries
its membership attributes draw from. The forms then follow from the schema.

## Top-level shape

A config file has three connection tables and one or more repeated profile
tables:

```toml
[server]        # where the directory is and how to reach it
[server.tls]    # optional TLS trust settings
[auth]          # how to bind
[[profile]]     # what a "user" / "group" / … means (repeatable)
```

`[[profile]]` is an *array of tables* — you write it once per kind of entry you
manage. Each profile may carry sub-tables (`[profile.defaults]`,
`[profile.password]`, `[profile.picker.<attr>]`) that refine how its entries are
created and edited.

## Orientation map

| Section | What it covers |
|---|---|
| [Server & Authentication](server-auth.md) | `[server]`, `[server.tls]`, `[auth]` — the connection, TLS trust, and bind credentials. |
| [Entry Profiles](entry-profiles.md) | `[[profile]]` — name, object classes, RDN attribute, search base, displayed/searched attributes, and labels. |
| [Defaults](defaults.md) | `[profile.defaults]` — literal, templated, and auto-numbered values that fill empty fields on create. |
| [Passwords](passwords.md) | `[profile.password]` — the inline masked password field and the Samba lifecycle. |
| [Pickers](pickers.md) | `[profile.picker.<attr>]` — populating an attribute from a live candidate search, including membership fan-out. |
| [Full Example](full-example.md) | The complete annotated `examples/config.toml`, walked through table by table. |

The fastest way to a working file is to copy
[`examples/config.toml`](full-example.md) and adapt the base DN, object classes,
and search bases to your directory.
