# DIT Tree Labels

The DIT navigation pane (pane 1) labels each branch node. By default eDAPtor
keeps the structural **RDN** visible and appends a human name when present:

- `{rdn} ({cn})` when the node has a `cn`,
- else `{rdn} ({description})` when it has a `description`,
- else just `{rdn}` (e.g. `ou=people`).

So `ou=people` carrying `description: People` renders as **`ou=people (People)`** —
the container's identity is never dropped.

## Configuring labels

Override the defaults with an ordered list of `[[tree.label]]` rules. The first
rule whose `when` attributes are **all present** (non-empty) wins; a rule with no
`when` is the unconditional fallback:

```toml
[[tree.label]]
when     = ["description"]
template = "{rdn} ({description})"

[[tree.label]]
template = "{rdn}"            # fallback: no `when` → always matches
```

- **`when`** (default `[]`): attribute names that must all be present. Matching is
  case-insensitive. An empty/omitted `when` always matches. (The reserved `rdn` is
  always considered present.)
- **`template`**: reuses the `{field}` substitution from entry labels, plus the
  reserved **`{rdn}`** token (the node's relative DN, e.g. `ou=people`). An unknown
  `{field}` renders empty.

If `[[tree.label]]` is omitted entirely, the built-in default rule set above is
used.

## Narrow panes (the truncation ladder)

When the pane is too narrow, eDAPtor trims the **rightmost** segment first and
drops a segment whole once its templated value is consumed — so the RDN (leftmost)
survives longest. For `{rdn} ({description})` on `ou=people` / `description=People`:

```
wide   ou=people (People)
        ou=people (Peop…)     trim the description in the last segment
        ou=people (P…)
        ou=people             description consumed → drop the "(…)" segment
narrow  ou=peop…              only the RDN segment left → ellipsize it
```
