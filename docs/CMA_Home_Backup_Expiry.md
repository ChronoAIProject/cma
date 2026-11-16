# Home backup retention and deletion

Status: **未交付 / Production remains HOLD**. This document records the PR-3
retention policy and storage interfaces for #1473. No expiry worker or provider
deletion path is connected by the current implementation. It does not establish
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

The existing snapshot creator labels only its controller identity. It does not
yet write the plan's `cma/backup-of` and `cma/backup-id` labels. The coordinator
must resolve the capture/provider contract and permission evidence before PR-3
can connect the expiry worker and implement orphan reconciliation. No new
permissions, secret, infrastructure file or substitute cloud mechanism has been
introduced here.

Still unimplemented: the supervised expiry loop, its default-false
`CHRONO_HOME_BACKUP_EXPIRY_ENABLED` configuration and Operations/ConfigMap entries,
provider deletion/independent absence, paginated orphan reconciliation and the
PITR rollback orphan regression. D8 tombstone delivery belongs to PR-5.

## Data migration record

| Required field | Record |
| --- | --- |
| Identity, phase and owners | `2026-11-17-am-home-backup-expiry`; #1473 PR-3; additive reader/storage preparation, not activation. Agent Manager owns Mongo records; storage/infra owns independent copies and permissions. Execution operator and release artifacts remain unassigned/unverified. |
| Baseline and dependencies | Parent model/store source `f3e63466760db540053a3874dde296bf391a62b7`, including develop merge `86629438414485a9991bade0f41dcde511c60920`; retention source `2faf0e18c`. PR-1 remains an unverified dependency. Parent CI selection fix must be merged forward before full checks. |
| Affected stores and triggers | Existing `home_backups` and `home_backup_schedules`; explicit deletion-store calls only. No worker, startup backfill, ordinary request or external writer is connected. No Sandbox document write. |
| Durable format and preservation | Existing schema version 1, existing `expiring`/`deleted`, lease/fence and erasure fields only. No collection, nested variant, index, validator or TTL change. Original IDs, cuts, custody and failure evidence remain unchanged. |
| Reviewed implementation checksum | SHA-256 of `layer2-core/src/sandbox/home_backup/deletion.rs`: `0b70b405943ee3c22968bf1ab7cf582607c2f8f4faf075fbbc0895c048933123`; SHA-256 of `cma/src/store/home_backups/expiry.rs`: `fd9423cb3664ee223e452822aafbcbf4f21a75fff6ec2845d2fae62362204f08`. Paths are relative to `api/crates/`. No transformation executable has run. |
| Reader floor and recovery artifacts | Parent strict reader already recognizes these shapes. Its semantic recovery behavior and this candidate have not yet been executed; no API/runtime image digest is certified. Retain a compatible forward-repair artifact before admitting a cohort. |
| Writer gate and cohort | No connected writer/cohort. The planned expiry flag and default-false manifest entry are not yet implemented; activation is blocked. No flag was enabled and no cluster was accessed. |
| Backfill and DDL | None for these interfaces. Retention publication and orphan import remain unimplemented. Existing parent startup DDL is unchanged. |
| Protected backup and dry-run | Not performed. This change issues no provider effect and transforms no deployed data. Before destructive activation, require the reviewed protected backup, matching custody/metadata, disposable-copy dry-run and full restore rehearsal mandated by `CMA_Data_Migrations.md`. |
| Recovery and irreversibility | Provider copy deletion is the irreversible boundary and has not been implemented or run. Disabling admission cannot undo a completed deletion; retain receipts and forward recovery. Do not remove retained fields or weaken readers to roll back. |
| Verification and completion | Static checks and unrun Rust/Mongo suites are recorded below. No production counts, execution receipt, restore rehearsal or operator sign-off exists. This record remains incomplete. |

Policy depends on D5's purge-linked/no-owner baseline. D2 permissions, the Retain
class and D9 object-count limits remain externally unconfirmed. Their absence is
not approval to expand permissions or retention.

## Local validation and isolated Mongo plan

The first logical commit contains the retention pure function and its tests.
Deletion success/failure tests were also written before implementation.
`cargo fmt --all --check`, configuration key/doc checks and source-size checks
passed. Rust execution is waiting for the coordinator's build
slot. Clippy, nextest, doc tests, cargo-deny, locked build and real-Mongo ignored
tests are **未运行**. An ignored test is not evidence of a passing contract.

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

The new ignored tests are under `store::home_backups::tests::expiry::`. The
coordinator has assigned the missing `store::home_backups::tests::` Mongo CI
selection prefix to PR-1; PR-3 must merge that correction and verify selection
before claiming CI coverage.
