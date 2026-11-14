# Named-query index toolkit execution packet

Owner: deployment-engineer-01. Issue #2005 / PR #2008. Activity is the first catalog entry. `agents-owner-page` adds the analyst-ranked
nonunique `{owner:1,_id:1}` access path through `am-agent-owner-page-index-v1`.
Other candidates require a reviewed catalog entry and complete manifest before execution. This packet supplies no operation
approval. Small collections may run soon after fresh approval; large collections
remain in 15 November 03:00–05:00 SGT. Never run production writes 02:00–02:59.

## Reusable command contract

The API dispatches these commands before serving. No request, worker, startup or
readiness path invokes the toolkit. `--index-catalog` prints the compiled names,
collection/filter/sort/projection/limit definitions and exact index/manifest
checksums. Runtime input selects names; it cannot supply arbitrary Mongo commands,
filters, collection names or index options. Add a reviewed catalog entry and its
complete one-index manifest to support another index. Never change an already
recorded manifest's checksum. Activity aliases retain the existing manifest.

```text
cma --index-catalog
cma --index-size activity-first-page,agents-owner-page
cma --index-verify activity-first-page
cma --index-build activity-first-page CHECKSUM IMMUTABLE_IMAGE [SECONDS]
cma --index-status activity-first-page
cma --index-abort activity-first-page CHECKSUM IMMUTABLE_IMAGE PREVIOUS_FENCE
```

The Agent owner query samples one owner string internally from the first matching
Agent in `_id` order, using a bounded projection. It outputs only a filter digest,
never the owner value. Compare that digest before/after; a different owner is a
different cohort. One sampled owner does not establish all-owner performance.
An empty collection has no representative owner explain and cannot pass automatic
verification. Activity uses its exact unfiltered first-page query. The interval,
partial-report, worker and small-pool research candidates are not build targets;
the analyst identifies possible sorts/write costs that still need evidence.

Sizing/verification accept at most eight distinct comma-separated catalog names.
Build, status and abort accept exactly one. Each read command has a five-second
server bound; a query inspection has a 30-second outer allowance. Unknown index
names/keys are redacted or hashed; no document values, planner text, hosts,
shard names, connection strings or credential values are output. Sizing executes
the bounded unhinted query with executionStats and returns no source documents.
The automatic verification exit is nonzero unless the exact index exists and
every supported plan branch reaches it, without COLLSCAN/SORT, missing children,
unknown topology or incomplete markers. Preserve its JSON on failure. This is a
planner gate; it does not establish HTTP or page-usable latency.

## Render and approve each Job

`infra/api/index-toolkit-job.yaml` defaults to read-only sizing and uses the
Deployment's ConfigMap, Secret references and service account. Render a fresh
explicit Job name for each size/build/status/verify/abort attempt; never mutate a
created Job's pod template or delete an active Job to reuse its name. Bind the
same approved immutable image in the container image and `INDEX_RELEASE_IMAGE`,
plus its actual compiled source and registry compatibility. The inherited image
is a placeholder, not release evidence. The image CLI argument is an operator
assertion, not independent image attestation.

Set `INDEX_OPERATION`, `INDEX_QUERIES`, and (for writes) the selected manifest
checksum. `INDEX_BUILD_TIMEOUT_SECONDS` defaults to 2100 and accepts 60–3600.
Build progress records the query/manifest, fence and elapsed time. The 30-second
server-time lease renews every five seconds; a heartbeat every 30 seconds means
the client is alive, not a server build percentage. The outer build allowance is
the chosen timeout plus 60 seconds; the Job deadline is 3900 seconds. There is no
automatic Job retry. Timeouts, lost acknowledgements and lease loss do not prove
that MongoDB stopped its submitted command.

Bind the reviewed controller version/verdict and exact create, stop,
Job-status and sanitized-log readback handles in the release record. The kubectl
shapes below are not a substitute for an approved controller route. No Secret
read, copied URI or pod exec is required. Attest effective grants internally:
ordinary runtime authority is unchanged; sizing adds collStats on selected AM
collections, build/status use the explicit ledger role. Use `--index-status` for the matching
operation credential; the older generic migration-status role is a separate contract. Shared-schema remains a
database-scoped authority model, not isolated least privilege, and refuses
dropIndex on selected collections or database-wide. Isolated operation roles
must match exactly; do not change a live isolated runtime's grants as a workaround.

## Exact-target abort through the toolkit

The abort operation deliberately refuses ordinary build credentials. A separate
approved operator credential must have only listIndexes/dropIndex on the selected
collection and find/insert/update/createCollection on the migration ledger. It
cannot have database-wide drops or unrelated runtime authority. Reference the
approved credential in a separately rendered abort Job; never extract its value.
The release owner must approve the conditional drop and its controller route
before admitting the build.

