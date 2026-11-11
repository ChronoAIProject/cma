# Compatible Agent log reader foundation

Part of #1348. This foundation accepts the retained legacy format and the
versioned segmented format. It does not admit adoption or rollover. Writer and
recovery activation require the complete consumer, publication, migration and
rollback gates in the segmentation plan.

## Format and source integrity

An Agent envelope has scalar BSON integer version 1 or 2. Version 1 has no
segment markers. Version 2 requires the exact format, immutable creation
incarnation and head revision; a partial or unknown shape is an error. Other
collection envelopes retain their existing version rules. Revision-only Agent
updates preserve its format discriminator.

`agent_log_heads` stores the bounded active interval, global counters, previous
seal and projection binding. `agent_log_segments` stores immutable manifests
with raw BSON leaf digests and predecessor linkage. Original event documents,
their BSON representations, IDs and global sequences stay unchanged. Manifests
are smaller than one MiB. Each physical segment is independently bounded; no
lifetime segment count or lifetime event-count policy is introduced.

Readers capture the Agent, head and quarantine state in one Mongo snapshot.
They verify the touched manifests, adjacent boundaries and original source
bytes. A page touching the active interval checks that whole bounded interval
once. Empty pages still check the active interval and unexpected rows past the
captured committed head. Concurrent writes cannot mix source snapshots.
Missing proof never authorizes a legacy fallback. Reads do not prepare, repair,
migrate, change quarantine, or create collections.

## Callers and bounded memory

The existing prepared legacy proof remains authoritative, including durable
quarantine and cohort controls. Legacy read-only fallback retains its existing
complete-scan accounting. A segmented Agent cannot enter a legacy-only complete
scanner; consumers must use the format-aware range or their indexed projection.

The streaming adapter delivers one event at a time. Its optional immediate
predecessor shares the page snapshot, allowing steering correlation without a
new active-tail validation for each message. Public windows retain the current
entry and byte bounds, validate requested source rows even after the projected
page fills, and recheck authorization before release. Point reads verify the
same segmented source proofs. Source BSON telemetry counts partial consumption
on failure as well as successful reads.

The retained Vec interface is compatibility only. Segmented requests exceeding
its physical row or replay byte budget fail explicitly. Full history remains a
paged consumer obligation under #1349; this foundation does not claim complete
transcript or response projection support.

## Captured source and context

The captured-source port requires exact equality with the supplied Agent and
preserves the existing quarantine proof. Legacy capture folds one physically
bounded prefix once and retains semantic state, the latest entry and original
BSON leaf digests. Segmented capture validates the exact projection/head binding,
its certified generation maxima and one bounded active interval. It reads a
sealed latest entry separately when the active interval is empty.

Requested points must be unique, ascending, inside the captured prefix and within
the source-byte budget. They check only requested sealed membership and reuse the
captured active hashes. Reads in a transaction also recheck the aggregate/head
because a transaction can see its own writes.

Snapshot proofs bind the established snapshot, session and database. The initial
read requires a snapshot timestamp returned by the server without forcing a
snapshot concern onto an ordinary session; a cached driver timestamp alone is
insufficient. Transaction
proofs instead bind the private stamp from the exclusive `TransactionScope`;
consuming that scope releases the session for caller commit or abort. A later
transaction gets a fresh stamp, even on the same session. A raw transaction-mode
argument cannot establish authority. Capture and points do not start, commit or
abort the transaction or write source, sidecars or quarantine.

## Bounded audit persistence

Audit progress version two separates the completed prefix from the current unit.
One read-only snapshot validates a physical unit. Its derived evidence is staged
under a private generation in batches bounded by both encoded bytes and row count.
Each short transaction writes the audit fence and batch together. Source payloads
are never copied into evidence or retained as a lifetime vector.

Only complete source, hash, semantic and queue checks authorize the promoting
phase. Promotion copies bounded batches into the audit result generation and
commits its cursor atomically. A crash during collection restarts only the current
unit under a fresh generation. A crash or ambiguous commit during promotion resumes
from durable progress without refolding source. A replaced holder or cancellation
fails the fence before any evidence batch can commit.

The result generation stays invisible to readers until exact captured Agent/head
CAS installs its projection and pointer. Completed audits cannot overwrite a newer
aggregate. Corruption records an audit quarantine; this work does not clear prior
quarantines or delete abandoned evidence. The additive schema includes an index
for bounded generation traversal. The historical single-transaction audit progress
shape is refused explicitly rather than treated as resumable version-two progress.

Qualification includes both original near-capacity fixtures, the existing query
and time bounds, and measured per-transaction write limits under the canonical
Mongo cache. Exact passing results belong in the review evidence; source and tests
alone are not an executable or activation claim.

