# Home backup retention and deletion

Status: **未交付 / Production remains HOLD**. This document records the PR-3
retention policy, storage interfaces and default-disabled supervised loop for
#1473. The real provider deletion path is not connected; enabling expiry is
explicitly refused at composition until that boundary is supplied. It does not establish
independent backup or recovery capability.

The approved plan is [IMPLEMENTATION.md](plans/home-backup/IMPLEMENTATION.md)
§3, §5.3, §8 and §9.1 E1–E4. Recovery follows
[R1-DESIGN.md](plans/home-backup/R1-DESIGN.md) and
[R2-DESIGN.md](plans/home-backup/R2-DESIGN.md).

## Fixed retention policy

The pure function consumes one home's ready history, including the ready times
of deleted records. The first ready point in each Singapore calendar day retains
30 days; the first in every other hour retains seven days. All other ready points
retain two hours after the first strictly newer ready time. While there is no
newer point, the deadline is the immutable maximum of 30 days after ready. Every
deadline is at most `ready_at + 30d`; there is no configuration override.

The function sorts by ready time and backup identity. Identity breaks ties when
several points have the same ready second. Equal timestamps do not supersede one
another. Invalid timestamps, empty identities and duplicate identities fail
without returning a partial policy.

Ready publication applies that function inside the existing capture transaction,
after its fenced schedule write. Its stable home key is `sandbox_id` plus
`source.home_identity` and `source.home_source_id` (stored as `source.pvc_uid`).
A replacement home/source starts its own history; runtime generation and boot
changes on the same home do not. It classifies ready history from the preceding
30 days including the whole first Singapore day and deleted receipts. A 4,096-row
bound refuses an exceptional history before publishing a partial policy. Ready
records change through revision/state CAS in that same transaction; an expiry
or restore race aborts publication. The incoming ready record, preceding short
deadlines and schedule pointer commit together. No additional lease is created.

## Deletion storage boundary

`HomeBackupDeletionStore` is separate from capture and changes no stored schema.

- `claim_expiry` selects an expired, unpinned ready/failed record or an abandoned
  `expiring` record. A document CAS persists `expiring`, increments revision and
  the fence high water, and issues a new token before any external effect.
- `claim_expiring` is the finite recovery path when new expiry admission closes.
  It selects only already-admitted `expiring` rows and never admits a ready or
  failed record. A live prior lease still prevents takeover.
- `claim_erasure` admits a named backup only when its parent schedule is
  `erasing`, has no capture lease or in-flight capture, and the backup has no
  restore pin or live lease. It writes the parent schedule and backup in one
  snapshot/majority transaction. This is the storage primitive for PR-5; PR-5
  supplies the schedule fencing/cancellation and account-deletion service.
- `save_deletion` requires an observation of absence for the exact recorded
  snapshot name and underlying copy reference. A present target, future
  observation, empty receipt or expired lease is refused. Token, monotonic fence,
  revision and server time are checked again at publication. The existing
  `erasure{erased_at,receipt}` is retained with `deleted`; missing capture evidence
  on a failed record is preserved rather than fabricated.

Mongo eligibility uses both caller time and server `$$NOW`. The lease holder is
`{deployment_id}:{uuid}`. On an uncertain result, read the record back using its
original identity; do not perform an external effect until durable `expiring`
authority is proved. A takeover waits for the prior lease to expire and uses a
new token and higher fence. Already-admitted deletion settlement must remain
available when future writer admission closes.

## Open implementation blockers

The parent baseline `f3e63466760db540053a3874dde296bf391a62b7` contains model and
capture storage interfaces but not the completed capture worker/provider path.
Its `HomeSnapshotObservation` carries no underlying copy handle. The existing
`provider/home_snapshots.rs` deletion method observes only the named snapshot
object disappearing. With a Retain class, that is insufficient evidence that
the independent copy has been deleted. Plan §5.3 requires deletion of both and
independent provider confirmation before recording `deleted`.

The legacy snapshot creator labels only its controller identity. The independent
native adapter and its `cma/backup-of` and `cma/backup-id` labels are being
provided by PR-1; PR-3 does not extend or consume an uncommitted legacy API. The
coordinator must hand off that native contract and permission evidence before
PR-3 can connect real deletion and inventory. No new
permissions, secret, infrastructure file or substitute cloud mechanism has been
introduced here.

