# S4 writer implementation checkpoint — 2026-11-11

Part of #1350 and transactional #1349. Branch:
`tl/cma-log-segments/writer-publication`. Develop merge freeze remains active.
This is an implementation-source handoff, **not a review-ready PR or an
executable atomic writer**. New modules remain outside the shared registries.
No writer activation, recovery, production mutation or develop merge occurred.

## Source delivered

| Commit | Owned implementation |
| --- | --- |
| `898a5a824` | Strict completed-publication schema and canonical request/result digests; stable append time in admission comparison; exact receipt reconciliation with explicit Unknown on absence, retained event verification and later-head support. |
| `18b9924f0` | Reused incremental projection schema/fold; immutable turn model/native binding, per-turn usage references and one retained completed turn; bounded/coalesced projection delta with final-state validation. |
| `60396a2d6` | Deterministic creation-bound cohorts, separate explicit recovery policy, bounded durable preparation slots, renewal/replacement/expiry fencing, operator CAS parsing/control and focused gate/Mongo test source. |

These build on the published interface `7c87e6657` and preserved six planner/gate
drafts. The original whole-batch BSON rollover planner remains the starting
point for publication. Neither `writer_commit` nor `writer_bootstrap` is wired;
none of the nine insertion families is claimed converted by this checkpoint.

## Concrete schema and API handoff

- `PublicationIdentity` now requires `key`, generic 32-byte `fingerprint` and
  original RFC3339 `appended_at`. Admission and completed receipt both retain
  and compare the timestamp. Admission time remains a separate field.
- `Publication::read` requires int64 revision/sequence counters. `writer_reconcile`
  returns `Recorded(Box<Publication>)` or `Unknown`; it cannot infer KnownAbort
  from absence. The caller still verifies its related domain/effect receipts.
- `Policy` has required `enabled`, `agents`, `owners`, `cohort_seed`,
  `basis_points`. Writer cohorts use domain-separated SHA-256, fixed big-endian
  encoding and a creation-bound bucket in `[0,10000)`. Once set, the seed cannot
  change through policy CAS. Close preserves it. Recovery rejects owner and
  percentage selection, including a dormant inherited percentage policy.
- `GateRecord` adds a required bounded `slots` array. A slot carries original
  operation holder/fingerprint, monotonic fence and BSON expiry time. At most
  two adoption slots or one independent recovery slot exist. Renewal preserves
  the preparation fence; reclaim issues a new fence. Final consumption must
  occur inside the publication transaction and checks server time. Closure
  preserves exact accepted work. Fitting appends do not touch this document.
- Turn facts add required nullable `model`, exact `conversation_id` and
  `runtime_generation`, required nullable `usage` and `usage_sequence`. Unknown
  historical model remains null. Usage may arrive after terminal publication;
  one scalar `latest_completed` turn permits the exact retained update without
  scanning history. The original model/native identity remains unchanged.
- `AgentLogEvidenceChange::Turn` and `EvidenceRecord::Turn.facts` use boxed turn
  facts; the serialized representation stays an object. S3 adapters should
  consume the typed facts directly and unbox when returning an owned value.
- `ProjectionDelta` validates source/result binding and at most 64 proposed
  events, then coalesces by evidence ID. It validates final evidence after the
  entire batch, allowing TurnStarted + OwnerMessage intermediate states.
  Projection/evidence documents have a 256-KiB planning bound.
- `command.rs` parses gate status/replace/close only. S2's shared CLI must verify
  current operator authority, schema and topology before `writer_control` entry.
  It deliberately contains no direct migration bypass or service-startup hook.

## Validation and pending gates

Passed at this checkpoint: workspace `cargo fmt --all --check`, explicit pinned
Rust 1.96.1 formatting of every new unregistered module, `git diff --check`,
repository file-size check, and staged secret scans. All S4 source/test files
are below 500 lines, including the new focused test children.

New test source covers deterministic/monotone cohorts, strict receipt fields,
request/result digest separation, recovery isolation, bounded/reclaimed/expired
Mongo leases, preserved acceptance after gate closure, immutable model/native
facts across a 30,000-event turn, terminal/late usage, invalid steering,
coalescing within a batch and metadata-only publication. These tests have **not
executed**. Cargo/clippy, targeted units and canonical Mongo atomicity/crash/
compatibility gates remain pending. Prior assessment passes do not certify this
checkpoint. A full-history secret scan is also required after the final commit.

## Exact dependencies before caller integration

S1 must publish a compiling common-base commit with the Head/Seal/raw digest
implementation, segment child registrations and validated captured-source port.
The capture needs exact aggregate revision/global sequence, format/incarnation,
active raw leaf proof, validated semantic/projection state and quarantine state.
Legacy bootstrap needs the fenced, complete staged-evidence generation plus
source-CAS proof; counters alone cannot certify it. S4 will consume that proof
in `prepare` and own the single Agent CAS in `persist`, as already contracted.
No duplicate capture/audit framework is introduced here.

S2 must publish core/store/test registrations, collection handles and operator
entry-point authority/grant/migration plumbing. The gate test fixture now expects
the canonical segment migration to create gates/admissions, rather than creating
unregistered collections ad hoc. S4 also awaits its assigned heavy-build lock;
no Cargo build or Mongo fixture has been started without that assignment.

S3 must publish serializable/equatable `UsageFold` without changing its current
semantics, and the wrapper around existing native-identity validation used by
`agent_log_projection`. The exact request is recorded on #1350. No S1/S2/S3-owned
file was edited to work around these dependencies.

After the common base arrives: normal merge; compile/fix the owned modules;
implement same-transaction bootstrap/publication; convert the owned callers;
run all required local gates and canonical Mongo crash/retry/compatibility tests;
then open the one planned S4 PR. No `PR #N ready` claim is appropriate yet.
