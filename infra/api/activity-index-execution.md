# Activity index production execution packet

Owner: deployment-engineer-01. Issue: #2005. The source change is the
merged Activity page index from #1974; this packet does not authorize a
production write by itself.

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

The Job at `infra/api/activity-index-job.yaml` defaults to sizing. It references
the serving API's `cma-agent-manager` ConfigMap and Secret through `envFrom`,
uses `chrono-bot-ksa`, and contains no credential values. The release owner must
replace both image occurrences with the same immutable API image that contains
the CLI modes, then bind that digest to the release record. `size` is the only
initial mode; changing `ACTIVITY_INDEX_JOB_MODE` to `build` is a separate
approved step.

## 1. Size before any build

1. Confirm the release image digest, source SHA, database name, MongoDB version,
   replica-set or mongos topology, and the operator approval ID. Confirm that
   the deployment's database credential has the Activity `collStats` read and
   ordinary `find`/`listIndexes` authority. Do not copy or print the URI or any
   secret.
2. Apply the Job unchanged and wait for the default `size` mode:

   ```sh
   kubectl -n chrono-bot apply -f infra/api/activity-index-job.yaml
   kubectl -n chrono-bot logs job/cma-activity-index-operation
   ```

   The single JSON result contains only the Activity document count, logical,
   storage and index bytes, sanitized index definitions, and the unhinted
   first-page explain. Every server command has `maxTimeMS=5000`. A sizing error
   or timeout is a no-go; do not switch the same Job to `build`.
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

After a fresh go decision, update only the Job's non-secret operation values:

```yaml
ACTIVITY_INDEX_JOB_MODE: "build"
ACTIVITY_INDEX_MANIFEST_CHECKSUM: "64811936c3025cefcb9ff7210c1994986fcfe491152e05f0eee2cef0ecb91c8b"
ACTIVITY_INDEX_RELEASE_IMAGE: "<the exact immutable API image digest>"
ACTIVITY_INDEX_BUILD_TIMEOUT_SECONDS: "2100"
```

The image field and `ACTIVITY_INDEX_RELEASE_IMAGE` must identify the same API
release. Apply the reviewed Job once. `backoffLimit: 0` prevents an automatic
second DDL attempt. The command acquires the existing migration lease, verifies
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
   inspect the sanitized completed receipt. Re-run the sizing mode and retain
   the post-build index inventory and explain.
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