`CHRONO_HOME_BACKUP_EXPIRY_ENABLED` is now strict and default-false in code and
ConfigMap, with the Operations table synchronized. Its optional supervised loop
uses one child task, a five-second tick and at most 20 items per tick, separate
from the shared background budget. Claims receive 60-second document leases;
each provider call is limited to 20 seconds and must leave more than 25 seconds
of authority before it starts. Store calls have five-second limits; before a
claim, the requested lease must have more than five seconds left, and after
observation the returned claim must still have more than five seconds left
before receipt publication starts. Budget checks retain subsecond wall-clock
precision and reject equality. Failures
retain `expiring`; only independent absence can reach receipt publication.
When a real provider is supplied, closing the flag permits only recovery claims
for already-admitted `expiring` work. Without a provider, default-disabled
composition returns before claiming even existing `expiring` work, and performs
no discovery or external effects. Thus finite recovery after admission closure
is a worker contract exercised with a supplied provider, not a delivered
production path. The flag is fixed at worker construction, not hot-reloaded.

Still unimplemented: real provider deletion/independent absence, native inventory
adapter, transactional Mongo orphan admission and real PITR orphan validation.
The pure decisions, bounded scanner and in-memory PITR fixtures described below
passed the selected local Rust tests; this does not establish production
admission or a real PITR result. D8 tombstone delivery belongs to PR-5.

Namespaced snapshot list/get/delete are already declared in
`infra/api/serviceaccount.yaml:68` and suffice for labelled pagination and object
cleanup, subject to unconfirmed remote D2 grants. They cannot delete a Retain
copy. Plan §7.1 (`IMPLEMENTATION.md:423`) specifies cluster
`volumesnapshotcontents get/patch/delete`, alternatively an infra-selected
provider deletion IAM grant. The current Role is namespaced and declares no
content permission. Independent copy absence needs that chosen path's real
observation interface; named-object 404 is insufficient. No RBAC was changed.

## Orphan decisions, pagination and native handoff

The new Layer 2 `HomeBackupOrphanObservation` is an in-memory observation, not a
stored schema or a replacement ready record. Its fields are `backup_id`,
`sandbox_id`, `home_source_id`, `home_identity`, `snapshot_name`, `snapshot_class`,
`snapshot_generation`, optional `source_reference` and `creation_time`, and
`ready`. `decide_orphan` checks known source/name/copy/capture identities before
returning catalogued, await evidence, import by creation time, or admit deletion.
A tombstone or existing deletion record never becomes a new ready backup.

The CMA scanner consumes two separate internal interfaces:

```rust
async fn list_page(cursor: Option<&str>, limit: u32)
    -> Result<OrphanPage, HomeBackupError>;
// OrphanPage { entries: Vec<HomeBackupOrphanObservation>, next: Option<String> }
async fn reconcile_observed(observed: &HomeBackupOrphanObservation, now: &str)
    -> Result<(), HomeBackupError>;
```

Each enabled worker tick attempts at most one page of 20 orphan observations in
addition to at most 20 deletion attempts. A listing call has a 20-second timeout;
each catalog call has a five-second timeout. Only a successful whole page
advances the in-memory token. Partial admission retries the same page and must
be idempotent. Empty pages can advance a token. Duplicate backup identities,
cycling/empty/oversized tokens, oversized pages and invalid evidence fail before
that page's admissions. A server-expired listing snapshot restarts at the first
page without undoing durable work. An unfinished scan stops with an error after
4,096 continuation pages (at most 81,920 remembered identities); it never reports
completion at that cap. D9 must confirm the object count. A failed inventory
attempt does not suppress the existing deletion loop. Disabled admission never
lists inventory, while admitted expiring deletion recovery remains available.
Production composition supplies neither inventory nor orphan admission yet.

The future native Layer 3 interface can use its own types; it must not depend
on Layer 2. The exact payload and behavior PR-3 needs are:

- A single-page API accepting an opaque continuation token and requested limit,
  returning verified entries plus the next token. Every request retains the
  native adapter's managed scope and selectors for both `cma/backup-of` and
  `cma/backup-id`; no unfiltered inventory fallback. A listing-snapshot expiry
  must be distinguishable from transient unavailability.
