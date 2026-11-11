# Verified permanent archives before retirement

Tracking: #1716 (code child of #1668), #1443 and #1461. Production remains HOLD.
This capability is an explicit operator command. It does not run at startup or
from a service scheduler. Each invocation defaults `enabled` to false. Installing
this binary does not admit an archive requirement or release an existing one.

## Required release qualification

The release owner supplies reviewed physical writer/provider exclusion, the
independent permanent destination and the exact private invocation manifest.
The code does not manufacture those facts from a timeout, stopped Pod, expired
lease, RWO volume, local lock or caller-provided hash. The release requirements in
[CMA Ledger Release Runbook](CMA_Ledger_Release_Runbook.md) continue to apply.

For the four owner-approved Retiring sources, qualify all four non-destructive
holds before a deployment can resume retirement workers. Keep their original
retirement intent. After complete archive verification, finish that intent through
the existing retirement workflow. Do not change the parents to Active. One pilot
precedes the other three, serially; no production phase starts at 02:00–02:59 SGT.
A merge, command success or synthetic test does not authorize production execution.

## Supported command and private inputs

The entry point is `cma --retirement-archive ACTION PRIVATE_INVOCATION_JSON`.
It exits before starting any server or initializing any Mongo schema. All private
input files must be regular files, owned by the operator, with no group/other
permissions. Symlinked path components are rejected. Credentials never appear in
arguments, output summaries, manifests or archives. Results are written create-only
with mode 0600. Standard output contains only a result digest and byte count.

Invocation fields (unknown fields are refused):

- `enabled`: false by default. Must be true for every action except inspect/dry-run.
- `request_file`: absolute path to the original request bytes. Preserve it unchanged.
- `output_file`: absolute path to a new private evidence file for this invocation.
- `custody`: `bucket`, optional `create_token_file`, and `verify_token_file`.
  Tokens are short-lived scoped access tokens in separate private files. The
  ordinary CMA service/runtime receives neither archive credential.
- `restore_directory`: a new directory below an operator-owned private parent.
- `manifest_reference`: only needed for independent `readback`; use the exact
  immutable reference from capture, including its object generation and digest.

The request binds format 1, a canonical nonnil UUID `operation_id`, `sandbox_id`,
`expected_revision`, exact original BSON `source_sha256`, original retirement
event `retirement_intent_sha256`, authoritative `home_identity`, `cohort_sha256`,
`epoch`, complete `roots`, `exclusion_receipt` and its exact `exclusion_sha256`.
Each root includes `role`, absolute `path`, filesystem `device` and `inode`.
Roles home/state/transfer are required; include attachments when independently
mounted. The release census must establish that this is the complete root set.
An existing unknown mount or external reference is a stop condition.

The private exclusion receipt repeats operation, parent, source, retirement intent,
cohort, epoch, home identity and the SHA-256 of the JSON root list. It also binds
`writer_matrix_sha256`, `provider_finality_sha256`, `physical_fence_sha256`,
`qualified_at` and `expires_at` (at most 24 hours). These hashes identify retained
release-owned proof. They are not a signature or proof of physical fencing. Every
source-dependent phase checks the exact bytes and expiry. Expiry leaves the durable
hold intact; it never resumes retirement. A renewed authority requires a reviewed
continuation, not editing the original admitted request.

| Action | Behavior |
| --- | --- |
| `inspect` | Reads the original parent and retirement event; records their hashes, revision and supported namespace hash. Does not modify Mongo or custody. |
| `dry-run` | Reads and hashes the complete supported source; reports counts/hash without Mongo or archive writes. It requires the same complete source and exclusion inputs, but does not claim custody or apply authority. |
| `hold` | Transactionally retains exact original request/source/intent bytes, advances parent revision and writer fence, and admits the durable pending archive requirement. Replay requires identical original bytes. |
| `capture` | Captures files, complete scoped BSON records and referenced document bodies, twice; any source drift refuses publication. Retains every component and the immutable manifest, then stores its exact reference. |
| `restore-check` | Reads every object with the independent verifier credential, reconstructs into a new isolated directory, checks byte hashes, BSON framing/IDs/counts and the entire file namespace; retains the verification receipt. No Agent starts and no input is replayed. |
| `complete` | Rechecks immutable source/custody/restore bindings, current exclusion and the source census; records the verified receipt with a parent revision CAS. Existing retirement workers may then continue the original request. The command itself does not delete source data. |
| `readback` | Requires only the independent read-only credential, original request and exact manifest reference. Reconstructs/verifies again without Mongo access, including after source retirement. It refuses an archive-creation credential. |

A fresh evidence filename does not create a fresh logical operation. On a lost
reply, reconcile the exact original request. Source gate transitions and manifest
references are idempotent; no overwrite or clear-gate command exists. Retain partial
archives permanently, including abandoned chunks. A failed isolated restore leaves
its private output for diagnosis; retry in another new directory, never over it.

## Capture and restore format

Mounted roots are traversed without following symlinks. The manifest retains raw
filename components and symlink targets in base64; POSIX metadata, hardlink identity
and extended attribute names/values are preserved. Regular files are chunked into
4 MiB objects. Directory, symlink, socket and FIFO types are explicit; devices,
changing metadata, unenumerable roots and unsupported attributes fail closed.
The complete file inventory is bounded to 200,000 entries, depth 64 and 1 TiB; each extended
attribute inventory is bounded to 128 names and 1 MiB. No budget truncates output. Nested roots may overlap when separate roles share
the same mount; each role is captured and checked explicitly.

