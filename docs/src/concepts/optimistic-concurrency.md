# Optimistic Concurrency

Two people can open the same entry at the same time. Without protection, the
second save wins outright and silently erases the first — a classic **lost
update**: neither editor sees a conflict, and no error tells the loser that
their change disappeared. eDAPtor closes that gap by making every save (and
delete) conditional on the entry still being the version you started from.

## `entryCSN` as the version token

Every write needs something to compare "the version I read" against "the
version on the server right now." eDAPtor uses OpenLDAP's `entryCSN` — an
operational attribute the server stamps on an entry each time it changes,
encoding a timestamp, a server ID, and a per-server change counter.

`entryCSN` is deliberately preferred over `modifyTimestamp`: `modifyTimestamp`
has one-second resolution, so two changes to the same entry within the same
second are indistinguishable by timestamp alone. `entryCSN` is monotonically
increasing and unique per change, so it never collapses two different edits
into the same value.

When eDAPtor reads an entry into the edit form, it captures that entry's
current `entryCSN` alongside the field values. This captured value is the
**baseline** — the version the form believes it is editing.

## Assert-then-write

On save (Modify) or delete, eDAPtor does not just send the operation — it
attaches an RFC 4528 **Assertion control**, `(entryCSN=<baseline>)`, marked
**critical**. A critical control tells the server: if you don't understand
this control, or the asserted filter does not match the entry as it stands
right now, reject the whole operation rather than applying it anyway.

For modifies, eDAPtor also attaches an RFC 4527 **Post-Read control**, so a
successful write returns the entry's *new* `entryCSN` in the same round trip —
that becomes the baseline for the next edit, without a second read.

If nobody else touched the entry, the assertion matches, the write applies,
and eDAPtor moves on having spent one extra control on the wire. If someone
else's write landed first, the entry's live `entryCSN` no longer matches the
baseline, the assertion fails, and the server returns result code 122
(`assertionFailed`) instead of applying your change. The other client's write
is never clobbered, and yours is never silently dropped either — you get a
definite answer.

## Rebase silently, or ask

A 122 on its own only says "something changed" — it doesn't say whether that
something matters to *your* edit. So on a conflict, eDAPtor re-reads the entry
and compares the attributes the other client's change touched against the
attributes your edit touches:

- **No overlap** — the other change and yours are about different attributes
  (someone updated a phone number while you changed a group membership, say).
  eDAPtor rebases automatically: it re-baselines onto the fresh `entryCSN` and
  resubmits your edit against it, silently. You never see this happen; it just
  works.
- **Overlap** — the other change touched at least one attribute you also
  edited. There is no safe automatic resolution here, so eDAPtor stops and
  opens the **"Entry changed"** dialog with three choices: **Reload** (discard
  your edits and re-read the current entry — the safe default), **Overwrite**
  (re-assert against the fresh version and force your values through anyway),
  or **Cancel** (keep editing, decide later). eDAPtor never picks a winner for
  you when the same attribute is in play on both sides.

The same assert-and-reconcile logic applies to every leg of a combined save —
including the per-group modifies fanned out for a
[membership](../configuration/widgets.md#the-membership-kind) change and the
edited entry's own leg — so a multi-operation save is protected consistently,
not just its primary write.

## Capability fallback

Because the assertion is sent *critical*, it only works against a server that
understands it — sending it to a server that doesn't would fail every single
write with result code 12 (`unavailableCriticalExtension`), which would make
eDAPtor strictly worse than before on such a server.

To avoid that, eDAPtor probes the root DSE's `supportedControl` list at
connect time for the Assertion control OID `1.3.6.1.1.12`. If the server
advertises it, optimistic concurrency is active for the whole session. If it
does not, eDAPtor falls back to the previous **blind-write** behaviour — no
assertion is attached, and a write simply overwrites whatever is there — and
shows a **one-time** status-line warning that concurrent edits on this server
may be lost. The warning fires once per session, not on every save, so it
informs without nagging.