- Per entry: the backup ID and original Sandbox ID with a checked Sandbox-hash
  label binding; immutable home identity/source UID; object name and UID;
  snapshot class; provider creation time; readiness; bound content identity;
  underlying immutable copy handle and restore size when actually observed.
  An absent handle remains absent, never an invented object-name handle.
- The verified native capture request/receipt must remain recoverable without
  a live source home. For a ready import, CMA additionally needs original
  requested/ready times, runtime generation/version/boot, and custody key IDs.
  Two labels and creation time alone cannot reconstruct those facts. These
  metadata may come from an approved surviving capture record; the scanner
  does not invent annotations or alter the legacy Observation reader.

`reconcile_observed` is an admission boundary, not a deletion implementation. Its
production adapter must read authoritative tombstone status, return an error on
unavailable authority, repeat identity/state checks in its write transaction,
serialize against capture/erasure, and use document leases/fences. It must apply
the same fixed retention function with same-home deleted history, persist
`expiring` before destructive effects, and leave receipt settlement to the
existing independent observer. No production implementation is supplied yet.

The in-memory fixture explicitly retains full original metadata outside the
rolled-back catalog. It exercises multi-page reimport across SGT midnight,
short-point successor deadlines, an already-expired original thirty-day cap,
tombstone admission before simulated effects, refusal of present-copy evidence,
receipt settlement, authority/metadata failures, timeout/retry and expired tokens.
This fixture is not a Mongo transaction, real provider deletion or actual PITR
closed-loop result. Its Rust fixtures passed in the selected local nextest run
recorded below; real Mongo/provider verification remains **未运行**.

Two admission design gaps require coordinator review before production wiring:

1. If a `deleted` catalog record's copy reappears, existing
   `deletion.rs:36` rejects a new deletion claim, and `model.rs:314` requires an
   erasure receipt exactly when state is deleted. Reopening expiring while
   retaining the original receipt therefore changes the strict shape contract;
   clearing it would discard evidence. Options: an approved separate fenced
   replay intent preserving the terminal backup/receipt, or an approved schema
   extension carrying replay authority and prior receipts on that backup.
   Recommend the separate intent, subject to a plan addendum and reader-first
   migration review. Neither is implemented. The fixture returns Conflict and
   preserves the terminal record, cut and retention deadline.
2. Plan §5.3 says import by creation time while the model requires complete ready
   evidence. If original ready/custody/source evidence does not survive, choose
   either an approved immutable metadata recovery source, or a separately
   reviewed non-restorable cleanup admission that never advertises Ready.
   Recommend recovering original metadata for import and keeping incomplete
   copies unavailable until cleanup admission is specified. Never substitute
   the scan time for the original cut or refresh a thirty-day deadline.

## Data migration record

