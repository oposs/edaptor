# Full Example

This is the complete annotated `examples/config.toml` — a copy-pasteable
starting point that exercises every supported option. Replace the
`dc=example,dc=com` base, the object classes, and the search bases with whatever
your directory actually uses; eDAPtor introspects `cn=subschema`, so the forms
adapt to your schema automatically.

```toml
# edaptor configuration reference
# ================================
# A single TOML file declares the LDAP connection, how to authenticate, and a
# set of "entry profiles" describing what a user / group means in your directory.
# Pass it with `edaptor --config <path>` (default: ~/.config/edaptor/config.toml).
#
# This file exercises every supported option and is safe to copy as a starting
# point. Replace the dc=example,dc=com base and the object classes with whatever
# your directory actually uses (edaptor introspects cn=subschema, so the forms
# adapt to your schema automatically).

[server]
uri          = "ldaps://ldap.example.com"   # ldap:// or ldaps://
base_dn      = "dc=example,dc=com"
start_tls    = false                          # true upgrades an ldap:// connection; do NOT combine with ldaps://
read_only    = false                          # true disables all write actions in the TUI
timeout_secs = 10                             # bound the TCP connect so an unreachable server cannot hang

# Optional TLS trust settings. Omit the whole table to use the system trust store
# with full verification.
[server.tls]
# ca_cert = "/etc/ssl/certs/my-ca.pem"        # trust a custom CA (PEM)
verify    = true                              # set false ONLY for testing — accepts any certificate

[auth]
method          = "simple"                    # simple bind (SASL EXTERNAL/GSSAPI are a later milestone)
bind_dn         = "cn=ldapmanager,dc=example,dc=com"
# The password is NEVER stored in this file. Choose a source:
#   "prompt"            -> ask interactively at startup (no echo)
#   "env:VAR"           -> read environment variable VAR
#   "command:some cmd"  -> run a command and read its stdout
password_source = "prompt"

# ---------------------------------------------------------------------------
# Entry profiles: what a "user", "group", and "posixgroup" mean here.
# ---------------------------------------------------------------------------
# `search_attrs` sets which attributes the picker substring-search matches;
# it falls back to `show`, then to ["cn"] when omitted.
#
# This "user" is a full posix (+optional Samba) account template: multiple object
# classes, defaulted/templated/auto-numbered fields, a set-password popup,
# and picker bindings that pull values from (or fan out to) other profiles.
[[profile]]
name           = "user"
object_classes = ["inetOrgPerson", "posixAccount", "shadowAccount"]
rdn_attr       = "uid"
search_base    = "ou=people,dc=example,dc=com"
show           = ["uid", "cn", "sn", "givenName", "mail", "userPassword", "uidNumber", "gidNumber", "homeDirectory"]
search_attrs   = ["cn", "uid", "mail"]        # picker searches these attributes
# How an entry of this profile is labelled in the membership picker. `{attr}` is
# substituted by that attribute's value; literal text is kept. Defaults to cn.
label          = "{cn} ({uid})"               # e.g. "Bob Baker (bob)"

# Defaults fill EMPTY fields on create (operator-entered values are never
# overwritten). Value kinds:
#   literal             -> a fixed string
#   "/home/{uid}"       -> template; {attr} is substituted from another field
#   "{next:MIN-MAX}"    -> auto-number; the next free value in [MIN,MAX] across
#                          the whole directory (refuses if the scan is truncated
#                          by a server size limit — bind with a high-limit identity)
#   "{auto:sambaSID}"   -> computed; sambaSID derived from uidNumber + the Samba
#                          domain, filled once uidNumber resolves
[profile.defaults]
loginShell    = "/bin/bash"
homeDirectory = "/home/{uid}"
uidNumber     = "{next:10000-60000}"
sambaSID      = "{auto:sambaSID}"

# Widget bindings: `[profile.widget.<attr>]` declares a rich in-line widget for
# an attribute's field.
#
# AUTO-CONFIG: edaptor ships baked-in widget defaults for standard LDAP schemas
# (posixAccount, posixGroup, inetOrgPerson, sambaSamAccount, groupOfNames,
# groupOfUniqueNames, OpenLDAP cn=config). Many entries below can be omitted for
# typical deployments; they are shown here for documentation purposes or to
# demonstrate how to override the defaults. See docs/configuration/widgets.md.

# Password widget: opens a masked set-password popup. Auto-configured for
# inetOrgPerson/posixAccount (samba=false). Only needed here to:
#   - enable Samba hash sync (samba = true, requires sambaSamAccount OC), or
#   - override the auto-assigned widget with different settings.
# Requires an encrypted connection (ldaps:// or start_tls = true).
[profile.widget.userPassword]
kind  = "password"
samba = false   # set true to also write sambaNTPassword/sambaPwdLastSet

# Choice widget: auto-configured for posixAccount.loginShell with a default
# shell list (bash, sh, zsh, false, nologin). Declare it here only to customise
# the visible options.
[profile.widget.loginShell]
kind   = "choice"
select = "single"
format = "plain"
options = [
  { value = "/bin/bash",     label = "Bash" },
  { value = "/bin/sh",       label = "POSIX sh" },
  { value = "/sbin/nologin", label = "No login" },
]

# Picker widget: `[profile.widget.<attr>]` with `kind = "picker"` populates an
# attribute from a live candidate search. Key options:
#   candidate   (required) — a [[profile]] `name` (or sentinel: _posix_group_,
#                 _posix_account_, _any_) supplying the candidate search scope.
#   store       (default "dn") — "dn" stores the candidate's DN; any other value is
#                 an attribute name whose scalar is stored.
#   select      (default "auto") — cardinality: "auto" derives from the attribute's
#                 schema arity; "single" or "multi" override it.
#
# Membership widget: `[profile.widget.<attr>]` with `kind = "membership"` fans
# this entry's DN into `via` on each picked candidate. The field itself is
# overlay-maintained and never written directly by edaptor.

# gidNumber: auto-configured for posixAccount (picks the first posixGroup profile
# via the _posix_group_ sentinel). Declare here to pin the candidate to a
# specific profile name or to customise store/select.
[profile.widget.gidNumber]
kind      = "picker"
candidate = "posixgroup"
store     = "gidNumber"
select    = "single"

# memberOf: the auto-config marks memberOf as `readonly` (overlay-maintained).
# Declare it as `membership` here to make it an interactive group-membership
# manager instead — ticking a group writes `member` on it.
[profile.widget.memberOf]
kind      = "membership"
candidate = "group"
via       = "member"

# Readonly widget: use `kind = "readonly"` for attributes that should be
# displayed but never edited (e.g. custom overlay-maintained attributes not
# covered by the baked-in schema). memberOf, sambaNTPassword, sambaLMPassword,
# and any NO-USER-MODIFICATION attribute are readonly by default without any
# explicit config.
# [profile.widget.myOverlayAttr]
# kind = "readonly"

# X-ORDERED widget: auto-configured for olcAccess, olcDbIndex, etc. on
# olcGlobal/olcDatabaseConfig entries. Use for your own X-ORDERED attributes.
# [profile.widget.myOrderedAttr]
# kind = "x_ordered"

# Companion entry: `[profile.companion]` declares a second entry eDAPtor creates
# alongside the primary on New — here, a user-private posixGroup. `attributes`
# templates resolve against the PRIMARY's final attributes ({next:...} autonumbers
# are not allowed in a companion); `rdn_attr` must be one of those attribute keys.
# Created atomically with the primary via LDAP transactions (RFC 5805) when the
# server supports them, otherwise companion-first with the primary aborted on
# failure. See docs/configuration/companion.md.
[profile.companion]
object_classes = ["posixGroup"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=com"

[profile.companion.attributes]
cn        = "{uid}"        # user-private group: same name as the login
gidNumber = "{gidNumber}"  # mirrors the user's already-allocated gid
memberUid = "{uid}"

[[profile]]
name           = "group"
object_classes = ["groupOfNames"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=com"
show           = ["cn", "description"]
label          = "{cn}"

# member: multi-select DN picker over users (cardinality from schema, typically multi).
[profile.widget.member]
kind      = "picker"
candidate = "user"

[[profile]]
name           = "posixgroup"
object_classes = ["posixGroup"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=com"
show           = ["cn", "gidNumber", "memberUid", "description"]
label          = "{cn}"

# memberUid: multi-select picker; stores each picked user's `uid` scalar (not DN).
[profile.widget.memberUid]
kind      = "picker"
candidate = "user"
store     = "uid"
```

