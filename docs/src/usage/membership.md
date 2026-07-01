# Membership Editing

Group membership in eDAPtor is **symmetric**: a group's `member` attribute and a
user's `memberOf` attribute are two views of the *same* relationship. You can
add Alice to the *Admins* group either by editing the group (add Alice to its
`member`s) or by editing Alice (add *Admins* to her `memberOf`s) — both write the
identical underlying link.

## Two views of one relationship

Each side is just an attribute field driven by a widget in the
[widget palette](../configuration/widgets.md):

- On a **group** entry, the `member` field uses the
  [`picker` widget](../configuration/widgets.md#the-picker-kind) over `user`
  candidates. Each pick stores a user's DN in the group's `member` attribute (a
  normal multi-valued attribute the group owns).
- On a **user** entry, the `memberOf` field uses the
  [`membership` widget](../configuration/widgets.md#the-membership-kind) over
  `group` candidates. `memberOf` is **overlay-maintained** by OpenLDAP (the
  `memberof` overlay) — eDAPtor must never write `memberOf` directly. So this
  field is configured as a fan-out: ticking a group does not write `memberOf` on
  the user; instead it adds (or removes) the user's DN in that group's `member`
  attribute (the `via` key).

The result is one consistent edit no matter which entry you start from. See
[Widgets](../configuration/widgets.md) for the `[profile.widget.<attr>]`
configuration behind both sides, including the
[`membership` kind](../configuration/widgets.md#the-membership-kind) that makes
the `memberOf` view write the link onto the chosen groups.

## Editing memberships

1. Select the group (or user) in the **Entries** pane to load it into the form.
2. Open the membership field (`member` on a group, `memberOf` on a user) with
   `↵`. A picker overlay opens, listing the candidate entries from the linked
   profile.
3. **Incremental search:** type while the candidate (Available) list is focused
   to search. The query matches against each candidate's rendered profile
   **label** (e.g. `Bob Baker (bob)` from `label = "{cn} ({uid})"`), not just its
   raw `cn`, so you can find people by any attribute the label includes — `uid`,
   `mail`, and so on. Because the candidate set comes from the directory, the
   query re-runs the LDAP search (it is not limited to an already-loaded page).
   The same type-to-find applies when browsing entries in the main **Entries**
   pane (there it filters the already-loaded list in place).
4. Toggle candidates in or out of the membership set, then accept the picker.
5. Save the form (`Alt+S`). The change goes through the usual LDIF
   preview → apply flow.

## The fan-out write model

When you edit the **`memberOf`** view of a user, the form field is *not* written
to the server at all. Instead, for every group you ticked or unticked, eDAPtor
adds or removes that user's DN in the group's `member` attribute. OpenLDAP's
`memberof` overlay then keeps the user's `memberOf` values in sync automatically.

This fan-out is the general mechanism behind any
[`membership` widget](../configuration/widgets.md#the-membership-kind), not just
`memberOf` — see [Widgets](../configuration/widgets.md) for the full
configuration and for the other widget kinds (DN vs. scalar storage,
single vs. multi select).
