# Runtime ledger retirement evidence and archives

This read-only preflight compares V1 runtime journal and authority files with a
consistent observation of the API's retained hierarchy. It reports occupancy and
conservative candidate counts. A separate command builds a detached archive from
an independently pinned byte backup and observation. These tools do not remove
source records, reconcile, restore, start, stop or update a Sandbox. Their output
is not maintenance authority or proof that an old runtime writer has stopped.

## Export authoritative observations

The release owner runs the service binary with its existing authorized Mongo
configuration, outside normal API startup:

```sh
cma --ledger-retirement-snapshot SANDBOX /absolute/private/new-observation.json
```

Use the existing `CHRONO_MONGODB_URI`, `CHRONO_MONGODB_DATABASE` and
`CHRONO_MONGODB_AUTHORITY_MODE`. Do not put credentials in command arguments or
shared notes. The command verifies the existing service database authority; it
never initializes collections, indexes or validators. A single snapshot-read
transaction reads the Sandbox and its Workspace, Worktree and Agent records,
then aborts even on success. Both projected and authoritative Sandbox lineage
are queried and checked, together with retained Workspace/Worktree parent
references so inconsistent child lineage cannot hide behind corrupt Sandbox fields. Malformed records, mismatched projections, duplicate
resource names or incomplete parent lineage refuse the observation.
An Agent log quarantine marker also refuses the observation, even on a Deleted
Agent or when the marker has a malformed or future format. The exporter retains
that integrity evidence and does not treat terminal status as overriding it.

The destination must be a new file in an operator-controlled private directory.
It is created with mode 0600, synced with its parent, and never overwrites an
existing file or symlink. On publication failure, any owned output is invalid;
use a new destination on retry. Treat the file as private: it contains the
Sandbox/home binding and hashes of eligible resources. Keep the reported
`snapshot_sha256` independently. Console output contains only fixed fields,
counts and hashes; this command does not enable the service logging subscriber.

Only Deleted Agents with a valid deletion time, no pending work or lease,
cleanup debt or cooldown, recovery, pending skill installation,
active turn, queued message,
blocking conversation operation or unfinished first message qualify. Their
Worktree must be Removed, name a settled Agent and have no unsettled attached
Agent. Stopped or Failed alone is insufficient. Missing records remain unknown.
The exporter bounds each child collection at 16,384 records, source BSON at
32 MiB, output at 4 MiB, each query at 10 seconds and the complete database
operation at 60 seconds. Exceeding a bound refuses; there is no partial report.

## Inspect retained runtime ledgers

Using the exact assigned Sandbox/home/generation configuration and a supported
read-only mount, the release owner runs:

```sh
cma-runtime --inspect-ledger-v1
cma-runtime --preview-ledger-retirement /absolute/private/new-observation.json SNAPSHOT_SHA256
```

Runtime configuration includes the original `CHRONO_SANDBOX_ID`,
`CHRONO_RUNTIME_HOME_IDENTITY`, `CHRONO_RUNTIME_GENERATION`,
`CHRONO_RUNTIME_HOME`, `CHRONO_RUNTIME_STATE_ROOT` and retained `TMPDIR` mapping.
Use the original paths expected by the persisted records. An identity or
generation mismatch refuses. No runtime supervisor, reconciliation or normal
recovery is started. Do not start an ordinary runtime to obtain these counts.
Provider access, mount qualification and production execution belong to the
release owner; these commands do not supply those capabilities.

Occupancy counts distinguish canonical files, temporary replacements and
historical process generations. V1 serving scans include the persistent root
and current generation. Journal temporaries count toward its 1,024-file limit;
authority temporaries are reported separately from the 4,096-record limit.
Historical files are retained but excluded from the current serving counts.
Counts at or above these limits can still be inspected within the separate
bounded offline inventory limits. These are physical occupancy figures, not a
promise of admission headroom: pending outcomes and cleanup reservations matter.

Preview verifies the exact snapshot digest, validates the whole ledger history
without mutating it, and requires completed absence receipts plus Absent
authority for every resource in a candidate group. Skills additionally require
completed removal of their Agent home. An API deletion observation alone is
insufficient. Started, Failed and Indeterminate journal records keep their
entire group. A source temporary keeps every group. Unclassified files remain
retained. The retirement index has an additional 8 MiB metadata budget. Current-scope
candidate counts are separate from historical candidates. The command rechecks
source inventory before reporting. Concurrent
source change holds; an unchanged scan still does not prove quiescence.

The report explicitly includes `source_mutated: false`, `apply_authority: false`
and `stale_replay_protection: required_before_archival`. Save counts and hashes
in release evidence; never paste the private observation, raw receipts or
resource identities into GitHub. Zero candidates means no proven candidate,
not evidence that all remaining resources are live or that cleanup is impossible.