Stop the build client, prevent other invocations, reconcile actual server DDL,
and wait for the durable lease to expire. Obtain the matching attempt fence from
sanitized progress/status. Abort validates that manifest is incomplete and the
latest attempt matches the approved image/fence, then acquires the next global
fence transactionally. A live executor, concurrent completion or intervening
migration refuses. Key/name/options/default-collation drift refuses before DDL.
The command drops only the selected visible index with maxTimeMS=5000 and
majority acknowledgement; absence before drop is a refusal requiring server
reconciliation. It never drops TTL/unique/partial indexes or uses wildcards.

An acknowledged drop is recorded as a FAILED build attempt and leaves the
manifest incomplete; it never erases receipts or certifies server quiescence.
The prior executor cannot publish completion after the new fence. Reconcile
server build state and exact inventory after any drop/timeout before considering
resume. If completion already won, ordinary abort refuses: use a reviewed
manifest-forward recovery. Keep the exact operator image/registry compatible
with all recorded migrations. Do not substitute killOp or restart MongoDB.

## Activity acceptance and scheduling

Activity's API acceptance is p95 <1 second, as stated in the original lead task;
collect at least 20 natural requests and retain errors and full denominators.
K19 additionally requires page completion: correct first-page rows or verified
empty state, loading cleared, and usable filters/pagination. No numeric page
ceiling is supplied; API p95 does not establish that client boundary. Under A3,
Activity API and page are UX-AFFECTING: the confidence upper bound on p95
regression must fit +10% of the matched N1 baseline, aiming for <=5%, while API
p95 remains <1s. Do not add noise to the percentage. Critical enclosing journeys
retain their own absolute targets and noise-only allowance. K25 automatic
production HPA remains a separate release obligation.

Size first. A collection being <=5M documents and having a <=30-minute approved
estimate does not automatically classify it as small. Deployment must explicitly
classify it using live size, topology and resource/duration evidence. If a large
build stays after03:00, the serial soak/build/repeat schedule still requires an
approved arrangement preserving K21; do not compress or borrow noise windows.
Use the Activity-specific gates and retained command shapes below for its first
catalog entry. All numbers are go/no-go inputs, never inferred production facts.

The candidate index is nonunique, visible and unfiltered:

```javascript
{
  name: "activity_recorded_at_id_desc",
  key: { recorded_at: -1, _id: -1 }
}
```

The manifest is `am-activity-page-index-v1`, checksum
`64811936c3025cefcb9ff7210c1994986fcfe491152e05f0eee2cef0ecb91c8b`.
The API build uses MongoDB's online index-build behavior; it deliberately omits
the removed legacy `background` option. The build command is bounded to 2,100
seconds by the Job and reports sanitized inventory, start, 30-second heartbeat,
finish and verification phases. The outer Job deadline is 3,900 seconds.

The sizing Job at `infra/api/activity-index-job.yaml`, build Job at
`infra/api/activity-index-build-job.yaml`, and postcheck Job at
`infra/api/activity-index-postcheck-job.yaml` are separately named, immutable
one-off operations. Each references the serving API's `cma-agent-manager`
ConfigMap and Secret through `envFrom`, uses `chrono-bot-ksa`, and contains no
credential values. The release owner must replace the image and
`ACTIVITY_INDEX_RELEASE_IMAGE` in the build Job with the same immutable API
image that contains the CLI modes before creating either Job.

## 1. Size before any build

1. Confirm the release image digest, source SHA, database name, MongoDB version,
   replica-set or mongos topology, and the operator approval ID. Confirm that
   the deployment's database credential has the explicit Activity-operation
   grants: Activity `collStats` for sizing, and `find`, `insert`, `update`, and
   `createCollection` on `agent_manager_migrations` for build mode, alongside
   ordinary runtime authority. In isolated mode the effective grants must match
   the checked-in operation role exactly; in shared-schema mode retain an
   effective-grant receipt and reject Activity or database-wide `dropIndex`.
   Do not copy or print the URI or any secret.
2. Apply the sizing Job unchanged:

   ```sh
   kubectl -n chrono-bot apply -f infra/api/activity-index-job.yaml
   kubectl -n chrono-bot logs job/cma-activity-index-size
   ```

   The single JSON result contains only the Activity document count, logical,
   storage and index bytes, sanitized index definitions, and the unhinted
   first-page explain. Every server command has `maxTimeMS=5000`. A sizing error
   or timeout is a no-go; do not create the build Job.
3. Retain the sanitized output with the candidate image digest, source SHA,
   database/cluster identifier, UTC time, and command result. Never retain raw
   documents, connection strings, authenticated usernames or secret values.

## 2. Go/no-go gate

Proceed only when all of these are recorded and approved:

| Check | Go threshold | No-go action |
| --- | --- | --- |
| `metadata.document_count` | `<= 5,000,000` retained Activity documents | Do not build; obtain a larger-window plan |
| Estimated online build duration | `<= 30 minutes`, based on approved same-topology evidence or a reviewed capacity estimate | Do not build; obtain a bounded strategy |
| Disk headroom | Enough for the reported current storage plus the estimated new index bytes, temporary build space and a 30% safety reserve | Do not build |
| Primary CPU and write load | Within the release budget, with at least 30% CPU headroom and no active incident | Defer to a quieter window |
| Replica lag | Below the release budget and stable during the preflight observation | Defer; do not trade durability for speed |
| Planner | Explain is read-only and the current plan is retained for comparison | Refuse if explain is unavailable or malformed |
| Existing index state | No key, name, collation, unique, sparse, partial or hidden drift; no competing DDL | Reconcile the conflict before build |