## Additive migration and retention

The reader migration adds the eight sidecars: heads, segments, publications,
projections, evidence, audits, rollout gates and exact publication admissions. The shared migration runner owns source and
checksum identity, authority, lease renewal and completion receipts. One adapter
step creates one collection or one index with majority acknowledgment, then
reads back the contract. Existing validator, index-order, name, collation or TTL
drift is refused before any additive change. Unknown migration history must
remain a refusal for older migrators.

Startup verifies instead of creating sidecars. All may be absent before any
version-2 Agent exists; a partial or incompatible inventory fails startup.
Original events and all sidecars must be backed up and restored coherently.
No TTL, deletion, compaction, archival or sequence reset is authorized by this
reader migration. Writer control/admission collections are part of the same
explicitly registered additive inventory; their creation enables no policy.

## Rollout gates and verification

Deploy and exercise compatible readers with writer/recovery admission closed.
Verify unchanged legacy reads and writes and absence of new-format emission.
Before activation, certify the actual permitted rollback binary and drain
incompatible API and background writers. A decoder tripwire does not fence an
old writer that already captured a legacy record. After any adoption, rollback
must preserve segmented readability, publication and recovery capabilities.

The focused tests cover codec goldens, malformed envelopes and manifests,
original-byte corruption, quarantine without read-side writes, exact frozen
ranges, predecessor/point proofs, empty tails, coherent concurrent snapshots,
history beyond one physical segment, bounded source consumption as sealed
history grows, and migration restart/drift. Actual test results belong in the
exact-head PR evidence; this document is not a passing-gate claim.

Passing fenced audit/rebuild and near-capacity resource tests, retention regressions,
executable predecessor/rollback qualification and the full integrated foreground
caller inventory remain required before #1348 closes. Release-owner production
verification for this reader phase checks retained legacy history, cursors/SSE,
identity, and unchanged format/head distribution through authorized read-only
surfaces, with segmented emission closed. Real boundary replay and natural
rollover belong to the later writer activation gate; elapsed time alone cannot
satisfy that separate evidence requirement.

`.github/scripts/agent-log-predecessor-tests.py` archives the original pinned
pre-segmentation store and injects a test module only. It exercises unchanged
legacy reads/writes, then verifies that the segmented envelope tripwire refuses
reads and appends without changing the original source BSON. Run it under an
existing shared heavy lock with two Cargo jobs and the worktree's single explicit
Cargo target. It reuses the canonical disposable Mongo runner's image, cache,
resource bounds and cleanup; its temporary source archive is removed afterward.
This predecessor check does not certify the release's permitted rollback image
or prove that an already-running old writer has been drained.

## Output ordering and durable controls

The additive inventory includes `agent_log_gates` and `agent_log_admissions`.
Migration creates their validators and the exact admission-key index without
creating a gate record, selecting a cohort or activating a writer. The gate
record includes its bounded preparation slots; admissions retain their original
request identity and required nullable recovery proof. Runtime parsers still
validate authority and semantic relationships before effects. Global gate
policy alone does not prevent a legacy hierarchy purge; per-Agent admissions
do preserve its evidence and lineage.

Output changes use the partial unique `agent_log_output_change_seek` index on
Agent, incarnation, evidence generation, kind, numeric turn and numeric ordinal.
Only `output_change` evidence carries the numeric seek fields. Its validator
requires BSON long values within the safe sequence range and equality to the
typed nested record. Other evidence kinds omit these fields. Migration inspection
compares the exact partial predicate as well as key order, uniqueness and name;
drift refuses before further DDL.

Audit collection uses the shared publication output fold. It rebuilds turn
counters, immutable output changes and indexed tool state into the same private
stage as the semantic evidence, then promotes both under one progress cursor.
A resumed physical unit requires the prior completed turn-output checkpoint;
missing output state refuses installation. The first-terminal tool reference
remains immutable when later output advances the current tool record. Source
reply row limits are independent of the smaller evidence write-transaction
limit; the historical source/query/time and transaction caps remain test gates.

The immutable migration checksum must include `schema.rs`,
`schema/validators.rs` and `schema/controls.rs`. The migration adapter and shared
operator registration remain an integration gate; startup still performs no DDL.

The pure `audit_command` parser accepts exactly one of
`--agent-log-audit-unit`, `--agent-log-audit-cancel`, or
`--agent-log-audit-install`, followed by one Agent identifier and one audit
identifier. It rejects missing/extra arguments, unknown actions, unbounded or
ambiguous identifiers and wildcard scope. The shared operator dispatcher must
verify its explicit maintenance authority and deadline before calling
`audit::run_unit` (cancel false/true) or `audit::install`. The parser itself
performs no I/O; its existence does not make the CLI mounted or authorized.