## Build a detached retained archive

After the preview, create and pin a complete byte backup with the existing
offline planner. Then build a separate archive from that exact backup and the
independently pinned API observation:

```sh
cma-runtime --plan-ledger-v1 /absolute/private/new-plan
cma-runtime --verify-ledger-plan /absolute/private/new-plan PLAN_SHA256
cma-runtime --stage-ledger-retirement /absolute/private/new-plan PLAN_SHA256 /absolute/private/new-observation.json SNAPSHOT_SHA256 /absolute/private/new-retirement-archive
cma-runtime --verify-ledger-retirement /absolute/private/new-retirement-archive READY_SHA256
```

Both output directories must be new, private and outside the retained runtime
roots and input backup. Keep the original sealed plan and its digest as well as
the resulting archive. The builder reuses the preview's conservative eligibility
rules, copies the selected original journal/authority bytes into a durable
archive, and retains the observation alongside it. Live, missing and uncertain
groups stay outside the archive. It never rereads active source bytes after
opening the sealed plan; the original runtime can therefore change independently
without making this artifact permission to reclaim its newer state.

The output reports only fixed fields, counts and hashes, including
`ready_sha256` and `database_sha256`. Retain the ready hash independently. The
verification command reads and hashes the detached archive and observation,
checks their private file shapes and home/generation binding, and emits fresh
counts/hashes without opening SQLite for writing or echoing arbitrary metadata.
Each group remains pending. The report says
`admission: retirement_held`, `writer_exclusion: unproved`, and
`source_deactivated: false`. Zero candidates is a valid empty archive. A rerun
at the same destination refuses without overwriting it. An interrupted output
without a valid `ready.json` is incomplete; use a new destination when rebuilding
from the same pinned inputs. A replacement output pathname cannot redirect writes
into source state or another directory.

This command supplies retained bytes for later reviewed maintenance. It does
not stop a runtime, publish activation metadata, free active-ledger capacity,
complete a retirement receipt, or supply restore/production execution authority.
Keep these private artifacts under the release owner's retained-backup policy,
independently of any PVC that may be released when its Sandbox finishes retiring.

## Activation and restore boundary

The runtime now understands a required retirement archive dependency. Its exact
descriptor binds the archive UUID, retained home, roots, creation generation and
active V1/V2 format. Startup opens the existing archive and refuses any pending
deactivation before home initialization or installation recovery. Missing,
partial, replaced or mismatched metadata holds startup; an unactivated process
also holds when activation artifacts appear while it is serving.

Admission checks the complete prepared pair before either active ledger writes,
then checks retirement again at journal begin and recovery. A retired group
refuses permanently, including another valid operation on that Agent. Both
archive payloads and scoped operation identities remain retained. The required
installation marker is unknown to earlier readers, which refuse before
installation recovery. This code-level refusal still requires qualification
against the exact approved rollback/recovery images before activation.

These are reader capabilities only. There is no supported activation publisher,
source deactivation command or automatic collector yet. Do not manufacture the
descriptor or marker to enable them. Activation metadata is currently produced
only by isolated tests.

Cleanup apply and restore are not implemented by these tools. Retained exact
receipts and authority still protect against stale replay. Moving them out of
the active lookup without permanent retirement refusal would make some old
operations appear new. A future archive operation must publish and verify a
retained backup plus durable replay protection before deactivating originals,
with old-reader refusal before effects, crash recovery and bounded active-state
checks for both ledger formats. It must reconcile durable API retirement intent
and prevent a lost acknowledgment or API purge from losing eligibility forever.

`--cutover-ledger-v1` remains an explicit hold under #1461. Do not bypass it,
manually edit Mongo, remove Pods as repair, or manufacture format markers.
No restore path may roll the ledger back after new effects. Those guarantees
require separate reviewed execution and release capabilities before cleanup.

## Held source transfer and restore core

The exact-image storage qualification now exercises source moves using only create-only synthetic journal and authority records. It verifies durable archival, loss-of-reply retry, source deactivation, held restore and permanent replay refusal. The private retained directory preserves the original source inode and bytes. Neither direction overwrites an existing destination. Both directories are synced before a move can qualify as complete.

Before restoring even one record, the archive commits a pending state. A failed or interrupted restore therefore refuses serving startup. Retrying restore accepts only the exact retained original or an already-restored exact original; a changed/new active record, duplicate, missing pair, symlink, hard link or directory replacement refuses. Restore does not erase retirement indexes or authorize an older image to serve. This primitive does not prove that no later effects occurred elsewhere in the home; the full restore-admission workflow must enforce that boundary.

This is the source-transition core, not the production cleanup command. It requires the caller to own a proved exclusive writer hold and a complete eligible group selection. A file lock on the archive cannot exclude an old raw writer; descriptor checks cannot replace that exclusion. The qualification fixture does not exercise the actual retained-home storage class, physical fencing, operator activation, V2 accounting or automatic authoritative collection. Those remain required before cleanup of a real home.