The service captures full BSON documents, not projections, in a single Mongo
snapshot session. It discovers and validates the complete Sandbox/Workspace/
Worktree/Agent hierarchy, including Failed and Deleted Agents, and captures the
matching records in every declared service collection using authoritative lineage
and retained references. The existing full-log scanner checks sequence continuity against each Agent's
authoritative head and refuses quarantine or mismatched creation lineage. Exact
BSON length framing is preserved. Embedded Agent
artifact bodies are included. Document artifact revisions are read through the
existing document store adapter and matched to their metadata hashes and sizes.
A second independent snapshot/census must match before manifest publication.
The Mongo census is bounded to two million documents and 32 GiB; exceeding a
budget refuses capture. The command normalizes its own parent hold bookkeeping to the retained original
parent; the full archive requirement and receipt remain separately durable.

Legacy external checkpoint objects/tombstones and channel media requiring retired
or unsupported source transports refuse capture. They cannot be treated as empty
or complete from their metadata. Their source reader and qualified root closure
must be supplied in a separate reviewed extension before those sources retire.
The same rule applies to a newly introduced segmented conversation backend:
its retained payload closure must be present in the pinned reader and rehearsal.

Isolated reconstruction writes ordinary files with private non-executable modes,
recreates hardlinks and symlinks without traversing them, and represents closed
socket/FIFO entries without starting a process. Original ownership, modes and
extended metadata remain in the verified manifest; privileged metadata is not
reapplied during inspection. Service records reconstruct as exact BSON dump files
with duplicate IDs, malformed/truncated frames and count/hash mismatches refused.
This is a byte-preserving offline reconstruction. The release rehearsal must also
prove import/readability with the approved Mongo/runtime/provider readers on the
matching topology. The tool does not certify a production engine restore from
successful BSON parsing alone. Recovery into a serving identity is a separate,
explicitly reviewed operation; it is never automatic or an input replay.

## Independent permanent custody

Each operation uses a release-approved independent GCS bucket dedicated to that
operation, labelled `cma-retirement-operation` with its exact UUID. A mismatched
label refuses all publication. Objects live at
`retirement-v1/<operation UUID>/<SHA-256>`, including original requests, source
records, manifests, file chunks and verification receipts. Each write uses
`ifGenerationMatch=0`; a lost acknowledgement reconciles the same content and
generation. Final readback is generation-pinned with an independent credential.
There is no delete, update, overwrite, retention-shortening or object-list API.

Admission requires default event-based holds, a hold on every object, uniform
bucket access, enforced public access prevention, no lifecycle expiry and no
customer-controlled KMS key dependency. It requires distinct create/read roles:
creation may only get bucket policy and create objects; verification may only get
bucket policy and read objects. Candidate permissions for delete, update, IAM,
retention override and bucket modification are refused. Readback needs no creation
credential. Every key is restricted in code to the original operation prefix.

The release owner must independently qualify the actual IAM conditions, inherited
roles, protected destination and permanent policy. The dedicated bucket keeps bucket-scoped permission probes compatible with least
privilege: it contains only this operation's retained objects; other buckets are
unrelated resources and must be denied by IAM. Do not use a shared archive bucket
with object-prefix conditions that a bucket-level probe cannot evaluate.
Bucket permission probes alone
do not prove unrelated-object denial or absence of project/IAM/key administration.
Rehearse denied delete/overwrite/retention weakening/key disable/unrelated access
with the candidate principals. Source retirement/runtime/purge roles must have no
archive authority; archive creation must not grant source deletion. Source and
account deletion must leave retained objects decryptable with approved recovery.
No repository code provisions infrastructure, exports master keys or creates policy.

## Durable gate and migration record

The optional `record.lifecycle.retirement_archive` field is absent on legacy
records and is never written by normal startup. Format 1 records immutable
operation/request/source/exclusion hashes and an optional verified receipt. Missing
verification is a permanent hold, not a lease. The coordinator retains exact original
bytes in the parent, fingerprints the admitted record, and advances usage and the
parent fence transactionally. Ordinary saves and lifecycle transactions cannot add,
clear or replace the requirement. Pending holds refuse parent/child work claims,
destructive Agent/Workspace execution, retirement and terminal purge. Checkpoint
cleanup and legacy tombstone retries also respect the hold. These checks do not
recall work already accepted by a provider; release-owned exclusion is mandatory.

Reader floor: the archive-capable CMA binary from this PR and approved recovery
images built from the same compatible schema. Older strict lifecycle readers reject
the new field. They are not a rollback target after admission. Before the first
hold, drain/exclude old writers and qualify the archive-capable reader. A flag or
image rollback must retain compatible readers and holds; do not remove the field.
No bulk rewrite, startup DDL or change to the shared Mongo envelope version occurs.
After completion, the immutable receipt persists through retirement; remote copies
outlive parent purge. Rollback before release leaves source held. After source
deletion an image rollback cannot undo deletion; use separately approved recovery.

## Acceptance evidence

Local tests must prove successful full capture/reconstruction and source-independent
readback, plus source drift, wrong source/intent/cohort/epoch, missing/corrupt objects,
wrong generations, scope/path substitution, expired evidence, destructive credentials,
stale save/claim/purge and retry after uncertain acknowledgement. Real-Mongo tests
exercise the transaction and usage boundary; HTTP fixtures exercise custody requests.
Production remains HOLD until reviewed artifacts, every source's actual archive and
restore results, E1–E4 physical exclusion and permanent-custody qualification are
retained and the lead relays explicit execution approval. No code-child closure
closes the production or same-Agent-recovery acceptance of the parent issues.
