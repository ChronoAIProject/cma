# Runtime mutation ledger capacity

Runtime mutation ledgers retain V1 JSON records and exact mutation receipts.
They have finite lifetime capacity: new distinct work is refused before it can
make a healthy ledger unreadable, with capacity reserved for cleanup of
runtime-admitted resources.

## Reservations and authority

Admission validates the persistent root and active runtime-process generation
root together under the supervisor's serialized operation gate. The journal
budget includes every canonical receipt (Completed, Failed and Indeterminate),
missing receipts for admitted authority, future Close/Remove receipts for each
admitted resource, and one physical terminal replacement slot. Historical skills
also reserve their parent AgentHome removal when needed. This budget remains
within the 1,024-file scan limit.

Authority retains the maximum Present and first/maximum Absent records for each
resource. Its budget reserves the applicable Present and both Absent records,
plus physical append headroom within the 4,096-record scan limit. Newer records
and directories are synced before redundant predecessors are deleted; those
deletions are synced before another append. Full history validation precedes
folding. High-water fences, exact identities and cleanup tombstones are retained;
API lease fences still advance.

Both PrepareAgentResources intents are checked before either authority write
or effect. A partial publication keeps its reservations; retries check the full
pair again. Failed or ambiguous work does not release capacity. Filesystem
tombstones survive runtime generations; old process generations are not folded
into current process authority.

## Recognizing a capacity hold

The fixed wire refusal is code `mutation_ledger_capacity`, detail
`runtime mutation ledger capacity is exhausted`, and `retryable=false`.
A pre-admission refusal is not stored as a terminal mutation receipt. Admission
diagnostics log canonical journal count, missing receipts, cleanup reservations,
physical authority count, reserved authority count and physical headroom.
Credentials and receipt content are not logged.

Compatible API handling distinguishes capacity holds from corruption, runtime
loss and owner quota. An unsafe legacy ledger remains held without increasing
scan limits or pruning canonical state to force admission. Malformed records and
conflicting replacements remain corruption. Do not delete receipts or tombstones
to clear a hold, and do not treat it as evidence that the runtime has been lost.

Startup validates the whole active V1 history. Bounded global scans also remain
on admission, recovery and process checks. Receipt replay uses the existing
hashed operation path, with a bounded filename walk for replacement temporaries;
any replacement requires full validation and reconciliation. Full identity,
payload, no-follow IO and completed-open liveness checks remain in force.

## Limits, rollout and rollback

Cleanup and exact replay retain capacity for runtime-admitted resources. Cleanup
for an unknown resource requires its own space. The API records cleanup debt
before dispatch, so a request refused before runtime admission can still leave
API cleanup debt; its later Close/Remove can also be held when full. A refusal
cannot serve as a receipt or prove that this cleanup is complete.

Completed receipts are not reclaimed. No fixed additional number of Agent
lifecycles is guaranteed. These limits do not repair already-poisoned homes,
remove the separate workspace-journal limit, or ensure progress on a full or
failed filesystem. New admission may stop earlier to preserve cleanup capacity.

All journal receipt bytes remain retained. **Rollback is V1 data-format
compatible but restores the old capacity and per-fence amplification bugs.**
The compatible API is the supported operational floor for capacity holds; an
older API can fail an Agent startup rather than schedule the intended hold.

Existing Sandboxes keep their running image. A merge, API deployment or image
publication does not install this protection in a running runtime. Image
publication, default-pin activation and eligible runtime replacement are separate
operations. Preserve the owner-interruption and retained-identity requirements
in `CLAUDE.md` and follow `docs/Release.md` for release activation.