## Settled V2 accounting

Finishing a V2 mutation removes its generation's derived scope-budget row when
both pending-work and logical-process counts equal zero. The removal commits
together with the receipt and counters. A storage failure rolls back that whole
transaction, preserving recovery from the started receipt.

Admission and recovery already interpret a missing budget row as zero, and an
admission recreates the row when needed. The persistent format is unchanged.
Journal receipts, authority, cleanup reservations and nonzero budgets remain
retained. Historical empty rows are still valid input; untouched historical rows
are not swept by this change. This prevents new settled accounting rows from
accumulating across generation turnover. It does not retire resource history,
bound either full history store, or repair an exhausted home.

## Held indexed-store archives

The V2 source inventory uses bounded primary-key pages and reconciles global and
per-generation accounting before its final page succeeds. Source columns retain
raw operation identities and receipts, all folded authority blobs and high-water
values, cleanup reservations, logical processes and scope counters. The inventory
also binds store identity, generation, roots, accounting revision and page geometry.
A source change invalidates continuation; this read-only audit establishes neither
eligibility nor exclusion of writers.

Completed V2 mutations discard a generation's derived budget row when both its
pending-work and logical-process counts reach zero, in the same transaction as
the receipt and counters. Existing admission and recovery readers already treat
an absent budget row as zero. This prevents new empty generation rows from
accumulating; journal receipts, authority, cleanup reservations and all nonzero
budget rows stay retained. Historical empty rows are still valid input. This
accounting cleanup does not retire resource history or repair an exhausted home.

Archive schema 2 adds exact encoded V2 rows and their source context to the existing
retirement index. Preparing one selected group commits its payloads and every
scoped operation identity together. A copy error rolls that transaction back.
After a lost reply, retry verifies the committed context, raw payloads, count,
source digest and replay indexes without rereading mutable sources. Both archived
and pending groups refuse later mutations on the retired resource. Shared scope
counters are retained context; they are not independently retired resources.

Binary source columns use base64 strings so a legal 1 MiB journal plus its raw
identity fits the unchanged 2 MiB SQLite value limit. New writes include separate
keys and SQLite row headers in that budget. Readers also accept earlier numeric
byte arrays, preserving raw bytes, NULL fields, source digests and retry receipts
without rewriting a prepared archive. Earlier numeric-only V2 checkpoint readers
cannot read compact payloads; these checkpoints have no production activation,
and pending groups continue to hold serving. This is not recovery-image approval.

This representation requires retirement descriptor version 2 as well as archive
database version 2. New readers retain version-1 support and require the descriptor
and database versions to agree. The existing marker filenames remain the reader
barrier: the frozen version-1 descriptor reader refuses version 2 before installation
recovery. This source-level compatibility check does not qualify an actual recovery
image; that qualification remains a release requirement.

The create-only `--verify-ledger-storage` probe now also reports
`indexed_retirement_copy_reopen_refusal: passed` after exercising synthetic V2
archival, reopen, retry, permanent replay refusal and rejection by the version-1
database reader. No active source rows are removed. V2 groups remain pending;
the V1 file-transfer primitive refuses them. Accounting-checked transactional
deactivation, complete-home restore admission, authoritative automatic collection
and a supported real-home activation command remain required before cleanup.

## Verify indexed sources before a transition

Archive inspection validates the complete group, source digest, context and
permanent replay indexes before exposing its first row. It then streams the
original columns in one read transaction, without collecting the whole history
in memory. Corruption in the final row refuses before any callback. Inspection
errors or cancellation never mark the group complete; an owned guard captured by
the storage callback remains held until that admitted worker finishes.
Inspection query and callback errors return a fixed refusal without consuming
the archive or active-store emergency reserve. A failed observation therefore
does not itself persist a new storage-capacity hold.

Active-source comparison accepts pages of at most 64 rows and 2 MiB including
context. Each comparison rechecks the exact store, generation, revision and page
geometry, then compares every original column using the table's primary key.
Raw JSON whitespace and NULL distinctions matter. The operation, authority,
cleanup-budget, scope-budget and logical-process tables are all covered. Missing,
duplicate, reordered, changed or oversized inputs refuse without source writes.
The shipped create-only storage probe exercises archive-to-source comparison.

These are current observations, not a reserved selection or complete-group
eligibility proof. A successful page does not exclude another effect afterward
or prove that omitted rows do not belong to the group. The eventual deactivation
transaction must repeat the source checks under its owned writer hold and require
the collector's complete-group and authoritative retirement evidence. Neither
inspection nor comparison clears a pending archive hold, removes an active row,
authorizes restore or proves reclaimed capacity.
