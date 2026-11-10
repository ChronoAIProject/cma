## Change

Describe the concrete problem and resulting behavior. Reference the tracking
issue with `Refs #N` while release verification remains outstanding. Use
`Closes #N` only when the change fully resolves its issue, including required
production evidence; do not close a parent issue for a partial slice.

## Validation

List the relevant local checks and CI evidence, including failures or limits.
Existing repository gates and continuity rules remain required.

## Data changes

Follow [CMA Data Migrations](https://github.com/ChronoAIProject/cma/blob/develop/docs/CMA_Data_Migrations.md). For an unaffected
item, write `Not affected` with a reason; a flag-only or documentation change
still needs an explicit assessment.

- **Affected stores, owners and writers:** Include ordinary requests, workers,
  startup/import, retry/recovery, runtime files, client journals and external effects.
- **New formats and preservation:** Fields/variants, nullability, indexes,
  validators/TTL, format IDs; retained IDs, histories, keys and operation identity.
- **Reader floor:** Exact compatible image/executable/asset identities and old/new
  fixture evidence; identify excluded predecessors and the complete release batch.
  Distinguish actual artifact execution from decoder models and missing cells.
- **Writer gating:** Flag/capability, default, cohort/authority, process drain,
  disabled-writer evidence, and retained read/recovery behavior.
- **Migration/backfill:** ID/checksum, phase, prerequisites/order, command,
  locking/fencing/CAS, idempotency, bounds, progress and completion criteria.
  Include startup DDL/scans, capability readiness, partial effects, schema guard
  parity with writer bytes, and protection from an older migrator.
- **Backup/dry-run:** Recovery unit, protected backup/restore evidence and owner,
  post-backup writes; justify no irreversible step when not applicable.
- **Rollback/forward recovery:** Exact permitted artifact, procedure, irreversible
  boundary, runtime/home/asset/key dependencies and abort criteria.
- **Completion evidence:** Tests, release record, production receipts, unresolved
  exceptions and the owner responsible for production verification/issue closure.
  Bind evidence to source SHA, candidate digest, environment, time window, sample
  count, configuration and raw results; report failed or unexecuted checks.

## Production verification

State the authorized verification plan, expected observations, stop conditions
and retained-data checks. Distinguish executed evidence from planned checks.
Do not imply that merge authorizes production writes or closes the issue.
