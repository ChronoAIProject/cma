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

## Additive migration and retention

The reader migration adds the six sidecars: heads, segments, publications,
projections, evidence and audits. The shared migration runner owns source and
checksum identity, authority, lease renewal and completion receipts. One adapter
step creates one collection or one index with majority acknowledgment, then
reads back the contract. Existing validator, index-order, name, collation or TTL
drift is refused before any additive change. Unknown migration history must
remain a refusal for older migrators.

Startup verifies instead of creating sidecars. All may be absent before any
version-2 Agent exists; a partial or incompatible inventory fails startup.
Original events and all sidecars must be backed up and restored coherently.
No TTL, deletion, compaction, archival or sequence reset is authorized by this
reader migration. Later writer control/admission collections require their
separate registered additive migration.

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

Fenced audit/rebuild, near-capacity audit resource proof, retention regressions,
executable predecessor/rollback tests and the full integrated foreground caller
inventory remain required before #1348 closes. Release-owner production
verification must observe real boundary replay and natural rollover without
synthetic user input; elapsed time alone cannot satisfy that evidence.
