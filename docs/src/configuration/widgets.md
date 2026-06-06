# Widgets

A `[profile.widget.<attr>]` table declares a **rich in-line widget** for the
field of attribute `<attr>`. Instead of typing a raw string, the operator opens
a checklist overlay and ticks the desired options; eDAPtor encodes the result
in the correct format for the attribute.

## The `choice` kind

The only implemented `kind` is `choice`. It presents a fixed vocabulary of
options as a checklist.

```toml
[profile.widget.sambaAcctFlags]
kind   = "choice"
select = "multi"
format = "bracketed"
options = [
  { value = "D", label = "Disabled" },
  { value = "X", label = "Password never expires" },
  { value = "N", label = "No password required" },
]
```

### Options

- **`kind`** *(required)* — must be `"choice"`.
- **`select`** *(required)* — cardinality:
  - `"single"` — at most one option may be checked. The stored value is
    replaced by the chosen option (e.g. `loginShell`).
  - `"multi"` — multiple options may be checked simultaneously (e.g. Samba
    account flags).
- **`format`** *(required)* — how options are encoded in the LDAP attribute:
  - `"plain"` — the attribute value **is** the chosen option's `value`. Use
    this for simple string attributes like `loginShell`, where only one value
    is stored at a time.
  - `"bracketed"` — Samba `sambaAcctFlags`-style encoding: a fixed-width
    bracketed string of single-character flags, e.g. `[DUW        ]`. Letters
    not included in the widget's option list are **preserved losslessly** on
    every edit (see [Lossless behaviour](#lossless-behaviour) below).
  - `"bitmask"` / `"delimited"` — reserved for future use; eDAPtor will reject
    a config that specifies these.
- **`options`** *(required, non-empty)* — a list of `{ value, label }` records:
  - `value` — the token stored in the encoded value (a flag letter, a path, …).
  - `label` — the human-facing text shown in the checklist and in the
    read-only summary.

## Worked examples

### `loginShell` — single-select, plain format

```toml
[profile.widget.loginShell]
kind   = "choice"
select = "single"
format = "plain"
options = [
  { value = "/bin/bash",     label = "Bash" },
  { value = "/bin/sh",       label = "POSIX sh" },
  { value = "/sbin/nologin", label = "No login" },
]
```

The stored value is exactly the chosen `value` string. If the current value is
not in the list (e.g. `/usr/bin/zsh`), the raw value is shown as a fallback in
the read-only summary and the checklist starts with nothing pre-selected.

### `sambaAcctFlags` — multi-select, bracketed format

```toml
[profile.widget.sambaAcctFlags]
kind   = "choice"
select = "multi"
format = "bracketed"
options = [
  { value = "D", label = "Disabled" },
  { value = "X", label = "Password never expires" },
  { value = "N", label = "No password required" },
]
```

Samba stores account flags as a fixed-width bracketed string such as
`[UW         ]`. Each letter represents a flag; the `U` (normal account) and
`W` (workstation trust account) flags are maintained by Samba itself and are
**not listed in the widget options** — they will be preserved untouched.

## Lossless behaviour

When you save changes via a choice widget, eDAPtor:

1. **Parses** the current attribute value into the full token set.
2. **Applies** only the options known to the widget: checked options are added,
   unchecked options are removed.
3. **Preserves** any tokens *not listed in the options* exactly as they were.

For bracketed format this means the `U` (normal-user) and other Samba-internal
flags survive an edit even though they do not appear in the checklist. For plain
format there is only one token so lossless preservation is not relevant.
