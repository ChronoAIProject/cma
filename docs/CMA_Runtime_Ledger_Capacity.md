# Runtime mutation ledger capacity

Part of #1443. This V1 containment change prevents new mutation admission from
making the runtime's bounded journal unreadable. It preserves the existing JSON
format and receipts. It is not the later indexed-store migration or permission
to replace a user's runtime.

## Reservation rule

Under the supervisor's serialized operation gate, inspect and validate the
persistent root and active runtime-process generation root together. Count:

- Every canonical journal entry, including Completed, Failed and Indeterminate.
- Every admitted authority identity with no journal yet.
- One future Close/Remove journal for each admitted resource without its cleanup
  identity. A skill also reserves its parent AgentHome removal when needed.
- One physical journal slot for a terminal replacement temporary file.

The sum must not exceed the unchanged 1,024-file scan limit. Authority reserves
one maximum Present and two Absent records (first and maximum) per applicable
resource, plus a physical append slot under its unchanged 4,096-record bound.
Both original intents in PrepareAgentResources are checked before any authority
write or effect. A partial authority publication retains the reservations of
that published intent; every retry checks the complete pair again. No second
effect is authorized by a partially published pair.

Reservations are derived from validated V1 records, not a new counter file.
Unknown-resource cleanup needs its own space. Ambiguity does not release space;
cleanup admission alone does not prove process closure. A fully reserved healthy
ledger still permits its accepted cleanup and exact replay. New distinct work
is refused before its authority, journal or effect is admitted.

## Authority folding and replay

For each resource, retain the maximum Present and first/maximum Absent when
those records are distinct. Publish the newer record through the existing
file-sync, hard-link and directory-sync path before deleting predecessors.
Before pruning, sync all retained records and their directories again, including
when recovering a prior ambiguous publication. The affected directories are
synced after removing predecessors and before admitting another append. A crash may leave redundant valid records, but never
requires forgetting the high-water fence or cleanup barrier.

Validate the entire history before folding; a later valid record cannot hide
an earlier identity conflict or a create after cleanup. API lease fences still
advance. Filesystem tombstones survive runtime generations; old process
generations are not folded into current process authority.

## Capacity holds and compatibility

The fixed wire refusal is code `mutation_ledger_capacity`, detail
`runtime mutation ledger capacity is exhausted`, and `retryable=false`.
It is not stored as a terminal mutation receipt. Admission diagnostics log
canonical journal count, missing receipts, cleanup reservations, physical
authority count, reserved authority count, and each physical headroom allowance.
No credentials or receipt content are logged. Compatible API handling distinguishes
this hold from corruption, runtime loss and owner quota.

An already-unsafe legacy ledger is held without increasing scan limits or
pruning its canonical state to force admission. Malformed records and conflicting
replacements remain corruption. Normal startup validates the whole active V1
history; bounded global scans still run on mutations. No overall logarithmic
complexity is claimed.

A frozen predecessor authority reader tests the retained V1 representation,
maximum fence, exact identities and Absent barrier. All journal receipt bytes
remain retained. **Rollback is data-format compatible but brings back the old
capacity and per-fence amplification bugs.** The compatible API is the supported
operational floor for capacity holds; an older API can fail an Agent startup
rather than schedule the intended hold.

## Limits and rollout

This is finite-lifetime containment. It does not reclaim completed receipts,
guarantee another fixed number of Agent lifecycles, repair already-poisoned
homes, remove the separate workspace-journal limit, or guarantee progress on a
full or failed filesystem. Near capacity, new admission stops earlier so cleanup
can finish. This guarantee covers runtime-admitted resources. The API records
cleanup debt before dispatch (`agent_worker/support/effects.rs`), so even a
request refused before runtime admission can leave API cleanup debt. Its later
Close/Remove is an unknown-resource mutation and can also be held when full;
this patch cannot infer completion or clear that debt without a receipt. The
later indexed store and reviewed recovery procedure remain part of #1443.

Existing Sandboxes keep their running image. A merge, API deployment or image
publication does not install this protection there. Image publication, default
pin activation and any eligible owner-consented runtime replacement are separate
lead decisions. An affected user's runtime must not be restarted by this PR.
