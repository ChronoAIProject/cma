# Durable held ledger completion

The held V2 completion coordinator retains original collection evidence and an
immutable final outcome. This is a core capability with synthetic qualification
callers; it does not expose a retained-home CLI, enable an online collector,
construct a maintenance permit or change an Agent's lifecycle. Follow the
[release runbook](CMA_Ledger_Release_Runbook.md) for the executable observations
and the independent production gates.

## Explicit archive reader floor

Archive format 3 adds original-evidence, bounded pending-operation and completion
tables to format 2. Active ledger V2 is unchanged. An activated dependency must
name retirement descriptor version 3 and pin an active V2 descriptor. New readers
still support archive versions 1 and 2; the descriptor/database pair must agree.
Version-1/2 archive readers reject version 3 before effects. No existing archive
is silently upgraded and no production activation publisher is introduced.

The bounded pending index blocks startup, recovery enumeration and already-open
serving readers, including the supervisor's ordinary, browser and Workspace
operation paths after acquiring their operation gate. This is a necessary runtime
barrier, not physical exclusion of an old process or delayed provider operation.
C must install its durable maintenance hold before using this core and retain it
after per-group acknowledgement until its separately verified home release.

## Original bytes and completion protocol

`Archive::retain_collection` accepts validated original candidate/request/observation
bytes and the independently accepted C plan hash. It commits and reads back the
exact originals before B may persist Selected. Candidate, request and observation
limits remain 2 MiB, 64 KiB and 4 MiB. Original artifacts are stored as ordered
64 KiB chunks in the same transaction; this preserves the existing 2 MiB SQLite
value/row limit. Readers bound lengths, chunk count and ordering before decoding.
No reserialization replaces accepted bytes.

The shared `LedgerCollectionArtifactReferenceV1` lives in the adapter and is re-exported
by core; the runtime uses that same type. It contains explicit format,
archive UUID/version, operation ID, group hash, common binding hash, C plan hash,
artifact-bundle fingerprint and ordered original lengths. Every field and the
exact bytes are read back from the archive before success. The
bundle fingerprint includes length framing and exact bytes. The common binding
hash, B group-observation hash, C plan hash and artifact fingerprint retain their
separate meanings. Exact retries may load historic accepted generations; a fresh
intent must match the current generation. Changed bytes, binding or plan refuse.

One pending operation is allowed per archive. An operation chooses one complete
resource group; a new mutation revision requires fresh candidate/authority evidence
before another group. A multi-group authority response is not permission to reuse
its operation ID or stale source revision for several source transactions. Retain
cold evidence and acknowledgements; a finite archive applies backpressure when full.
The one pending slot cannot be overwritten by a later group.
`pending_collection(accepted_binding)` discovers an original staged intent after
a lost put reply or lost Selected publication. B reconciles it before refreshing
source evidence, even if its own projected pending field is empty. Missing or
changed original bytes hold; no no-effect cancellation is inferred.

The held coordinator consumes the archive and active-store owners. Detached work
owns both handles and the caller's exclusion guard through verification, source
commit, acknowledgement and closing both stores. Caller cancellation cannot release
that guard while admitted work continues. The generic internal guard is not C's
qualified `ValidatedLedgerExclusion` and cannot be presented as production authority.

The sequence is:

1. Retain exact accepted bytes and the pending slot; read them back. The slot
   already blocks serving, even before the selected group is archived.
2. Archive the complete selected group, source context and permanent replay indexes.
   Fresh preparation must match that durably retained intent and source revision.
3. Execute the existing audited source transaction, atomically removing eligible
   rows and adjusting accounting. Live/uncertain/incomplete groups still refuse.
4. In one archive transaction, retain the final receipt, mark the group complete,
   settle its evidence and clear the pending slot. Read back the result.
5. Close both stores before replying. B persists the acknowledgement reference
   before advancing; C independently decides whether the whole home can be released.

Source and archive are separate databases. There is no cross-database transaction.
A source commit with a lost reply is reconstructed from the exact immediate
before/after states while pending intent prevents another serving/collection effect.
A later retry uses the immutable cold receipt, so later groups cannot overwrite
its outcome. Missing/changed artifacts, corrupt indexes or an inconsistent pending
slot refuse; missing rows or matching counters alone never mean success.

`CollectionCompletion` uses `runtime-ledger-transition-receipt-v1` and phase
`acknowledged`, with the frozen binding/operation/plan/group/source/observation
hashes, before/after hashes and revisions, removed counts and free-page counts.
Its domain-separated hash covers every receipt field. This slice returns only the
final acknowledged receipt; intermediate boundaries are held internal state, not
new public Prepared/SourceCommitted APIs. A receipt proves a group outcome, not
consent, no-newer-effects restore admission, unlimited disk or home release.

## Qualification and remaining integration

Tests cover exact original bytes and maximum observation/request sizes, changed
plans/bindings, second-pending refusal, source-revision drift, chunk/slot/artifact
and acknowledgement damage, source/ack lost replies, process exit, cancellation
at each coordinator boundary and retry after later lifecycles. Archive versions
1/2 retain their prior readers; format 3 requires active V2 and refuses old archive
decoders. The create-only storage probe reuses its synthetic source/archive pair
for both older held protocols and format-3 completion within the existing reserve
footprint. It never opens a retained production home.

The online `collect_group` port, source-worker barrier, B2 execution/cursor wiring,
C maintenance writer matrix and reader/schema gates, qualified late-writer
exclusion, real-home activation/restore and authorized Failed-Agent recovery remain
integration prerequisites. Restore requires independent proof of no newer home,
conversation/provider or external effects. Permanent replay refusal and backups
remain retained. This slice alone does not repair the incident cohort or establish
bounded operation across all future lifecycles, including unresolved outcomes.
