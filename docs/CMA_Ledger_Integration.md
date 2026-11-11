# Runtime ledger integration and recovery census

This page describes the shared groundwork for bounded collection and supported
recovery. It does not enable mutation of a retained home or authorize a Failed
Agent to become usable. Follow [the retirement contract](CMA_Runtime_Ledger_Retirement.md)
and [the migration standard](CMA_Data_Migrations.md). The release owner performs
production observations and actions using reviewed immutable artifacts.

## Exact-cohort read-only census

Run the service executable with `--ledger-census PRIVATE_REQUEST PRIVATE_NEW_FILE`.
The command runs before service initialization. It uses the existing Mongo URI,
database and authority-mode configuration, verifies service collection authority,
and performs indexed `_id` lookups in one snapshot transaction. Success and failure
both abort the transaction. It does not initialize collections/indexes, inspect
Agent log payloads, touch runtime files or perform a lifecycle transition.

Input is a strict versioned JSON object:

```json
{
  "format": "runtime-ledger-census-request-v1",
  "agents": [
    {"agent_id": "fixture-agent", "sandbox_id": "fixture-sandbox"}
  ]
}
```

The release owner supplies the privately retained exact Agent/parent identities
for the intended incident cohort. Hashes from an aggregate fleet report cannot be
reversed into identities and are not accepted as a substitute. Refresh the same
explicit cohort, including missing or changed members, so disappearing rows cannot
silently improve the reported recovery count. The request allows 1–256 distinct
Agents, at most 256 KiB. Source BSON is bounded to 32 MiB, the entire command to
60 seconds and each query to five seconds. No broad fleet scan is implied.

Both artifact paths must be absolute, in private 0700 directories. Input must be
a regular 0600 file with one link; file symlinks are refused. Output is create-only
0600, synced with its directory, never overwrites another artifact and is limited
to 4 MiB. Retain input and output together. They contain private resource identity;
never attach them to GitHub or operational logs. Standard output contains only
classification labels, counts and SHA-256 hashes. Failure messages do not include
input/database details. The summary groups assessed members jointly by failure
cause, Agent state, parent state and recovery route. It also counts preservation
requirements and observed evidence. `requested = assessed + unknown`; missing,
unreadable and changed-lineage members remain in `outcomes` and the unknown count.
Unknown members are counted as `unresolved_member_preserve_all`, not recovered or
eligible for cleanup. State changes remain in the cohort and require reclassification.

Each requested member has an explicit outcome. Valid records are classified by
diagnostic cause, current Agent and parent status, revisions, scheduled cleanup,
lease/debt, conversation/input presence, attempted work and retained-home metadata.
`input_state_present` includes terminal input receipts and commands; it makes no
claim that they are still active or queued. They remain preservation dependencies.
Missing, quarantined, unsupported, malformed or changed lineage is recorded as an
unknown preservation case. A string matching a historical limit error is only a
diagnostic classification, never recovery authority.

Preserve the complete home, Worktree files/Git state, both mutation stores and
archives, history, original operation keys, configuration and artifact references.
Conversation/input and pending-cleanup preservation are called out separately.
Every result states that filesystem contents were not observed, absence of provider
effects was not proved, and recovery is not authorized. No recorded conversation
does not prove that files or partial effects are absent. Fresh home/archive/provider
evidence and the owner's disposition still belong to the maintenance/recovery path.

A Retiring parent requires explicit disposition followed by supported workload
migration if selected. Capacity repair/restart can resume already-scheduled cleanup;
it does not revive Failed Agents. An Active parent still requires qualified recovery
assessment. Other parent states require their own recovery first. The census does
not change Retiring to Active, Failed to Ready or replay attempted input.

## Planned collector lookup indexes

The explicit Agent Manager migration runner now accepts a step ID after its command.
The existing command spelling without an ID still selects the immutable Activity
index step. The new step is `am-ledger-collection-indexes-v1`:

```text
cma --migration-plan am-ledger-collection-indexes-v1
cma --migration-status am-ledger-collection-indexes-v1
cma --print-migration-role am-ledger-collection-indexes-v1
cma --migration-apply am-ledger-collection-indexes-v1 CHECKSUM IMMUTABLE_API_IMAGE
```

Only release-owned explicit apply runs DDL. The plan/status paths and census are
read-only. No new index is built by normal service startup. The collection step
requires the existing service collections; it adds five complete, nonunique indexes:

| Collection | Ordered keys |
| --- | --- |
| agents | `worktree_id, _id` |
| agents | `record.worktree_id, _id` |
| workspaces | `sandbox_id, name, _id` |
| workspaces | `record.sandbox_id, record.name, _id` |
| worktrees | `record.workspace_id, record.name, _id` |

All directions are ascending. The existing projected Worktree workspace/name index
is reused by the collector. Live-only partial indexes do not prove complete retired
or shared membership. The new migration refuses changed key order, partial/unique/
sparse/hidden/TTL/collation drift, aliases and ambiguous duplicates. It does not
drop, rebuild or rewrite an existing index. Missing collection or uncertain outcome
requires inspection and explicit retry.

Each immutable manifest has its own checksum, completion and attempts; steps share
one renewable, server-time runner lease. Unknown manifest/checksum versions refuse
before effects. The previous migrator recognizes only the Activity manifest and
must not run after the collection manifest has been written: retain this migrator
or a newer qualified recovery image. Existing Activity manifest bytes/checksum
and receipt semantics are unchanged. Additive indexes do not alter Agent records
or make old runtime writers safe. No service reader-floor activation is implied.

## Integration boundary

The collector owns bounded authority lookup/progress and purge retention. The
maintenance/recovery track owns durable hold/restore/admission state and external
writer-exclusion qualification. Shared module/command registration, aggregate
reader gates and service-worker wiring are integrated only with their concrete,
reviewed leaf commits; missing implementations are not filled by permissive stubs.
The read-only census and explicit migration are independently runnable groundwork.

An executing collector additionally requires archive-before-source-before-durable-
acknowledgement, exact binding/revision checks, cancellation-safe ownership and a
qualified maintenance permit. Restore requires full-home no-newer-effects proof.
No background collector is scheduled merely by deploying this groundwork.

Validation covers malformed/private-path input, preservation classifications,
snapshot/no-DDL behavior, missing/changed/quarantined records, ordered index drift,
separate manifest receipts, retry, shared-lease exclusion and future-reader refusal.
Disposable Mongo tests are part of Backend Mongo contracts. Production census and
complete cohort recovery are separate release evidence.
