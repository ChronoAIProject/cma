# Bounded runtime ledger collection

This describes Track B's candidate, observation and progress contracts for
#1443 and #1461. The first slice does not enable collection or reclaim runtime
capacity. Runtime transitions require the separately reviewed Track A adapter
and Track C maintenance/exclusion authority. The existing whole-hierarchy
retirement export retains its meaning and limits.

## Authority and limits

The runtime supplies candidates from its active working set. The API does not
discover them by scanning historical Deleted Agents. A candidate page pins the
exact binding, active revision, opaque cursor and explicit completion marker.
An empty page alone is not completion. Pages contain at most 64 distinct sorted
group keys and encode to at most 2 MiB. A changed active revision invalidates a
cursor; a consumer cannot continue across a mixed snapshot.

Agent and Worktree keys reuse the existing permanent retirement hashes.
Track A parses Worktree paths into validated workspace/name components before
constructing a candidate. Raw paths and filesystem canonicalization are not
alternative identities.

The API observes a requested dependency closure in one Mongo snapshot and aborts
the transaction even on success. A request has an independent operation ID and
pins its exact page hash. It contains at most 64 groups and encodes to at most
64 KiB. The shared read budget is 256 document reads, 16 MiB of BSON and five
seconds overall; each query has a two-second server limit. Repeated reads count
against that budget. A dependency exceeding it refuses the request instead of
silently truncating eligibility.

Lookups use complete indexes over both the projected relationship and the
authoritative record relationship. A drifted projection cannot hide a shared
live child. Queries explicitly hint the index; a missing index refuses. The
observation path performs no DDL, history reads or durable writes.

| Collection | Required index keys |
| --- | --- |
| All queried collections | `_id` |
| Agents | `(worktree_id, _id)`, `(record.worktree_id, _id)` |
| Workspaces | `(sandbox_id, name, _id)`, `(record.sandbox_id, record.name, _id)` |
| Worktrees | `(workspace_id, name)`, `(record.workspace_id, record.name, _id)` |

Track A owns registration and index installation. Indexes are appended without
reordering the existing mandatory contract. Observations never install them.

## Eligibility and evidence

Only a fully settled Deleted Agent with a Removed Worktree is eligible.
Pending input, active turns, cleanup debt, work leases, recovery, pending skill
installation and shared live Agents preserve the group. Missing, malformed,
foreign or quarantined facts refuse the request. A caller must treat refusal
as unknown eligibility. Neither absence nor failure is an assumed tombstone.

Every returned group carries `SettledDeleted`, `RetainLive` or `RetainUnknown`.
The binding includes the exact Sandbox/home, boot, generation, maintenance
epoch and Sandbox revision. Exact original evidence bytes are retained beside
their SHA-256 reference; they are not reserialized to claim original identity.
Structured request hashes use a domain-separated, length-prefixed encoding.

These are observations, not permits. Track A independently revalidates source
membership and source revision inside its transaction. Track C supplies the
non-serializable validated exclusion permit and compatible-reader activation.
Failed and Indeterminate runtime attempts remain subject to the existing
Completed/Absent restrictions. Settled API deletion does not prove that delayed
effects cannot arrive. Failed Agents selected for workload recovery are retained.

## Durable progress and purge

One bounded `ledger_collection_v1` marker is stored on the Sandbox parent.
Acquisition compares the parent revision; subsequent updates compare both the
parent and progress revisions. Putting retention on that row makes acquisition
conflict with the hierarchy purge transaction's final parent deletion.

Worker claims have monotonic fences and expiry. Takeover preserves the original
operation ID, evidence references and unprocessed page groups. A lease controls
scheduling only. It is never source, maintenance or release authority.

The collector records `Selected -> Prepared -> SourceCommitted -> Acknowledged`
in order, using Track A's validated original receipt at each durable phase.
Only an acknowledged mutation or an explicitly retained no-effect group may be
settled. Cursor advance requires the exact pinned page, unchanged source revision
and all groups settled. Unknown outcomes stay pending across restart and block
rollover. A later worker reconciles the original operation; it cannot invent a
fresh operation to retry an uncertain source transition.

The progress record retains one pending operation, at most one page and one
last acknowledgement reference. Permanent replay refusal belongs to Track A's
cold archive indexes; no per-retired-group hot tombstone array is introduced.

Any collector marker holds hierarchy purge, including malformed, future or
completed markers. An unreleased retained home also holds purge. Acknowledging
one group is not acknowledgement of final-home disposal. This slice exposes no
marker-release method. A/C must implement a qualified final-home release and
protect retained backup/checkpoint objects from all independent deletion paths
before activation. In particular, hierarchy retention alone does not fence
checkpoint object retirement or already-admitted external cleanup.

