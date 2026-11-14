# Named-query index execution

Issue #2005 / PR #2008. Deployment-engineer-02 executes under fresh lead approval.
The lead's 14 November ruling controls this route; it replaces the earlier
separate-DBA-credential and capacity-budget packet requirements. This source
change does not itself execute or approve production DDL.

## Credential and first-step preflight

Render each Job with the qualified API release image. Copy **exactly** the live
`cma-agent-manager` Deployment's `api` container `envFrom` and `env` in memory,
preserving `secretRef` and `secretKeyRef` names/key layout. Append only the Job's
non-secret operation controls. Do not read Secret objects, print values, copy
credentials into files, substitute a separate DBA credential, or change authority
mode. Checked-in env references and image digests are rendering placeholders.
Bind the same actual image in the container and `INDEX_RELEASE_IMAGE`.

Every Job starts with this read-only command before its selected operation:

```text
cma --index-preflight activity-first-page
```

A comma-separated selection is supported. `connectionStatus` uses a five-second
server bound and proves **effective** `createIndex` and `dropIndex` grants on each
selected collection, plus the existing metadata/ledger operation requirements.
A successful `listIndexes` alone never qualifies either DDL action. Output contains
only named queries/collections and boolean checks; no raw privileges or identities.
Missing grants return `preflight_passed:false`, `blocked_collections`, and exit 1.
STOP and report `BLOCKED: URGENT` with those collection names; the lead escalates
to the owner. An unavailable authority response also stops the Job. No mutation
or grant/role change is made by preflight. Build and abort recheck the same
credential before writes. Ordinary runtime/startup authority is unchanged.
Shared-schema permits the app user's other same-database grants; cross-database
and global grants remain refused. Isolated mode retains its explicit role checks.

## Named commands and order

```text
cma --index-catalog
cma --index-size activity-first-page
cma --index-build activity-first-page CHECKSUM IMMUTABLE_IMAGE [SECONDS]
cma --index-status activity-first-page
cma --index-verify activity-first-page
cma --index-abort activity-first-page CHECKSUM IMMUTABLE_IMAGE PREVIOUS_FENCE
```

Use `infra/api/index-toolkit-job.yaml` with a fresh immutable name per operation
and attempt. `INDEX_OPERATION` selects preflight/size/build/status/verify/abort;
`INDEX_QUERIES` selects compiled names. No arbitrary commands, collections,
filters, index options, wildcard drops or automatic retries are accepted. Job
logs contain the sanitized preflight JSON followed by the operation result.
Legacy Activity Jobs and CLI aliases retain the same Activity manifest.

After Batch 9 N1/OFF and its 30-minute stable soak, run Activity first, then the
next build-supported analyst entry, **one at a time during 15 November
03:00–05:00 SGT**. Never write during 02:00–02:59. Size first, retain the unhinted
explain, and obtain the lead's go decision before building. Existing Activity
indexes are verified/reconciled; never build a duplicate.

| Named query | Candidate keys | Build support |
| --- | --- | --- |
| `activity-first-page` | `{recorded_at:-1,_id:-1}` | Existing `am-activity-page-index-v1`, first build |
| `agents-owner-page` | `{owner:1,_id:1}` | New `am-agent-owner-page-index-v1`, next build |
| `usage-event-interval` | `{sandbox_id:1,ended_at:1,started_at:1,revision:1}` | Inspection only; possible blocking sort |
| `usage-reserved-expiry` | `{logical_expires_at:1}`, partial `reserved:true` | Inspection only; complete prune OR predicate |
| `usage-reserved-retire` | `{kind:1,retire_after:1}`, partial `reserved:true` | Inspection only; same OR predicate |
| `sandboxes-pending-claim` | `{work_ready_at:1,_id:1,work_lease_expires_at:1}`, partial `work_pending:true` | Inspection only; lease/retirement predicates retained |
| `workspaces-pending-claim` | Same keys/partial predicate on workspaces | Inspection only; lease predicates retained |
| `native-ready-agent-claim` | `{kind:1,state:1,_id:1}` | Inspection only; agent channel, limit 2 |

The six conditional research candidates have no migration; build/status/abort
refuse before database access and automatic verification cannot pass. Preflight
reports grants separately from `build_supported`. Their read/write tradeoffs
still require evidence before a new reviewed manifest enables DDL. Usage explain
binds one sampled sandbox/UTC day and a 51-event prefix with integer-second time
bounds; it does not establish 7d/30d report latency. Agent owner inspection samples
one private owner value. Only the filter digest is emitted; changed digests mean
different cohorts. Sizing returns count/storage/index metadata and projected plan
structure/numeric counters, never source rows or planner/server error text.

## Online build, thresholds and fenced abort

Build timeout is **60–1800 seconds, default 1800**; outer timeout adds 60 seconds
and the Job deadline is 1900 seconds. MongoDB's modern online build is used without
the legacy `background` option. A 30-second durable server-time lease renews every
five seconds. Progress every 30 seconds means the client is alive, not a server
percentage. Exact name, ordered keys, options and default collation are checked
before DDL. An exact preexisting index can reconcile an incomplete manifest with
zero changes; a completed receipt with drift refuses.

The operator monitors user-route baseline and final 5xx counts during the build.
Invoke the approved exact-target abort when **any** lead threshold is reached:

- API user-route p95 exceeds **2× baseline for two consecutive minutes**.
- Final **5xx exceeds 20 in five minutes**.
- The build is **not complete within 30 minutes**.

Check replication lag only where visible to the app user. The CLI does not infer
route latency/error rates or claim that its timeout cancels server-side DDL.

Stop the build client, prevent concurrent invocations, and reconcile the server
operation and matching durable attempt. Wait for its lease to expire, then run
abort using the **same live Deployment credential references** and the approved
manifest checksum/image/previous fence from status. Abort requires an incomplete
manifest, matching predecessor image/fence and the next global fenced lease.
Live executors, intervening attempts, completed manifests and inventory drift
refuse. It issues only `dropIndexes` for the selected visible index, bounded to
five seconds with majority acknowledgement; no wildcard, document/collection drop,
TTL/index-family removal, killOp or receipt editing is allowed. Missing/in-progress
inventory that cannot be matched exactly requires lead escalation, not an alternate
credential or blind retry. An acknowledged drop records a failed build attempt;
it does not certify server quiescence. Reconcile before resume. A completed
manifest requires reviewed forward recovery, never automatic rollback deletion.

## Postcheck

Create a separately named verification Job using the same release and live env
references. Verification requires `executionStats.executionSuccess` exactly true,
the exact index present, and every complete supported plan branch using it without
COLLSCAN/SORT, missing children or malformed topology. Missing/wrong-type/failed
execution refuses with a fixed safe diagnostic. Failed verification exits nonzero.
Retain sanitized preflight/status/size/verify output and the operator measurements.

Activity API p95 must be **below one second** and satisfy the current owner
regression allowance. Page completion additionally means correct rows or verified
empty state, loading cleared and usable filters/pagination; API p95 alone does not
prove it. The source fixtures demonstrate query work reduction, not production
build duration or production acceptance. Keep the compatible migration binary
when rolling back an application: older registries reject newly recorded manifests.