## Walk-through

The sections below mirror the file top to bottom. Each links to the page where
that table is documented in full.

### `[server]` and `[server.tls]`

The `[server]` table sets the LDAP URI, base DN, StartTLS/read-only flags, and
connect timeout; the optional `[server.tls]` table adds a custom CA and the
`verify` switch. See [Server & Authentication](server-auth.md).

### `[auth]`

A simple bind with `bind_dn` and a `password_source` that is never the password
itself (`prompt`, `env:VAR`, or `command:cmd`). See
[Server & Authentication](server-auth.md).

### The `user` profile

A full posix (and optional Samba) account: multiple object classes, an `uid`
RDN, a `label` template, and three sub-tables —
[`[profile.defaults]`](defaults.md) (literal `loginShell`, templated
`homeDirectory`, auto-numbered `uidNumber`),
[`[profile.widget.userPassword]`](widgets.md#password) (overrides the
auto-configured password widget; only required when `samba = true`),
a [`[profile.widget.loginShell]`](widgets.md#choice) choice override (the
baked-in default already works; declared here to show a custom shell list),
a [`[profile.widget.gidNumber]`](widgets.md#picker) picker binding (auto-configured
via the `_posix_group_` sentinel; shown here to pin it to the `posixgroup` profile
by name), and a [`[profile.widget.memberOf]`](widgets.md#membership) membership
binding that overrides the default `readonly` treatment to allow interactive
group management. See [Entry Profiles](entry-profiles.md) and
[Widgets](widgets.md).

A [`[profile.companion]`](companion.md) block additionally creates a user-private
`posixGroup` alongside every new user — `cn` mirrors the user's `uid`, `gidNumber`
mirrors the user's already-allocated gid, and `memberUid` seeds the user as its
sole member. See [Companion Entries](companion.md).

### The `group` profile

A `groupOfNames` group whose `member` attribute is filled by a multi-select
[picker widget](widgets.md#the-picker-kind) over the `user` profile (storing
DNs). See [Entry Profiles](entry-profiles.md).

### The `posixgroup` profile

A `posixGroup` whose `memberUid` attribute is filled by a multi-select
[picker widget](widgets.md#the-picker-kind) over the `user` profile, storing
each user's `uid` **scalar** rather than a DN. Its `gidNumber` is what the
`user` profile's `gidNumber` picker widget consumes. See
[Entry Profiles](entry-profiles.md).