| Required field | Record |
| --- | --- |
| Identity, phase and owners | `2026-11-17-am-home-backup-expiry`; #1473 PR-3; additive reader/storage preparation, not activation. Agent Manager owns Mongo records; storage/infra owns independent copies and permissions. Execution operator and release artifacts remain unassigned/unverified. |
| Baseline and dependencies | Parent model/store source `f3e63466760db540053a3874dde296bf391a62b7`, including develop merge `86629438414485a9991bade0f41dcde511c60920`; retention source `2faf0e18c`. Parent CI selection fix `9024760af406f438d242658f199737292f42b776` was merged forward as `98e32441ff06c987bf6b3ac13064b71be156fd08`. No history was rewritten. PR-1 remains an unverified runtime dependency. |
| Affected stores and triggers | Existing `home_backups` and `home_backup_schedules`; retention runs at fenced ready publication. The optional expiry loop is composed default-disabled without a provider and performs no discovery/effect. No startup backfill, ordinary deletion request or external deletion writer is connected. No Sandbox document write. |
| Durable format and preservation | Existing schema version 1, existing `expiring`/`deleted`, lease/fence and erasure fields only. No collection, nested variant, index, validator or TTL change. Original IDs, cuts, custody and failure evidence remain unchanged. |
| Reviewed implementation checksum | SHA-256 of `layer2-core/src/sandbox/home_backup/deletion.rs`: `0b70b405943ee3c22968bf1ab7cf582607c2f8f4faf075fbbc0895c048933123`; SHA-256 of `cma/src/store/home_backups/expiry.rs`: `c423a4673b1f8e28aa050b3c88297e55ad05149439b74684a66252455e40eb37`. Paths are relative to `api/crates/`. No transformation executable has run. |
| Reader floor and recovery artifacts | Parent strict reader recognizes these shapes and its BSON round-trip/rejection test passed locally. Real Mongo semantic recovery and provider execution remain unverified; no API/runtime image digest is certified. Retain a compatible forward-repair artifact before admitting a cohort. |
| Writer gate and cohort | `CHRONO_HOME_BACKUP_EXPIRY_ENABLED=false` in code and ConfigMap; no admitted deletion cohort. Composition refuses true without a real deletion provider. Capture publication inherits the parent's capture writer gate. Admission closure and finite recovery settlement must be verified with the real provider before activation. No flag was enabled and no cluster was accessed. |
| Backfill and DDL | No startup backfill or new DDL. Retention is applied transactionally at ready publication; orphan import remains unimplemented. Existing parent startup DDL is unchanged. |
| Protected backup and dry-run | Not performed. This change issues no provider effect and transforms no deployed data. Before destructive activation, require the reviewed protected backup, matching custody/metadata, disposable-copy dry-run and full restore rehearsal mandated by `CMA_Data_Migrations.md`. |
| Recovery and irreversibility | Provider copy deletion is the irreversible boundary and has not been implemented or run. Disabling admission cannot undo a completed deletion; retain receipts and forward recovery. Do not remove retained fields or weaken readers to roll back. |
| Verification and completion | Static and selected Rust checks, plus unrun Mongo suites, are recorded below. No production counts, execution receipt, restore rehearsal or operator sign-off exists. This record remains incomplete. |

Policy depends on D5's purge-linked/no-owner baseline. D2 permissions, the Retain
class and D9 object-count limits remain externally unconfirmed. Their absence is
not approval to expand permissions or retention.

## Local validation and isolated Mongo plan

The first logical commit contains the retention pure function and its tests.
Deletion success/failure tests were also written before implementation.
`cargo fmt --all --check`, configuration key/doc checks and source-size checks
passed. During the coordinated Rust review, `cargo check --locked --offline -p
layer2-core -p cma --all-targets` passed after correcting two unsupported BSON
serializer names in this PR's expiry store. The initial failure log is retained.
Selected nextest runs passed 44 domain/worker/scanner/config tests and one BSON
strict-reader test. The selection filters excluded unrelated tests; these are
not full-crate or workspace test results. The selected tests include exact lease
budget boundaries, an observe-to-save wall-clock advance and disabled-admission
recovery. `cargo test --locked --offline --doc -p layer2-core -p cma` completed
with zero doc tests.

The preceding results were obtained at `719a65cdd`. At that revision,
`cargo clippy --locked --offline -p layer2-core -p cma --all-targets
--all-features -- -D warnings` failed on the parent model's `manual_flatten` at
`api/crates/layer2-core/src/sandbox/home_backup/model.rs:251`. The coordinator
identified the parent fix as `419ac014f`. No allow, test relaxation or
parent-code modification was made. Workspace-wide Clippy/nextest/doc gates,
cargo-deny, locked build and real-Mongo ignored tests were **未运行** in that
focused review batch. An ignored
test is not evidence of a passing contract. Logs for this batch are in
`/tmp/1473-retention-rust-review-l3j_hcb0`.

The next review merged parent `d3e98bd96d3443ea3731a9317a4c7c81e060d2db`
forward without conflicts as `46bd36b27e35ca64fcf3012a969b1bd75e0e1a10`.
This includes the model lint fix and independent native snapshot adapter;
neither legacy provider APIs nor production deletion wiring were changed.
Post-merge fmt, config keys/docs and file-size checks passed (7604 files).
Affected-crate Clippy, layer2 nextest, affected-crate doc tests and locked build
were attempted but failed compilation at the parent's
`layer3-adapter/src/agent_sandbox/pool/native_home_snapshot_validation.rs:21`:
`super::api_error` is not in that module (E0425). No tests executed in those
attempts; the previous 45 passing tests do not establish this merged revision.
The namespace correction was reported to the coordinator for upstream ownership.

