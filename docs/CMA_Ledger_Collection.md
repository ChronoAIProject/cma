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

## B2 coordinator checkpoint and integration boundary

`layer4-runtime/src/ledger_collector.rs` is an executing coordinator skeleton
against private Track B interfaces. It has no production `CollectorPort`
implementation or worker registration. Its view types are not additional wire
formats and do not replace A's receipt or C's binding/permit types. A owns the
eventual registration and adapters. No new environment flag is introduced.

Each call loads progress and performs at most one durable step or dispatch of
the original operation. Scheduling admission defaults closed. Closing admission
still permits reconciliation and settlement of an already selected operation;
it does not admit another group. Every effect path requires an owned hold from
the adapter. The real hold must contain C's validated authority plus A's gate
and ordered storage barrier, and admitted detached work must retain it after
caller cancellation. The trait itself supplies no exclusion.

The coordinator validates the original intent against each projected receipt and
records Prepared, SourceCommitted and Acknowledged in order. Missing evidence,
uncertain reconciliation, changed authority or unavailable/full archives preserve
pending work. Acknowledgement and fair position must be committed together before
another group is scheduled. A changed source revision causes a fresh page; opaque
cursors are never reinterpreted against a different snapshot. A completed sweep
checks for changed source work before returning complete again.

The proposed fair-position adapter preserves the last classified group across
revision refresh within a sweep. Fresh enumeration starts strictly after that
key. Completion is an explicit inventory boundary; a later sweep starts from
the beginning and revisits retained or newly inserted earlier groups. This keeps
earlier churn from repeatedly moving the same bounded tail out of reach. It does
not prove fairness under arbitrary unfenced writers: A/C must provide a bounded
qualified gate window, and the scheduler must bound deadlines and opportunities
between homes. No collector result releases a home or certifies empty occupancy.

The tests include an in-memory fault model and a reusable lifecycle driver
interface. Both model format selections run 1,400 cycles with modeled totals of
8,400 operations and 4,200 authority lifetimes, restart points and retained-replay
checks. They exercise the harness and coordinator only. They do not execute the
V1 journal, V2 SQLite, Mongo, a provider, a restart of a real process or physical
exclusion. Actual V1/V2 drivers must supply measured rows/bytes/pages, exact
original receipts after later groups, and real replay denials before these tests
can serve as the combined lifecycle proof. The model's page counters are not
measurements of SQLite allocation or disk headroom.

## B2 prerequisite design: exact evidence survives before Prepared

This is an integration proposal for A/C review, not a new archive schema written
by B. Keep one bounded staged operation per home in the existing accepted-plan or
archive authority. It must survive before B writes Selected, including a lost
response and worker-fence takeover. Never allocate a fresh logical operation just
because the stage publication or Selected CAS response was lost.

1. Under the qualified hold, resolve the immutable admission key from the full
   binding, original candidate revision/page and group. A worker fence is not
   part of logical operation identity. An existing staged slot is reconciled
   first, including after a changed candidate revision; conflicting content or
   an unowned slot holds rather than being overwritten.
2. Publish the original operation ID and exact page, request, observation and
   selection references durably. Retain byte lengths and SHA-256 for each original
   artifact. Validate the full binding and explicit format on readback. A owns
   archive/selection encoding; no JSON reserialization is accepted as original
   bytes. Source selection keeps its existing 16,384-row/32-MiB bound and the
   archive's 64-MiB bound; B's request/observation bounds remain unchanged.
3. Only successful immutable readback permits the Selected progress CAS. An
   uncertain publication is resolved by the same operation and admission key.
   A CAS loser can retain one unresolved stage but cannot create an unbounded
   chain of abandoned artifacts or dispatch source work without selection.
4. On takeover, reload the exact original artifacts before asking A to reconcile
   or resume. Missing bytes, a changed plan, unknown completion or archive pressure
   hold. The staged slot is not reclaimed by elapsed time or lease expiry.
5. After A's durable acknowledgement, atomically retain its reference and fair
   position in B's progress. The active slot can then be replaced under CAS; cold
   original receipts and permanent replay indexes remain available after later
   groups. Final retention release belongs to C's separate home transition.

Required combined tests: interruption before/after stage publication and readback,
before/after Selected CAS, changed worker fence, competing original requests,
missing/truncated/reencoded evidence, occupied/full archive, and acknowledgement
reply loss followed by later groups. Real adapters must prove the protocol through
their persistence primitives; the coordinator model cannot establish durability.

## B2 prerequisite design: checkpoint deletion cannot race retention

Hierarchy retention covers the final parent delete, but external object cleanup
has separate admission paths. The reviewed integration must cover
`checkpoint_objects/purge.rs` (`purge_sandbox`, `retire_deleted_sandboxes`,
`cleanup_due` and `cleanup_one`) and `checkpoint_objects/tombstones.rs`
(`cleanup_due`, including already-admitted retries). A tombstone may outlive the
manifest, so inspecting only published manifests is insufficient.

The preferred qualification is for C/release to retain the full-home backup and
cold archive in existing independent roots that these cleanup paths cannot name
or delete. Prove object/version identity, access and retention policy, absence of
aliases, and a restore rehearsal. A root hash or a metadata declaration is not
proof of that separation. If this qualification holds, do not add unrelated
checkpoint writes merely to manufacture a second retention mechanism.

If a retained dependency shares these cleanup roots, A/C must integrate a durable
deletion-admission fence. Retention acquisition and new deletion admission must
write the same authoritative gate transactionally with the referenced parent/root
state. Every cleanup path must claim that gate before an external delete; a
read-only marker check followed by a delete leaves a race. Existing deleting
manifests and tombstones must participate with their exact original object keys.

An already admitted or uncertain provider delete cannot be undone by inserting a
retention marker. Acquisition must refuse qualification until the original delete
is proved final or an independently protected replacement backup is qualified.
Cancellation of an RPC or worker lease expiry does not prove external finality.
Never clear tombstones, reset pending deletes or claim a missing backup succeeded.

The reader floor must exclude every cleanup worker and recovery executable that
ignores the fence. Tests must race acquisition against each admission path, lose
delete replies, retain an already-deleting manifest, retry an old tombstone, and
prove no external delete after retention wins. Root-alias and missing-object cases
hold. Release must supply the provider/storage finality evidence; this design and
the B1 parent-row race test do not replace it.