## Deployment and follow-up proof

Server startup does not schedule collection. The optional observation command
only reads authenticated Mongo authority and writes a private artifact. Its
input must be an absolute 0600 file; its create-only output is 0600 under a 0700
directory. Artifacts contain private bindings and must remain private. Console
output contains counts and a hash, never raw identifiers or source contents.

No production execution is authorized by this document. Old readers/writers must
be excluded through C's qualified reader floor before progress or transition
writes. A completed B1 review is not proof that a home can serve again.

B2 requires A's V1/V2 enumeration, online `collect_group`, durable reconciliation
after later groups, and a fresh-cursor scheduling contract. After a mutation the
source revision changes, so the next observation must use a fresh candidate page.
Rollover currently refuses pending operations but does not itself establish fair
scheduling under continuous revision churn. Fair bounded scheduling, cancellation,
archive-full/unavailable behavior and the combined lifecycle proof are required
before an executing collector is review-ready.

Required focused tests include complete indexed reads, allowed/retained/refused
eligibility, read counts independent of unrelated history, missing-index refusal,
two-worker CAS, restart identity, ordered acknowledgement, explicit page completion,
purge/acquisition races and preservation of unknown state. The integrated caller
must additionally prove both store formats past the legacy bounds and actual V2
page limits, including changed source/authority and crash boundaries. The current
contract does not claim those lifecycle tests or the production recovery have run.

## Migration record: 2026-11-11-am-ledger-collection-v1

Phase: expand, writers closed. Apply the [data migration standard](CMA_Data_Migrations.md).
The integrating PR must pin the reviewed source/checksum and the complete
release batch; no executable image or recovery artifact is qualified here yet.

| Required field | B1 boundary and outstanding release evidence |
| --- | --- |
| Service and owners | Agent Manager; Track B owns observation/progress and hierarchy retention. Track A owns shared registration/indexes and source/archive transitions. Track C owns maintenance/exclusion. The release owner qualifies and executes production steps. |
| Affected stores and writers | A new top-level marker on `sandboxes`; complete additive Agent/Workspace/Worktree indexes supplied by A. Observation reads all four collections. Server startup only installs A's reviewed indexes; it does not enqueue, claim or run collection. The progress store has no production caller in B1. |
| Format and preservation | `runtime-ledger-candidates-v1`, `runtime-ledger-authority-request-v1`, `runtime-ledger-authority-batch-v1`, `runtime-ledger-group-authority-v1`, `runtime-ledger-group-evidence-v1`, `runtime-ledger-collector-progress-v1`. No TTL, validator contraction, identity replacement, history rewriting or key changes. Unknown formats hold. |
| Prerequisites | Held storage foundations #1659/#1663, C's frozen binding and A's registration/index hooks. Executing collection additionally needs A's acknowledged transition adapter, C's durable activation and reviewed external writer exclusion. |
| Reader floor | Before any progress write, every hierarchy purge reader must recognize marker presence, including malformed/future markers. Readers that ignore the marker are excluded even if they can decode the Sandbox. Old/new exact-artifact execution remains unverified. |
| Writer gate | No server/worker registration calls the progress writer or runtime collector. The observation command is read-only. Future integration must enforce C's default-closed epoch/permit, compatible reader floor and exact cohort; a local lease never supplies those permissions. |
| Migration/backfill | No production backfill or marker initialization is run by B1. A owns additive index initialization and its partial-DDL/readiness evidence. Runtime observations require every hinted index to exist; no fallback scan or inline creation. Later progress uses parent/progress CAS and bounded pages. |
| Backup and dry-run | Observation is nonmutating and does not require a destructive-transform backup. Source removal is absent. Before B2 activation, C/release must qualify independent full-home backup and cold archives, including checkpoint cleanup exclusion and restore rehearsal. |
| Rollback/forward recovery | Before marker writes, B1 adds only readers and additive indexes. After writes, never return to an API that can purge through a retained marker; use a qualified compatible recovery artifact. Closing writer admission does not remove this floor or settle pending operations. No marker clearing or schema downgrade is a rollback. |
| Completion evidence | Local contract/Mongo/Clippy results belong to the exact B1 head; missing results stay unverified. B2 needs both-format lifecycle and crash/restart proof. Production image digests, reader/drain receipts, configuration, counts/hashes, backup evidence and final release/closure belong to the integrating release record. |