`cargo deny check` initially could not acquire its advisory lock in the
read-only sandbox; the authorized retry actually ran and failed
`RUSTSEC-2026-0548` for existing `zip 9.0.3` (requires at least 9.0.5).
Bans, licenses and sources passed. No advisory exception or dependency change
was added. The fixed Mongo 8.0.13 digest pull failed with registry EOF. A
separate supplemental fixture using cached Mongo 8.0.34 was verified writable
primary, but its ignored-suite command hit the same E0425 before test execution.
It used the verified local Docker Desktop `desktop-linux` Unix socket,
random owned container/volume and a random port bound only to 127.0.0.1;
ownership-checked cleanup completed. Neither version has a passing real-Mongo
contract result. Logs for this merge batch are in
`/tmp/1473-retention-postmerge-h40pg5AF`.

After the slot is released, use only the official read-only tool paths
`/tmp/1473-runtime-rust-1.96.1/prefix/bin` and `/tmp/1473-tools/bin`, with
`CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, `RUST_MIN_STACK=33554432`. Stop the
task's compilation if free space falls below 8 GiB, without touching other
processes or caches.

The real-Mongo fixture follows `.github/scripts/mongo-contract-tests.sh`: the
fixed CI image is
`mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c`.
Use a random PR-3-specific container/volume/label, bind a random port only on
`127.0.0.1`, initialize its own replica set, and pass only that local URI to the
tests. Each contract creates a UUID database. Cleanup must verify ownership
labels and remove only that fixture's container and volume. Do not reuse an
existing DB, container or cluster. The fixed image was absent from the local
tag inventory; a cached Mongo 8.0.34 run would be supplementary evidence only.

The parent correction is now merged: the ordinary runner and prefix inventory
contain `store::home_backups::tests::`, covering expiry and publication modules.
Compiled nextest metadata now confirms all fifteen home-backup ignored tests,
including five expiry and five publication tests, match the unchanged ordinary
CI filter. The unchanged all-mode filter passes completeness: 1719 ignored CMA
tests, 1684 selected, 31 separate-fixture tests, three child helpers, one tracked
deferral and zero unclassified omissions. The first local proof incorrectly
applied all-mode completeness to ordinary-only metadata; its expected five
separate-partition omissions are retained in the log, and the corrected proof
uses all-mode metadata. This verifies selection only; real-Mongo execution
remains unverified.

After the merge, fmt, configuration key/doc checks and source-size checks passed
again. Direct Node completeness execution in this worktree failed because its
pre-existing use of URL `.pathname` passes the literal `Application%20Support`
path to Python. The exact same script bytes copied to the isolated no-space
directory `/tmp/1473-retention-node-co3yoq14` passed all nine completeness tests,
with zero skipped. No test source, assertion or exclusion was changed to obtain
that result. The original-path failure remains part of the local evidence.

The complete original-path Node run finished with 64 tests: 47 passed, 17 failed,
zero skipped/cancelled. Completeness and selection failures pass encoded paths
to Python (`mongo-contract-completeness.py` and `backend-selection-proof.py`).
The unchanged exported selection registrar was also executed from the exact-byte
no-space copy with the original runner's constants: all three selection tests
passed, zero skipped. This isolates the path issue; it does not turn the original
failed run into a pass or verify the not-yet-compiled Rust test inventory.

The full same three Node test files subsequently ran through an exclusive
no-space symlink to this worktree, with explicit symlink entry paths and
`node --preserve-symlinks --preserve-symlinks-main --test`: **64 passed, zero
failed/skipped/cancelled**, exit 0, Node v22.23.1. All three file hashes were
unchanged before/after. Evidence is under
`/tmp/1473-retention-node-symlink-p1l34z77`; the successful run took 280.23 seconds.
Using relative entries from that same symlink working directory still resolved
to the real path and reproduced 47 passed/17 failed (276.67 seconds, exit 1).
Both logs remain recorded. These Node results do not execute Rust or Mongo.
