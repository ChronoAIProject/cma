# Ledger maintenance and recovery

Refs #1461 / #1443. The frozen ledger contracts revision 1 governs integration.
This binding slice adds private types and necessary writer decisions. It does
not enable maintenance, release, restore, collection or workload recovery. No
production route, worker, configuration or lifecycle transition is changed.

## Shared binding

`layer3_adapter::runtime_ledger_maintenance::LedgerBindingV1` implements serde,
Clone, equality and hash-only Debug. All frozen fields are public. The source
enum is `LedgerSourceFormatV1::{V1,V2}`, serialized exactly as `"v1"` / `"v2"`.
Direct deserialization rejects unknown fields, missing required fields, unknown
formats, zero revisions/epochs, invalid identities and malformed hashes.

Use `validate() -> Result<(), LedgerBindingRefusal>` before using an in-process
value. `validate_for(&accepted)` additionally compares every field. No consumer
may infer authority from successful validation or a root hash.

`LedgerBindingArtifactV1::parse(&[u8])` bounds input to 8 KiB and retains its exact
original bytes and SHA-256. Persist `original_bytes()` and verify readback before
source use; this in-memory wrapper does not claim persistence. Do not regenerate
JSON on takeover. `binding_sha256()` instead hashes structured identity: domain
`chrono.runtime.ledger.binding.v1\0`, then each component in declaration order,
with an eight-byte big-endian byte length before each component. Integers are
eight-byte big-endian values. The final component is `length-prefixed-be-v1`.
That structured hash and the artifact byte hash serve different purposes.

The archive ID is a canonical non-nil UUID. Sandbox and operation IDs follow the
existing bounded string identity convention. Home and boot validation reuse the
runtime identity contract. Errors contain no supplied identifiers.

## Durable records and guard contract

`LedgerMaintenanceRecordV1` records the binding, exact accepted plan hash,
monotonic record revision, phase and authority/backup/release receipt hashes.
It is a persistence schema; no persistence adapter is installed by this slice.

The phase grammar is Requested → Draining → Excluded → BackedUp → Applying →
HeldVerified → Released. RestoreHeld and AbortedHeld remain non-serving. The
grammar does not grant transition authority. A restore cannot silently become
Released. Lease expiry and elapsed time have no serving meaning.

`check_ledger_writer(observation, class, fence)` is a necessary check inside the
same transaction that admits the writer. It is not a separate read-before-write
authorization check. Normal ownership, worker fencing, retention and lifecycle
checks still apply. An unavailable or incomplete read is Unknown, not KnownAbsent.
Malformed records refuse. Every non-serving phase refuses every writer. Released
requires a complete release record and the exact new writer binding; an old epoch,
boot, root or revision refuses. Purge still requires B's independent final-home
retention acknowledgment. Neither group completion nor release deletes that marker.

The exhaustive integration matrix comprises owner input, owner control, creation,
Agent workers, Workspace workers, recorder, checkpoint, output, reconciliation,
recovery, rotation, revival, credentials, retirement, home cleanup, purge,
administration and provider paths. Existing admitted external work must be fenced
or physically excluded too; this function cannot undo an already issued effect.

## Recovery requests and retained originals

`LedgerRecoveryPlanV1` records one immutable logical request: original binding,
source Agent/log/command-lane revisions, failure/attempt evidence, disposition,
backup/salvage/source-evidence hashes, consent receipt and explicit successors.
`LedgerRecoveryArtifactV1` retains up to 16 KiB of exact accepted bytes. Retry
requires identical bytes. A takeover advances receipt authority, never rewrites
the accepted request. Progress is recorded separately in `LedgerRecoveryReceiptV1`.

Structural plan validation does not prove lifecycle eligibility, ownership,
consent authenticity, backup availability or absence of newer effects. Those
checks and durable transactional acceptance are required in C1/C2.
Migration requires both a new Sandbox and a new Agent. PreserveHeld and the
separately qualified SameAgentRetry cannot carry successor identities. There is
no attempted-input replay field or automatic replay path.

Retiring parents require owner-selected workload migration with verified salvage;
they cannot be reset to Active. Failed recovery candidates are not collector
garbage. Originals remain held until durable preservation and the exact accepted
disposition. Starting a repaired image must not resume scheduled destructive work.

## Runtime exclusion boundary

`retirement_activation::ValidatedLedgerExclusion` has private fields, immutable
binding/plan/backup/authority accessors, an owned Send hold and no constructor.
It cannot be cloned or deserialized. This slice intentionally cannot produce
execution authority: E0–E5 validation and real old-provider/mounted-writer exclusion
remain prerequisites. Detached work must own the eventual validated permit.

Restore additionally needs A's exact transition receipts and independent proof
of no newer filesystem, conversation/provider or external effects. A database
revision alone is insufficient. Restore keeps permanent replay refusal and the
serving hold. Release requires separately verified E8 and explicit consent.

## Integration handoff

A owns shared module registration and existing writer paths. The exact additive
registration hooks are:

```rust
// api/crates/layer3-adapter/src/lib.rs
pub mod runtime_ledger_maintenance;
// api/crates/layer2-core/src/sandbox/mod.rs
pub mod ledger_maintenance;
pub mod agent_ledger_recovery;
// api/crates/layer4-runtime/src/main.rs, with the consuming adapter
mod retirement_activation;
```

The adapter export is immediately required by B. Core registration and writer
integration must land together with the reviewed persistence/reader floor.
The runtime type is not an execution hook and must not replace qualified exclusion
with a generic guard. This branch does not independently edit A's shared files.

Integration tests compile the exact owned source leaves before registration.
They cover the frozen JSON schema, every changed binding component, byte retention,
all writer/phase combinations, restart, stale release fences, malformed evidence,
explicit successors and immutable retry. Real-Mongo CAS, lease/two-worker races,
physical exclusion, restore and actual recovery are later gates; the binding
slice introduces no database writes to exercise yet.
