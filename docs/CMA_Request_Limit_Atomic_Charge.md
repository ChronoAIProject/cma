# Atomic shared request-budget charge

Request admission uses the existing MongoDB `request_rate_limits` collection and
server-clock GCRA. For interval `ceil(60000 / max(rate, 1))`, a charge of `tokens`
is eligible exactly when the previous theoretical arrival time is at most
`now + interval * (rate - tokens)`. Admission advances it to
`max(previous, now) + interval * tokens`. Exhaustion leaves the stored time and
expiry unchanged and returns the ceiling of the wait in seconds. Invalid token
counts fail before contacting MongoDB.

One conditional `findAndModify` upsert both creates and charges a missing bucket.
Its preimage projection returns the previous budget's wait; a missing preimage
means the insertion admitted the charge. Duplicate-key first-insert races retry
within the existing eight-attempt bound. Database errors and retry exhaustion
refuse admission; neither grants a process-local allowance. Existing lease policy
and majority write concern are unchanged.

Both expressions must use the same command-scoped `$$NOW`. The disposable
MongoDB 8.0.13 contract delays the update after its clock evaluation by 75 ms,
then checks the before-image projection's clock against the persisted arrival
time and expiry. The exact production expressions are also replayed with one
literal clock input to compare the previous implementation at the boundary,
1 ms past it, rounding boundaries, and multi-token admission.

## Compatibility and rollout

This changes no collection, index, grant, validator, field format or reader floor.
The existing schema marker remains Int32 version 1; retained records without it
and with Int32 arrival times remain readable. No decision sidecar is persisted.
Old and new writers serialize against the same arrival time, including during
rollback and subsequent upgrade. No backfill, recovery artifact or data reversal
is required; rollout and rollback select the API image. This is a shared admission
implementation change, independent of the default-off Agent-list batching flag.

## Required evidence

Run `bash .github/scripts/mongo-contract-tests.sh --request-limits` for the pinned,
owned disposable replica set. Existing ordinary/full runner selections retain
these tests. Qualification includes concurrent fresh/warm keys across independent
clients, duplicate-key and database failures, legacy rows, mixed writers and
rollback/upgrade, clock equality, boundary parity and complete command counts.
Count every command-start event, including retries and getMore. Expected normal
counts per check are one for a fresh key, one for warm admission, and one for
warm exhaustion; the previous implementation uses four, one and two respectively.
Injected or actual retries are additional commands and must be reported.
