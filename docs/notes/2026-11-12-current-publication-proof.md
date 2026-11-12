# Current publication proof for recovery after later progress

Owner: S2 API/schema/publisher; S4 invariant confirmation and service caller.
Published narrow source pin: `c4e4b995a`, branch
`tl/cma-log-segments/current-publication-proof`. S4 caller target: 06:30 SGT.
S5 owns current activation domain/native helpers and restart/loss qualification.
S4 confirmed the invariant in `281f92000`; caller source is published at
`2dbb5d81c` and consumed by S5. Actual combined caller qualification is pending.
This is a dependency contract, not P1-3 closure or permission to activate writers.

## Frozen API

Module: `crate::store::agent_log_segments::current_publication`.

```rust
pub(in crate::store) struct CurrentPublication { /* opaque */ }

pub(in crate::store) async fn capture(
    store: &MongoSandboxStore,
    current: &Agent,
    scope: &mut TransactionScope<'_>,
) -> Result<CurrentPublication, TransactionLogError>;

impl CurrentPublication {
    pub(in crate::store) fn require_current(
        &self,
        store: &MongoSandboxStore,
        current: &Agent,
        scope: &TransactionScope<'_>,
    ) -> Result<&Publication, SandboxStoreError>;
}
```

The proof is non-cloneable and has no public constructor. It binds the complete
current Agent, database and exact transaction identity. A new transaction on the
same session needs a new proof. It is evidence in the current snapshot: callers
must not reuse it after their own writes or across attachment/native IO. Capture
again with fresh authority before acknowledgement, as the existing route does.

## Publisher and schema

`Publication::new` computes the canonical full-Agent witness from `next` itself.
The caller's `result_digest` retains its domain-specific meaning. Publication
format 2 adds `agent_digest`, a 32-byte generic BSON binary in the independent
`cma.agent-log.current-agent.v1` domain. Existing format-1 publications remain
readable with no such field; they cannot supply positive current-state proof.
The original format-2 recovery carrier, original result witness, request key,
result timestamp and completion receipt are not rewritten or synthesized.

The canonical additive schema registers unique
`agent_log_publication_head(agent_id, incarnation, head_revision)`. Shared
publication increments the head for every write, including revision-only work.
The proof captures S1's certified segmented source in the caller's transaction,
then performs one explicitly hinted newest-first lookup at or below the captured
head revision: limit 2, batch size 2, single batch,
maximum 1 second and 16 KiB per receipt. The newest publication must be format 2 and
match current Agent revision, sequence, incarnation, ordinal,
previous seal and full Agent digest. The second row must have a strictly earlier
head revision. Audit rebuilds only advance head revision/evidence generation, so
a positively matching current Agent/source witness remains valid after rebuild.
A newer incompatible/v1 publication is never skipped in favor of an older one.
No lifetime publication enumeration occurs.
Missing/old/ambiguous evidence or a missing index refuses; it grants no ordinary
reconnect permission and never establishes KnownAbort.

This extends the still-unactivated segmentation schema. The release-owner reader
floor and rollback artifact must include this pin before first writer admission;
a prior reader that only understands publication format 1 is not a rollback
candidate after the first format-2 write. Keep writer/recovery admission closed
until canonical schema/runtime-role verification and the combined gates pass.
An already-installed incompatible draft validator is refused; do not drop,
recreate, silently relax or auto-migrate it at startup. No production DDL here.

## S5 consumption and remaining responsibility

In the same TransactionScope, obtain fresh personal resource/parent authority,
verify the unchanged original completed carrier/receipts using `receipts::read`,
and capture current publication proof. Require that proof against the freshly
read current Agent. Compare the verified current receipt with the original
completion; revision growth alone is insufficient. Preserve same incarnation,
conversation/generation, runtime/home identity and parent lifecycle constraints.

Current native target/status/turn can differ after legitimate ordinary progress.
S5 must qualify a current activation or positively proven ordinary route without
mutating the original RecoveryWork target. Do not merely remove the existing
revision equality or reuse its original active turn, input hash or writer fence.
No input replay, new recovery admission, policy bypass or invented native fence.
Fresh authority and current native confirmation remain mandatory. The proof API
does not claim that the production native adapter supports that confirmation.

## Qualification

New tests cover complete Agent binding, v1/v2 shape, missing/foreign evidence,
actual shared-publisher later progress plus store reopen, original receipt
immutability, same-session later-transaction refusal, foreign database/session,
abort, canonical unique-index enforcement and ambiguity refusal under drift.
S5 owns the actual restart/loss/acknowledgement and current native-target matrix.
Gate results and final pin are recorded after execution; no inferred pass.
