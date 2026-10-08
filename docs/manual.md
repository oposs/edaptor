---
title: EDAPTOR
section: 1
header: edaptor manual
footer: edaptor
date: 2026-10-07
---

# NAME

edaptor - schema-driven terminal editor for OpenLDAP directories

# SYNOPSIS

**edaptor** [**--config** *path*]

**edaptor** [**--config** *path*] **check**

**edaptor** [**--config** *path*] **schema** *object-class*

**edaptor** [**--config** *path*] **passwd** *user*

**edaptor** [**--config** *path*] **profiles** [**--detected-only**]

**edaptor** [**--config** *path*] **tui-create** [**--container** *dn*] [*profile*]

**edaptor** **--version** | **--help**

# CONFIGURATION

**edaptor** reads one TOML file.
It names the server (`[server]`), the bind (`[auth]`) and, optionally, entry profiles that override what **edaptor** detects in the directory.
The bind password is never stored in the file: `password_source` is `prompt`, `env:VAR` or `command:cmd`.

Without **--config**, **edaptor** looks for `*.toml` files in `$XDG_CONFIG_HOME/edaptor/` (or `~/.config/edaptor/`) and in `/etc/edaptor/`.
One file found is loaded.
Several files found open a picker.

The full reference is the eDAPtor manual at <https://oposs.github.io/edaptor>.

# DESCRIPTION

Without a command, **edaptor** opens a three-pane terminal interface: the directory tree, the entries of the selected container, and a form for the selected entry.
It creates, edits, renames and deletes entries, and edits group memberships.
Forms follow the server schema and the profiles in effect.

At startup **edaptor** samples the directory and detects profiles: object classes, naming, the fields a form shows, defaults, pickers, free `uidNumber` and `gidNumber` ranges and user-private groups.
Detection takes up to 10 seconds and prints `detecting profiles…` on standard error.
Values from the configuration file take precedence over detected values.

# OPTIONS

- `--config <path>`: Read the configuration from *path* and skip the search.
- `--version`: Print the version and exit.
- `-h, --help`: Print a usage summary and exit.

# COMMANDS

- `check`: Connect, bind and print a summary of the server schema.
- `schema <object-class>`: Print the effective attributes of *object-class*, with those inherited from its superclasses.
- `passwd <user>`: Prompt twice for a new password and set it on *user*, a user name or a full DN. Updates `userPassword` and, for a `sambaSamAccount`, `sambaNTPassword` and `sambaPwdLastSet` in one modify. Requires TLS.
- `profiles`: Print the profiles in effect as TOML that can be pasted into the configuration. Each value carries a comment saying whether it was detected, assumed or set by the configuration. Warnings and notes go to standard error. Password values are never printed.
- `profiles --detected-only`: Print the detected profiles before the configuration is applied.
- `tui-create [profile]`: Open the interface in the create form of *profile*, matched without regard to case. Without *profile*, a chooser is shown first.
- `tui-create --container <dn> [profile]`: Create the entry under *dn* instead of the profile's `search_base`.

# KEYS

- `Tab`: Move focus to the next pane.
- `Shift-Tab`: Move focus to the previous pane.

The eDAPtor manual lists the keys of each pane and dialog.

# EXIT STATUS

**edaptor** exits 0 on success, 1 on an error, which it prints on standard error, and 2 on an unknown option or a missing argument.

# ENVIRONMENT

- `XDG_CONFIG_HOME`: Base of the per-user configuration directory.
- `HOME`: Used for `~/.config` when `XDG_CONFIG_HOME` is unset.

A variable named in `password_source = "env:VAR"` holds the bind password.

# FILES

- `~/.config/edaptor/*.toml`: Per-user configuration files.
- `/etc/edaptor/*.toml`: System-wide configuration files.

# EXAMPLES

Show what **edaptor** detects in a directory before writing any profile:

```
edaptor --config /etc/edaptor/site.toml profiles --detected-only
```

Create a user in the detected profile `user-people`:

```
edaptor tui-create user-people
```

# SEE ALSO

**ldapsearch**(1), **ldapmodify**(1), **slapd**(8)

eDAPtor manual: <https://oposs.github.io/edaptor>