The 2,100-second command bound is a failure limit, not evidence that a build
fits the 30-minute estimate. The sizing CLI does not invent a duration from
query latency. A production estimate must include the collection scan, index
entry creation, temporary disk, commit-quorum and replication work.

## 3. Start the bounded online build

After a fresh go decision, update only the build Job's non-secret release values:

```yaml
ACTIVITY_INDEX_MANIFEST_CHECKSUM: "64811936c3025cefcb9ff7210c1994986fcfe491152e05f0eee2cef0ecb91c8b"
ACTIVITY_INDEX_RELEASE_IMAGE: "<the exact immutable API image digest>"
ACTIVITY_INDEX_BUILD_TIMEOUT_SECONDS: "2100"
```

The image field and `ACTIVITY_INDEX_RELEASE_IMAGE` must identify the same API
release. Apply the reviewed build Job once:

```sh
kubectl -n chrono-bot apply -f infra/api/activity-index-build-job.yaml
kubectl -n chrono-bot logs job/cma-activity-index-build
```

`backoffLimit: 0` prevents an automatic second DDL attempt. The command acquires
the existing migration lease, verifies
the exact manifest and options, and records a durable attempt. A repeated
successful invocation returns `already_completed`; a completed manifest with a
missing or drifted index refuses. It never changes Activity documents or the TTL
index.

The build requires a supported replica set or mongos topology for the durable
lease transaction. While MongoDB builds the index, the CLI renews that lease
every five seconds so a second operator cannot claim the migration; progress
heartbeats remain at 30-second intervals.

Watch the sanitized log phases and the Job deadline. Do not sum heartbeat
intervals into build duration. Do not blindly retry after a timeout, lease loss,
write-concern error or client disconnect: the server command may still be
running, and the durable attempt may be unfinished or failed.

## 4. Abort and recovery

The migration credential has no `dropIndex` privilege on Activity. Aborting is a
separate database-operator action. First stop this Job and prevent any second
operator from starting it. Reconcile the Job, the migration status and the
actual Activity index/build state. If the index is present and the approved
abort decision requires removal, the operator may use the narrowly targeted
command below with the server-version-specific timeout and write concern:

```javascript
db.runCommand({
  dropIndexes: "activity",
  index: "activity_recorded_at_id_desc",
  maxTimeMS: 5000
})
```

This is a command shape for the separately approved operator route, not a
permission embedded in the Job or a command to run from the API container. Do
not use a wildcard, drop the collection, remove `activity_expiry` or actor/action
indexes, kill the database process, or edit migration receipts. Confirm the
server's current operation and actual index state before and after the drop; a
client timeout does not prove that a server-side build stopped.

If the manifest was already marked completed and the index is later dropped,
ordinary reapply must refuse completed-manifest postcondition drift. Use a
separately reviewed manifest-forward recovery procedure. For an unfinished or
failed attempt, reconcile the durable attempt, lease fence and index state before
resuming the same manifest. Retain the new migration-capable executable after a
manifest is recorded because older migration registries reject the new history.

## 5. Verify the result and release

1. Run the existing migration status command with the exact manifest ID and
   inspect the sanitized completed receipt. Apply the separately named
   postcheck Job, then retain its post-build inventory and explain:

   ```sh
   kubectl -n chrono-bot apply -f infra/api/activity-index-postcheck-job.yaml
   kubectl -n chrono-bot logs job/cma-activity-index-postcheck
   ```

   Never delete or mutate an active Job to reuse its pod template. If a new
   postcheck is needed, render this reviewed manifest with a fresh explicit
   `metadata.name` and retain the rendered artifact before applying it.
2. The unhinted explain for the Activity first page must use
   `activity_recorded_at_id_desc`, examine approximately `limit+1` keys/docs, and
   contain no blocking `COLLSCAN` or `SORT`. Preserve the before/after explain
   artifacts.
3. With exact API and Web release identities bound, run at least 20 natural
   Activity requests covering the unfiltered first page and normal navigation.
   The release gate is Activity API p95 below 1 second, with no elevated error
   rate, write pressure or replica lag. The previous production baseline was
   p50 2.561 s and p95 2.780 s (batch 7, n=5); it is a comparison point, not a
   causal before/after pair for this packet.
4. Confirm full Activity responses still include detail when no summary view is
   requested, retention and existing indexes are unchanged, and the operational
   read-sampling switch remains false. Roll back API and Web together if the API
   version does not accept the summary query parameter used by the new Web.

Retain the sanitized sizing/build/status/verification outputs and the operator
approval IDs in the release record. No production build, drop or sampling
activation is implied by merging the implementation PR.
