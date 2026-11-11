# Agent Manager migration runner

This is the first executable slice of [#1328](https://github.com/ChronoAIProject/cma/issues/1328).
It follows the [migration standard](CMA_Data_Migrations.md). The command runs only
when explicitly selected; ordinary API startup does not invoke it. Deployment
does not authorize production execution. The release operator owns execution,
artifact verification and the production evidence record.

## First step and limits

The only supported manifest is `am-activity-query-indexes-v1`. It has no
prerequisites and adds the existing nonunique activity query indexes:

- `record.actor: 1, recorded_at: -1, _id: -1`
- `record.action: 1, recorded_at: -1, _id: -1`

The runner requires the exact generated names and key order, and refuses unique,
partial, sparse, hidden, TTL, collation or other unexpected options on a matching
name/key. It never drops/replaces an index, changes a validator, changes retention,
rewrites an activity record or accesses another service's collections. It is safe
to rerun if an equivalent index was concurrently created by existing startup.

The checksum hashes the exact embedded source bytes of `activity_indexes.rs`.
Once delivered, changing that step requires a new reviewed ID/checksum. The
ledger refuses a different checksum, unknown schema/history, unsupported manifest
or a missing completion receipt. This is a single-step implementation, not a
generic manifest interpreter: future ordering and new manifests require code,
review and tests. Gateway and Image Manager runners are separate later slices.

## Commands and authority

Use the existing `CHRONO_MONGODB_URI`, `CHRONO_MONGODB_DATABASE` and
`CHRONO_MONGODB_AUTHORITY_MODE` configuration. The database name is mandatory for
these commands. No encryption/master key, web login, application bootstrap or
provider credential is needed. Commands do not create users or grant roles.

```sh
cma --print-migration-role
cma --migration-plan
cma --migration-status
cma --migration-apply "$REVIEWED_STEP_CHECKSUM" "$VERIFIED_API_IMAGE_REFERENCE"
```

`plan` and `status` are read-only, including on a fresh database. `plan` reports
the immutable step/checksum, prerequisites, phase and missing index count. `apply`
requires that exact checksum, a release binary compiled with a full
`CHRONO_BUILD_SHA`, and the verified immutable
`ghcr.io/chronoaiproject/cma-api@sha256:...` reference. A development build or tag
is refused before connecting. The supplied image identity must be verified by
the release artifact process; a process cannot infer its registry digest from
the source SHA. No reference supplied to this command pulls or deploys an image.

The printed migration role is confined to `activity` (`listIndexes`,
`createIndex`, `createCollection`) and `agent_manager_migrations` (`find`,
`insert`, `update`, `createCollection`). Read-only commands require only the
inspection subset. Isolated mode refuses broader collection/actions; explicit
shared-schema mode reuses the existing database-authority verifier. Runtime
grants are unchanged, and the new ledger is not opened by normal API startup.
Any production grant change needs separate authorization.

## Durable execution and interruption

`agent_manager_migrations` is Agent Manager owned. A manifest document records
its checksum and completion pointer. A service lease uses Mongo server time,
a monotonically increasing fence and a 30-second expiry. Claiming the lease and
inserting an attempt happen in one snapshot transaction with majority commit.
Each attempt retains source/image, checksum, fence, start/end, outcome, observed
index-create count and a sanitized failure reason. Terminal attempts are never
rewritten into a later success; reruns receive separate attempts.

The runner renews before each effect and every five seconds while awaiting DDL.
Completion atomically fences the current lease, settles the original attempt
and publishes the manifest completion pointer. An unknown commit resolves that
same attempt through a snapshot at the commit response operation time; it does not mint a new operation identity.
Lease loss prevents stale completion. A process crash leaves the original
attempt unfinished; after expiry, another invocation may acquire a higher fence.
The existing index state is the durable per-step checkpoint, so a partial run
creates only the missing indexes before publishing its final receipt.

DDL already submitted to Mongo can finish after the caller times out or loses
its lease. The first step is deliberately limited to identical additive indexes;
the lease does not provide server-side fencing for destructive DDL. TTL changes,
index contraction and validator replacement must not be added to this path
without a separately proven execution boundary and recovery plan.

The complete command is limited to 45 seconds; server selection/connect are
limited to five seconds, metadata reads to five seconds, and each create-index
command to ten seconds with majority acknowledgement. A timeout is an uncertain
attempt, never successful completion. Use status and plan before retrying.
Status returns at most twenty recent attempts; `running` means unfinished and
can describe a crashed executor. It is not a liveness claim. All history remains
in the ledger, and bounded status failure must not be replaced with an empty
success. No ledger TTL or history cleanup is installed.

## Review and release acceptance

Targeted tests cover exact/missing/drifted indexes, isolated/shared authority,
read-only dry-run, rerun and retained records, concurrent claims, expired
executors, interrupted partial DDL, checksum/future-history refusal, missing
completion receipts, and unknown claim/completion commits. Mongo tests require
an explicitly disposable replica set and are included in the existing Mongo
contract selector; never point those tests at production.

The release record must bind source SHA and image digest, reviewed plan/checksum,
environment/time window/configuration, actual attempts, postchecks and retained
record checks. Deploying this CLI alone is not an executed migration. A release
operator may first use read-only plan/status against the authorized target;
execution remains an explicit production operation. Existing startup DDL remains
until the separately reviewed #1329 cutover. Cross-service execution, arbitrary
prerequisites and other backfill receipts remain open under #1328/#1331.
