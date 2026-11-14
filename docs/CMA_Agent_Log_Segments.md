# Immutable Agent log segments and rollover

This is the canonical design required by [#1348](https://github.com/ChronoAIProject/cma/issues/1348).
The original November 5 baseline is retained below. Its source revisions,
planning statements and proposed rollout are historical; the implementation
contract and executable checks are described here and in
[CMA_Agent_Log_Segment_Readers.md](CMA_Agent_Log_Segment_Readers.md).

## Implemented reader contract

- Agent envelope v1 remains readable. Envelope v2 requires supported format,
  incarnation and head revision; malformed metadata never selects legacy reads.
- Original event BSON and event IDs remain unchanged. Immutable manifests bind
  raw BSON leaves with `sha256_raw_bson_v1`, neighboring seal links and cumulative
  counts/bytes. Manifest decoding enforces an actual encoded size below 1 MiB.
- Range and point reads use one snapshot for aggregate, head, manifests and
  source. Each page rejects source outside global sequences `1..=last_seq`
  before delivering rows, including an empty page. Sealed reads verify touched
  leaves and neighboring links; they do not scrub untouched historical bytes.
  Active-tail reads validate at most one physical segment. Corruption is an
  error, never an empty or truncated successful page.
- The streaming visitor retains one decoded source event at a time, with driver
  batches limited to 64 rows and Mongo's reply byte ceiling. Public adapters
  preserve their 200-source-row and 4-MiB serialized-page budgets, predecessor
  correlation, frozen-through cursors, redaction and authorization revalidation.
- An audit invocation validates at most one physical segment. Completed sealed
  prefixes survive restart; the unprocessed tail is recaptured after append or
  rollover. Progress records expose the checked prefix and captured high-water,
  so an audit receipt is not a claim about later writes or later storage damage.
  Staging/promotion writes use fenced, bounded transactions; only a complete
  audit with an unchanged aggregate/head can install its evidence generation.
- Sidecar DDL is explicit and additive. Runtime startup verifies the registered
  inventory. Retention guards preserve segmented history and ownership lineage;
  this design grants no TTL, archival or physical source deletion.
- Writer and recovery activation remain separate, default-off controls. Reader
  implementation and local tests do not authorize production activation.

## Implementation and verification map

Paths below are relative to `api/crates/cma/src/store/` unless stated otherwise.

| Requirement | Implementation | Executable coverage |
| --- | --- | --- |
| Compatible envelopes and replacements | `agents.rs`, `agent_log_segments/envelope.rs` | `agent_log_segments/codec_tests.rs`, `agent_log_segments/mongo_tests/ranges.rs` |
| Immutable identity, hashes, integer and BSON limits | `agent_log_segments/{codec,digest}.rs` | `agent_log_segments/codec_tests.rs` |
| Snapshot ranges, point reads, source bounds | `agent_log_segments/reads.rs`, `agent_log_reads/segmented.rs`, `agent_log_store.rs` | `agent_log_segments/mongo_tests/{ranges,range_bounds,snapshot,bounds}.rs` |
| Streaming public pages, cursors and redaction | `layer2-core/src/sandbox/agent_reads/{window,stream}.rs`, `layer1-public/src/agent_v2/` (crate-relative paths) | Core Agent read and public v2 cursor/stream tests |
| Resumable audit, stale leases, uncertain commit | `agent_log_segments/audit/` | `agent_log_segments/mongo_tests/audit_{tests,movement,unknown,validation,bounds}.rs` |
| Schema restart/drift, grants and backup inventory | `agent_log_segments/schema/`, `migrations/`, `isolation.rs`, `segment_operations/authority.rs` | `agent_log_segments/mongo_tests/schema*.rs`, migration and operator authority tests |
| Retention and channel history | `agent_log_segments/retention.rs` and retirement callers | `agent_log_segments/mongo_tests/retention.rs` |
| Actual predecessor refusal | `.github/scripts/agent-log-predecessor-tests.py` (repository-relative) | Archived original predecessor store against disposable Mongo |

Mongo reader contracts are included by the `store::agent_log_segments::mongo_tests::`
selection in `.github/scripts/mongo-contract-tests.sh`. The scaling fixture covers
1/10/100/1,000 closed segments and 20/100/200 source rows, recording consumed BSON,
query counts and maximum reply/batch sizes. Reply sizes bound driver buffering;
they are not whole-process RSS measurements. The audit checks retain separate
near-capacity row/byte and write-transaction limits.

The release owner separately certifies the permitted rollback artifact, deploys
compatible readers, and verifies retained legacy reads/cursors/SSE and unchanged
format/head distribution. The predecessor harness is an executable tripwire
check, not certification of that release artifact. Production observations and
activation remain release deliverables; no local test result substitutes for them.

## Consumer-to-evidence inventory (#1349)

The table records format-2 dispatch, including legacy compatibility callers.
Store paths are relative to `api/crates/cma/src/store/`; core paths are relative
to `api/crates/layer2-core/src/sandbox/`. A captured source validates its active
physical segment once, bounded by 10,128 events / 65 MiB. Exact points verify
original BSON against their seals. Missing or inconsistent segmented evidence
refuses the operation; it never selects a legacy lifetime fold or repairs data.

| Consumer / retained scan caller | Segmented evidence and work bound | Compatibility / verification |
| --- | --- | --- |
| Personal/shared transcript (`agent_reads/{authorized,transcript_pages}`) | Certified turn index, at most 32 requested source events plus four header points; adaptive reads and serialized admission keep each page within 4 MiB. One long turn can span any number of pages. | Complete shape remains available within 200 source events / 4 MiB; otherwise `transcript_pagination_required`. `agent_log_segment_reads/transcript/tests/{parity,byte_pages,refusal}.rs` and core bounded-consumer tests. |
| v1 event pages (`agent_reads/log_pages`, public `rest/agent_reads`) | One captured source, at most 200 events / 4 MiB. Read chunks contain at most 32 points. A short byte-limited page retains a continuation at its last admitted original. | Legacy exact fold and existing cursor semantics retained; byte-page and core cursor tests. |
| v2 history and SSE (`agent_reads/{window,stream}`) | Snapshot range/visitor with frozen high-water, predecessor proof, redaction and final authority checks. | Existing cursor protocol and shared-history authorization tests. |
| Raw complete vectors (`agent_log_prefix`, `log_snapshot`, old `event_stream_snapshot`) | `agent_log_reads/segmented` refuses above 10,128 events or 4 MiB before returning a complete result. It never represents a partial vector as complete. | Compatibility-only API; callers needing larger histories use paged history/transcript ports. Legacy data retains its original bounded lifetime format. |
| Final answer and feedback (`agent_reads/answer`) | Exact message evidence → exact turn evidence → original message/start/terminal points. Terminal outcome, lost frames, steering and latest nonblank answer determine eligibility without a turn-length scan. | `agent_log_segment_reads/transcript/tests/answers.rs` covers 1/10/100/1,000 segments and corrupt/non-final refusals; core tests enforce owner/capability revalidation. |
| Usage (`agent_reads/authorized`, `agent_log_segment_reads`) | Certified incremental usage peaks/totals at the captured source. | Existing usage-generation/parity and stale-proof tests; legacy fold remains format-1 fallback. |
| Title read/claim (`agent_titles`, `agent_title_claims`) | Certified first eligible task turn and at most four originals in the same snapshot or transaction. Claim reconciliation repeats this bounded proof. | Existing truncation/setup/steering semantics; transcript/title parity and title claim scaling tests. |
| Title completion/rename (`agent_title_writes/{scoped,reconcile}`) | Common scoped publication updates Agent, head, projection, original event and exact operation receipt atomically. Unknown commit uses the immutable `Recorded` receipt. | Original claim/user token binds publication. No v2-to-v1 replacement; stale automatic work loses to owner title. Legacy writer remains format-1 only. |
| Suggestions (`suggestions`) | Fixed context policy with format-aware bounded range; enqueue validates the contiguous already-published batch in the same transaction. | Empty tails and unpublished append batches refuse; no transaction exception to source validation. |
| Command/approval admission (`agent_commands/{reserve,targeted_control}`) | `agent_log_segments/ordinary` reads exact answer/approval evidence plus bounded original points. Outstanding sets use indexed evidence. | Legacy semantic scans run only after positive legacy dispatch. Existing admission and approval contract tests. |
| Command reconciliation (`agent_commands/reconcile`) | Exact indexed command/dispatch evidence; original identities remain authoritative. | Missing proof cannot imply safe redispatch. Existing command publication/reconciliation contracts. |
| Conversation/work/lifecycle publication (`agent_conversation_store`, `agent_worker_transaction`, `lifecycle_transaction`) | Scoped capture, common projection update and exact publication receipt; later unrelated appends do not erase prior success. Empty-event metadata publications retain receipts. | `agent_worker_reconcile` and conversation reconciliation select exact recorded evidence before legacy scans. Existing common-writer Mongo contracts. |
| Creation replay (`agent_admission_records`) | Original creation event at sequence 1 in the same snapshot, checked against the retained binding. | `transcript/tests/admission.rs` covers scale and missing-original refusal. New creation remains legacy; its unknown-created-commit check validates the original one-event prefix. |
| Checkpoint/export (`checkpoint_capture`, `checkpoints/agents`) | Captured Agent/projection and global prefix identity through scoped capture. | Native/runtime/worktree identity and acknowledgment semantics remain exact; rebuild stays outside interactive lifecycle transactions. |
| Reconciliation/maintenance (`sandbox_reconciliation/agents`, `sandbox_maintenance/evidence`, `sandbox_unpublished_recovery`) | Bounded Agent/workspace inventories and scoped certified current-state/native-conversation facts. | Legacy scans are format-1 branches. Existing recovery, maintenance and checkpoint contract tests. |
| Retained reopen (`retained_reopen`, `sandbox_unpublished_recovery/transaction/reopen`) | Same-transaction creation point and digest of certified head, previous seal, active leaves and projection. Normal startup allowance remains 64 events / 512 KiB; no rollover or cleanup-reserve admission. | Original legacy history digest retained. Exact accepted-receipt replay precedes new capture; corruption refuses without mutation. `transcript/tests/reopen.rs` covers 1/10/100/1,000 segments. |
| First-message attachment on reuse (`agent_admission_reuse`) | Current legacy transaction root explicitly refuses segmented input before `scan_complete_log`; a common-writer adapter is still required. | Writer ownership/scope tracked with #1350; this inventory does not claim the mutation is implemented. New-format activation must remain closed until supported. |
| Terminal Trigger first-message publication (`trigger_first_messages/projection`) | Current legacy transaction root explicitly refuses segmented input before scanning. Both worker completion and refusal callers require a common-writer adapter. | Writer ownership/scope tracked with #1350; same activation prerequisite as reuse attachment. |
| Response projections/retention (`segment_response`, response readers) | Separate exact response/command evidence and retention ownership. | #1674 is developer-07's work; this consumer change does not certify or alter that implementation. |
| Bootstrap/audit/rebuild (`agent_log_segments/{writer_bootstrap,audit}`) | Fenced bounded source capture, resumable physical-segment traversal and exact promotion CAS. | Administrative/background procedures; no synchronous lifetime rebuild on foreground refusal. |

`scan_complete_log*` rejects a format-2 envelope before opening its legacy
source scan. In-memory stores retain finite legacy fixtures and do not represent
the Mongo segmented format. The inventory was checked with searches for
`scan_complete_log`, `agent_log_prefix`, and `complete_log` across the store and
core consumer modules, including the retained vector APIs above.

The source-bound proof has a separately tracked pre-existing restored-data gap:
[#1913](https://github.com/ChronoAIProject/cma/issues/1913) covers missing/null/string
top-level sequence keys omitted by Mongo numeric comparison type bracketing.
The numeric range checks in #1900 do not certify rejection of those shapes.

Consumer migration identity: `2026-11-05-am-log-segment-consumers-v1`, compatible
consumer phase. This change adds no DDL, TTL, source deletion, active-Agent
conversion or activation switch. Schema/index prerequisites and projection
formats remain those of the common segmented reader/writer modules. New-format
emission remains default off. Source/image, retained-client/recovery artifact,
gate revision, coherent restore and production verification receipts belong to
the release record; local consumer tests do not supply those receipts.

## Historical design baseline — November 5, 2026

Design spike · 5 November 2026 · owner/author: `cma-log-segments` / `chronoai-shining`.

The owner selected option B from PR #1342: keep the same Agent, retain every event, and roll a full active log into an immutable segment. This design removes the **lifetime** 10,000-entry / 64-MiB admission limit while keeping bounded validation and append transactions. “Indefinitely” means no configured lifetime entry, byte, or segment-count cap; it does not promise infinite physical storage, unlimited response bodies, or availability during a storage outage.

This is a design and issue-planning artifact. No application source, PR, deployment, database, runtime, or Agent message is changed by this spike. GitHub issue/milestone creation is explicitly authorized. The older statements in #1321/#1342 that rollover was not authorized describe those changes' scope; the current owner instruction authorizes this design and follow-up issues, not production activation.

## Evidence and scope

| Source | Inspected identity and finding |
| --- | --- |
| `origin/develop` | `ecfb557b6305619b49b2dd1b9d62c8792c455b90`; partial clone, source inspection only. |
| [Issue #1321](https://github.com/ChronoAIProject/cma/issues/1321) | [Investigation](https://github.com/ChronoAIProject/cma/issues/1321#issuecomment-6635920585): two Agents in two Sandboxes had exactly 10,000 entries at the 2026-11-04 16:15 UTC cutoff. Cleanup: 1,271 `CloseProcess` failures; reconnect: five `PublishConnection` failures. These are historical observations, not a live census in this spike. |
| [PR #1342](https://github.com/ChronoAIProject/cma/pull/1342) | Open when inspected; head `e1360a0a83890a9e3e2d94f6d1eee9d3f03db61c`. Read its owner-decision section and [capacity design](https://github.com/ChronoAIProject/cma/blob/e1360a0a83890a9e3e2d94f6d1eee9d3f03db61c/docs/CMA_Agent_Log_Capacity.md). It settles capacity failures and preserves finite cleanup reserves; it neither adds capacity nor reopens Failed Agents. |
| [PR #1316](https://github.com/ChronoAIProject/cma/pull/1316), [#1313](https://github.com/ChronoAIProject/cma/issues/1313) | #1316 is merged; head `be593eb462e8264296a3bd29f5c49c432ac6e781`. `agent_log_range` removes per-event complete scans but still scans the whole retained log once per page. Preserve this range interface and its frozen high-water semantics. |
| [PR #1338](https://github.com/ChronoAIProject/cma/pull/1338) | Open when inspected; [migration standard](https://github.com/ChronoAIProject/cma/blob/debffe119e94ab4b7e7d8c801cce99650a52963d/docs/CMA_Data_Migrations.md) read at `debffe119e94ab4b7e7d8c801cce99650a52963d`. Readers first, gated writers, explicit format/floor, compatible recovery, bounded restartable migration and recorded evidence apply. |
| Local migration assessment | `/Users/chronoai/code/work/tech-lead/notes/2026-11-04-cma-migrations.md`, read only. The generic runner/ledger and compatibility matrix are follow-ups, not capabilities assumed to exist. |

Stable source links below use the inspected develop commit and survive deletion of the clone:

- [Log envelope and range implementation](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/cma/src/store/agent_log.rs), [complete validation and capacity constants](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/cma/src/store/agent_log_integrity.rs), [snapshot traversal](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/cma/src/store/agent_log_reads.rs).
- [Store interface](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/layer2-core/src/sandbox/store/agent_log.rs), [public read service](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/layer2-core/src/sandbox/agent_reads.rs), [range projection](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/layer2-core/src/sandbox/agent_reads/window.rs), [cursor codec](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/layer1-public/src/agent_v2/cursor.rs).
- [Conversation transaction](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/cma/src/store/agent_conversation_store.rs), [work publication](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/cma/src/store/agent_worker_transaction.rs), [unknown-outcome reconciliation](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/cma/src/store/agent_worker_reconcile.rs), [command evidence](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/cma/src/store/agent_commands/reconcile.rs).
- [Strict Agent decoding and replacement fields](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/cma/src/store/agents.rs), [domain Agent](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/layer2-core/src/sandbox/agent.rs), [AM collection catalog](https://github.com/ChronoAIProject/cma/blob/ecfb557b6305619b49b2dd1b9d62c8792c455b90/api/crates/cma/src/store/isolation.rs).

## Decisions and invariants

1. Keep the existing `agent_log` collection, event document IDs, event bodies, Agent ID and one global sequence. Segments are immutable logical ranges over these rows; do not create a collection per Agent/segment or move old rows during initial rollout.
2. Introduce a bounded active head and insert-only closed-segment manifests in AM-owned sidecars. A closed segment's events, bounds, digests and predecessor identity never change. The active segment is append-only; its counters/head are mutable until sealed.
3. Publish rollover, the entire triggering append batch, affected aggregates, command/effect receipts and projection updates in the **same existing Mongo transaction**. Rollover alone never consumes a conversation event number and does not require a new public event variant.
4. Normal writes and readers validate the bounded active segment plus certified immutable ranges they actually touch. A restartable audit validates the entire captured logical history in bounded pieces. No claim is made that a constant-cost request freshly revalidates arbitrary historical bytes elsewhere in an unlimited log.
5. Keep strict ownership, channel retirement/history seals, runtime identity, revision/lease fencing and exact command identities. Log segmentation is not native conversation rotation, context compaction, a new Agent incarnation, or permission to reopen a stopped/deleted Agent.
6. Closed segments remain online in Mongo with no TTL in the first release. Archival is an explicitly deferred storage-tier feature with the same logical read and recovery contract.
7. All whole-history consumers must be made bounded before enabling rollover. A segmented writer behind unbounded transcript or recovery code is not a complete feature.

## Durable format, identity and indexing

The following are proposed versioned internal formats, not claims about existing types. Use the repository's Mongo/serde/CAS helpers and existing SHA-256 dependency. No new distributed log service or generic migration framework is necessary.

| Record | Required fields and authority |
| --- | --- |
| `agents` format discriminator | A dedicated **Agent envelope v2** for activated Agents, decoded by v1/v2 readers; retain the existing nested Agent/event wire shapes. A persisted `agent_log_format = segmented_v1` and head revision/identity bind the aggregate to its sidecar. All replacement helpers preserve and compare these fields. Do not increment the global schema constant for unrelated collections. An old `require_version` reader must reject this envelope; this is a tripwire, not a substitute for draining old writers. |
| `agent_log_heads`, one per activated Agent | `format_version`, `agent_id`, stable log incarnation from Agent ID + creation identity, `revision`, `active_ordinal`, `active_start_seq`, `last_seq`, active count/BSON bytes, previous seal ID/digest, lifetime count/bytes, projection format/through-sequence, migration ID and required reader capability. Bounded scalar fields, no growing manifest list. Aggregate global `last_seq` and nested conversation `last_seq` must agree. |
| `agent_log_segments`, one per closed segment | Identity `(agent_id, incarnation, ordinal)`; `start_seq`, `end_seq`, `entry_count`, `encoded_bytes`, predecessor ID/digest, cumulative prefix count/bytes, ordered fixed-width per-entry SHA-256 digests, own seal digest/version, sealing operation identity, source format and `storage_kind = mongo`. Insert once, no TTL. No empty closed segment. |
| Existing `agent_log` rows | `_id = agent_id:global_seq` and unique `(agent_id, seq)` remain unchanged, including legacy BSON bytes. Range membership is derived from the manifests/head; no rewrite to add `segment_id` to historical rows. Future rows retain the existing event envelope unless another separately reviewed event-format migration requires otherwise. |
| Publication and recovery receipts | Extend existing durable operation receipts where possible; add an AM sidecar for append paths lacking a durable receipt. Key by Agent/incarnation + stable publication key; retain request fingerprint, expected revision/head, assigned global interval, segment/seal identity, result digest and affected command/operation references. No user content in diagnostics. Exact duplicate succeeds with the recorded result; key reuse with a different fingerprint conflicts. |
| Derived projection/evidence records | Explicitly versioned records keyed by Agent and stable turn/command/approval IDs, with covered sequence and source seal/head identity. Bounded current-state record plus indexed per-entity rows, not one growing array. They are rebuildable from retained events; they never replace original receipts or history. |

Define the incarnation from the existing immutable Agent creation identity; do not introduce a new public cursor incarnation on activation. Ordinals begin at zero. An unactivated legacy Agent is an implicit segment zero, `[1, last_seq]`. At activation, validate its complete bounded history once and install the active head under CAS. If already normal-full (or beyond normal into reserves), seal segment zero and create active segment one beginning at `last_seq + 1`. A valid zero-entry initialization is active-only until creation is appended; segment zero must ultimately start with the sole Created event.

Use checked integers throughout. Persist sequences/ordinals/counts as nonnegative BSON signed 64-bit values, refusing overflow before any mutation. Public JSON numeric consumers must stay within their demonstrated exact-integer range; retain current encodings and add tests at the JavaScript safe-integer boundary. A future string-sequence protocol migration must precede that boundary; never wrap, round or reset a cursor. This astronomical representational ceiling is distinct from a 10,000-entry product lifetime cap. Lifetime byte counters also need checked overflow (do not cast unchecked `usize`); exceeding representation is a named operational error, not silent reset.

Add unique indexes `(agent_id, incarnation, ordinal)` and `(agent_id, incarnation, start_seq)` for manifests; ordered start-sequence lookup resolves the greatest start at/below a requested sequence, then verifies containment. Unique receipt keys and existing `(agent_id, seq)` enforce identity. Check overlaps/gaps transactionally because a unique start index alone cannot enforce interval non-overlap. Register all sidecars in AM schema/collection/grant/backup inventories, with strict format validators. DDL is an explicit additive migration with expected-index readback and partial-DDL restart checks; ordinary startup verifies it. Do not assume the deferred migration runner already exists.

### Seal digest and bounded read proof

During sealing, scan the active interval in order and hash **the original complete raw BSON bytes**, including envelope and record, once per row. Specify `digest_version = sha256_raw_bson_v1`: domain separator, length-prefixed UTF-8 Agent/incarnation, big-endian sequence and raw length, then raw bytes. Store the ordered 32-byte digests; the offset is `seq - start_seq`. Define a deterministic length-prefixed binary encoding for all immutable manifest fields (including predecessor digest and ordered leaf digests) and hash it with a separate seal domain. Publish golden byte/digest fixtures. Do not depend on unspecified JSON object order or deserialize/reserialize legacy BSON before hashing.

At at most 10,128 leaves, the binary digest vector is about 324 KiB before BSON overhead. Set a **1-MiB encoded seal-document limit** and test the actual maximum BSON representation; no manifest approaches Mongo's 16-MiB document limit. Seal/projection metadata is separately accounted, never subtracted from event bytes. Copy/restore/archive must preserve raw BSON or explicitly version a canonical representation with proven equivalent digests.

For a sealed-range page, read its immutable manifests, validate their own digests/bounds, fetch the requested raw events and compare each leaf digest plus decoded envelope identity/sequence. Verify the immediate predecessor/successor manifest links at touched boundaries, origin at sequence 1 when touched, and agreement with the active head at the tail. Queries are Agent/incarnation-scoped and require the captured committed head. Missing middle manifests, overlaps, duplicate sequences, wrong owner/Agent, wrong hash, missing event or extra uncommitted tail are integrity errors, never successful empty/truncated history.

The database's atomic publication and immutable-manifest discipline are the trust root. This detects accidental row/manifest corruption; it is not cryptographic protection from an administrator rewriting the entire database and its hashes. A range read does not walk every ancestor seal to the origin. Corruption outside the requested span is detected by the full scrub, not necessarily by that range read. This explicit change from fresh whole-history validation is required for bounded operation on indefinite history. A malformed or missing segmented head must never fall back to the legacy reader.

## Atomic append, rollover and unknown commits

Reuse the existing snapshot transaction, majority commit, retry classifier and reconciliation machinery. All append sites must call one segment-capacity/publication helper inside their existing transaction; the helper must not open a nested independent transaction.

1. Resolve the stable publication key before beginning work. Use the accepted command key, runtime checkpoint/update receipt, lifecycle operation + step + fence or existing mutation identity; bare append paths need a stable caller-supplied/internal operation key. Keep the same timestamp, fingerprint and key across retries. Do not deduplicate equal text: identical events with distinct legitimate keys remain distinct events.
2. In the transaction read the authoritative Agent, head, operation and work/runtime/owner fences. Require supported format, matching Agent/head global sequence and revision, same retained lineage, no channel history seal/retirement barrier, and the caller's still-owned unexpired lease where that operation uses a lease. Conversation writers retain their own runtime generation/transport checkpoint and Agent CAS; segmentation does not grant them a lifecycle lease they do not have.
3. Validate the domain mutation and receipt/effect evidence first. Validate the active interval exactly once: count, raw BSON bytes, consecutive global sequences beginning at `active_start_seq`, no extra tail, and aggregate/projection agreement. Validate referenced sealed evidence through bounded exact lookups.
4. Encode the **whole proposed batch** before choosing capacity. More than 64 events, any document above 16 MiB, arithmetic overflow, or a batch that cannot fit an empty segment in its eligible class is a deterministic rejection. Do not create empty segments or repeatedly roll over for an impossible batch.
5. If the batch fits the active class budget, append there. If an eligible normal batch does not fit, and rollover admission is open for this Agent, seal the current nonempty segment and put the entire batch in its successor. Do not split a logical atomic batch between segments. The successor starts at the old global head + 1; the batch gets exactly the sequences it would have had without rollover. A capacity-full old segment consumes **no additional event** to close.
6. In that same transaction insert the seal, advance head/aggregate using expected revision, sequence, active ordinal and all existing fences, insert every event, update bounded projections/command receipts, insert the immutable publication receipt, and commit. Concurrent appenders contend on the same Agent/head; only one wins. The loser rereads under its original operation identity and current authorization. No separate precommitted seal or unattached successor can become visible.
7. A known transaction abort may replan with current state under normal retry bounds. For `UnknownTransactionCommitResult`, retry commit on the same transaction/session as supported, then use the existing majority visibility barrier and a consistent snapshot to read the publication receipt, exact event interval, segment/seal and related receipts. A matching durable result is success **even if later writers have advanced the head**. Head equality alone is not sufficient for this check.
8. Absence without proof of abort is still unknown. Keep the original receipt/key, checkpoint acknowledgment and external-effect fences; return the established unavailable/unknown outcome and reconcile later. Never classify unknown as capacity, issue a new key, resubmit input, or allocate another successor. A conflicting receipt/content is corruption/conflict. A subsequent attempt that races a late commit still competes on the same unique key and CAS; it cannot commit a second publication.

For empty-log lifecycle metadata publications, retain their existing operation receipts and exact aggregate/worktree/effect checks. For a storage-only recovery rollover with no new events, require an explicit recovery receipt keyed by the old head/ordinal and recovery operation; the global sequence stays unchanged. Such a successor may temporarily be empty and must not be sealed again until it has events.

## Budgets, reserves and performance

| Dimension | Segmented policy |
| --- | --- |
| Normal events in one active/closed segment | At most 10,000 entries **and** 64 MiB of actual complete event BSON. |
| Stop/startup-failure ceiling | 10,064 entries and 64.5 MiB for that segment, using #1342's validated reserve eligibility. |
| Delete/physical ceiling | 10,128 entries and 65 MiB for that segment. |
| One event / atomic batch | Existing 16-MiB document / 64-event limits remain independent. Whole batch must fit its eligible empty-segment budget. |
| Seal metadata | At most 1 MiB per closed segment, independently measured. No growing manifest array in Agent/head. |
| Page / SSE window | Existing 1–200 source entries and 4-MiB projected-page budget; bounded raw-document decode and existing authorization deadline. No per-event complete scans. |
| Foreground active validation | At most one physical segment, 10,128 entries / 65 MiB, streamed; appended batch and fixed metadata are separately bounded. Historical sealed event bytes are not rescanned on append. |
| Whole-history audit / rebuild | One segment per work unit, bounded memory and time/cancellation; durable progress, captured high-water and seal identity. Total work remains O(total events/bytes). |
| Lifetime retained storage | No application lifetime count/byte/segment cap. Monitor aggregate disk, replication lag, manifest/receipt growth and backup size; storage provisioning remains necessary. |

Preserve #1342's fixed cleanup-failure markers and metadata-only repeat handling. Normal reconnects/retries never consume stop/delete reserves. Already-admitted stop/delete completion can consume its designated reserve even when rollover admission is disabled. A sealed segment can never be reopened to spend unused reserve; the active successor has its own reserves. Rollover failure caused by unavailable/corrupt storage does not justify fabrication of a seal, close receipt or cleanup success.

Do not admit endless cleanup-only segments to hide a retry loop. Finite cleanup uses the current reserve and the deduplication rules; exhausted physical reserves remain a named recovery/operator case. The narrowly scoped recovery step may seal a valid legacy segment already at the physical ceiling without needing a free event slot. A corrupt or physically over-limit legacy segment is quarantined for repair; this design does not silently split/relabel invalid history.

A read of closed history examines only manifests covering the page and their neighbors, plus requested raw rows. Across a consecutive 200-row page, leaf arrays in the first and last touched manifests are bounded by two physical segments; all fully interior segments together cover at most the requested rows. Thus manifest metadata fetched does not grow with total segment count. Verify this with query-count/bytes assertions, including small segments caused by large unsplit batches. Indexed lookup cost may grow logarithmically with the number of segments.

Initial active validation keeps the current bounded scan cost, so a cold active read/append can still scan 65 MiB. This is a deliberate first-release bound, not a claim of constant low latency or improved intra-segment CPU. Use bounded concurrency/backpressure and benchmark this maximum; if it misses the existing 15-second authority deadline, add independently certified immutable blocks or lower the segment target **before** writer activation. Do not remove authorization deadlines or use an unbounded cache as a fix. Any cache is size-limited and keyed by immutable seal digest or exact Agent/head revision; never cache authorization decisions or mutable tails by Agent ID alone.

## Range, page, SSE and authorization contracts

Keep `agent_log_range(agent_id, after, through, limit)` from #1316. In one coherent store snapshot resolve the captured high-water and relevant manifests/head. Return exactly `min(through - after, limit)` ordered entries, or a typed failure. The public projection may shorten the delivered page at its existing 4-MiB serialized limit; its next cursor advances only to the last delivered global sequence. A source error is never disguised as byte-budget truncation. Streaming validation must cap raw buffering: hold at most one bounded raw document plus the bounded projected page and metadata, not 200 potentially 16-MiB documents in an intermediate vector. Adapt the internal range visitor/stream interface if needed while preserving public semantics.

An example: segment zero ends at 10,000; segment one starts at 10,001. A frozen page `(after=9,998, through=10,004, limit=4)` returns 9,999–10,002 and continues after 10,002 with the **same through=10,004**, even if a concurrent append advances the live head to 10,100. A roll at 9,997 due to BSON size works identically; no sequence is implied by `ordinal * 10,000`.

Preserve v1 numeric `after`, v2 event cursor `ag1.<incarnation>.<sequence>`, snapshot cursor `ags1.<incarnation>.<through>.<after>`, and SSE `Last-Event-ID`. Internal ordinal is never a public cursor component. Rollover is not a cursor reset, synthetic public event, native generation change or reconnection requirement. An old retained cursor can replay through every boundary. SSE polls frozen bounded windows, then observes newer heads. Network reconnection can redeliver an acknowledged event; stable global IDs support client deduplication. The guarantee is no duplicated durable event and exact ordered replay, not exactly-once network delivery.

Authorize current Agent/Workspace/Worktree/Sandbox lineage and `ViewHistory` before resolving history; retain shared/visitor redaction and revalidate access after bounded reads and during stream polling. Manifests/proofs do not grant access and must not be directly public. Verify every row's Agent and incarnation, including cross-segment predecessor reads used to bind steering input. Preserve `reject_uncommitted_tail`'s concurrent-head distinction using a coherent snapshot or its current validated reread; a legitimate later commit is not corruption. Empty pages still validate head/format/authorization and never use missing metadata as proof of empty history. Archive or database unavailability is retriable history unavailability; a proven gap/hash failure is corruption; access revocation must stop emission without advancing past unseen events.

## Bounded folds, projections and recovery consumers

Changing only `agent_log_range` leaves several lifetime assumptions behind. The second implementation step must inventory every complete scan and classify what evidence it actually requires. Keep the legacy bounded path until an Agent activates; segmented paths must never call an unbounded `agent_log_prefix -> Vec` under an interactive deadline.

| Consumer / current locations | Required replacement before writer enablement |
| --- | --- |
| Conversation mutation (`agent_conversation_store`) | Active-segment validation plus bounded latest-turn/usage/runtime-generation state carried across the seal; exact checkpoint/command references retained. Preserve validation of runtime updates and pending approvals. |
| Work publication and unknown commits (`agent_worker_transaction`, `agent_worker_reconcile`, `lifecycle_transaction`) | Active-head invariants and exact original receipt/event range, including empty-event publications and later-head reconciliation; all parent/worktree/effect checks stay authoritative. |
| Commands (`agent_commands/{reserve,reconcile,targeted_control}`), trigger-first-message/admission/reuse, unpublished recovery, maintenance evidence | Per-command evidence projection with covered sequence/source identity and indexed exact original log references. Include dispatch/ack/deferred/terminal evidence; never infer absence from a partial segment. |
| Pending approvals | Indexed approval state keyed by original request identity, advanced transactionally with events. Page the outstanding set; do not copy every historic approval into one head document. |
| Usage and current semantic validation (`agent_reads`, `agent_reads_fold`, `agent_reads_usage`) | Versioned incremental fold state: creation lineage, latest native conversation/runtime generation, current turn identity, usage generation peaks/cumulative values and queue/steering references. Preserve fold semantics across a mid-turn seal. No transcript bodies in scalar state. |
| Transcript, final-answer/feedback checks, title generation, suggestions | Indexed per-turn/message facts and bounded text retrieval. Existing unpaginated transcript cannot represent unlimited history in bounded memory: add a paginated public transcript/history path, update all first-party/SDK consumers, and retain the old response for histories within a documented response budget. Above that budget return an explicit “use paginated history” error, never silently truncate or return a blank conversation. Final-answer authorization uses exact indexed turn/answer evidence, not complete transcript materialization. Title/suggestion inputs use their defined bounded context, while originals remain readable. |
| Checkpoint capture/restore (`checkpoint_capture`, `checkpoints/agents`, `conversation_checkpoint`), retained compatibility paths | Reference the captured global sequence and manifest/prefix identity; preserve exact update receipt and native continuation semantics. Large retained exports/rebuilds stream bounded chunks outside lifecycle transactions. Existing retired-checkpoint readers remain readable. |

Carry projection state across segment boundaries without pretending a segment boundary completes a turn. Events such as a steering fence followed by its owner message must remain paired by global sequence; keeping a batch together helps but is not a substitute for cross-boundary proof. A single turn may itself span many segments; do not accumulate its full transcript in one Mongo document or memory buffer. Store bounded turn metadata and indexed references/messages; paginate a large turn.

Bootstrap legacy projections from the same validated ≤65-MiB snapshot and commit them with activation under Agent/head CAS. If expensive preparation happens outside the transaction, pin the original revision/head, then reject/retry if they changed; never install stale prepared evidence. Derived state has explicit format, through-sequence and source identity. Missing, ahead-of-head, mismatched or unsupported state blocks dependent operations; rebuild from retained segments with a fenced resumable job. Do not return an empty projection or replay a command to repair it. The hot path must not trigger a synchronous lifetime rebuild.

Projection maintenance must be included in **every** event writer and unknown-commit receipt. Step two ships consumers and legacy fallback with new-format emission still closed; tests use segmented fixtures. Step three activates projection writers and segment publication together, so the first rollover cannot outrun its recovery evidence.

## Retention, deletion, archival and full-history audit

Closed events, seals and required operation receipts have no TTL. Retain creation/ownership lineage sufficient for authorized historical reads even when a runtime/worktree is retired. Runtime Agent/Sandbox deletion is distinct from erasing audit history. Audit all generic collection-deletion paths and channel-history seals so adding new sidecars cannot create orphaned history, drop a proof, bypass a retirement barrier or accidentally erase the retained log. Any future owner-authorized history purge requires a separate retention design and cannot be used as rollover capacity management.

The first implementation keeps every closed segment online in Mongo. This avoids coupling normal append/recovery to a new object-storage service; copying history elsewhere alone would not replenish the current accounting. Whole-database backup/restore must include Agent aggregates, heads, manifests, events, projection/evidence rows and receipts at a consistent recovery point, with original home/key/native identity dependencies retained. Rehearse restore using the exact compatible recovery binary and reconcile post-backup accepted writes.

A future archive phase must use versioned content-addressed objects, verified byte count/hash and durable readback before publishing a separate placement record via CAS. Do not mutate an immutable seal to change location. Keep Mongo source data until independent readers, key/access retention, backup/restore and outage tests pass; removal of a redundant hot copy is a separate reviewed operation, never deletion of logical history. All range, receipt, projection-rebuild and full-audit readers must route through the same placement resolver. Missing/unavailable archive returns an explicit retriable error, not a gap/empty page; corruption returns integrity failure. Resume/cleanup must have access to the exact referenced evidence. Do not reuse the retired checkpoint backend as though it is active log archival.

A full audit captures `(incarnation, through_seq, terminal seal/head revision)`, streams manifests in ordinal order, verifies predecessor seals and consecutive ranges from 1, recomputes every raw event digest/count/byte total, checks cumulative prefix totals and captured tail, and validates cross-segment semantic folds/evidence. Snapshot/copy the bounded active tail at capture or restart that tail if it changed; sealed prefixes need no lifetime Mongo snapshot. Persist per-Agent progress and expected seal digest under a fenced audit lease, with rate/concurrency limits and cancellation. Audit completion is a receipt naming the captured high-water, not “history is valid forever.” Corruption quarantines affected reads/writes for that Agent and alerts an operator; never skip a bad segment to continue. Record audit lag so lazy integrity detection is visible.

## Recovery of the two already-exhausted Agents

Deploying readers or raising capacity does not itself reopen a Failed Agent. The historical heads of 10,000 may have advanced into #1342's reserve by rollout; discover their **current** statuses, revisions, leases, close operations, command delivery and exact tails through authorized metadata. Do not hard-code public IDs or assume the incident's statuses are current.

Provide a separate default-closed, revisioned **capacity-recovery cohort** and an idempotent domain operation, not a direct Mongo status patch. Its dry-run classifies eligible Agents without writing. Use the original capacity failure/operation identity as the request identity, pin Agent revision/native/home identity and gate revision, and keep an immutable outcome receipt. Wait for existing valid leases to finish/expire; do not steal them. Admission must refuse stopped/deleted/retired lineage, owner-requested stop/delete, unsupported format, bad history, replaced home/native identity, or a superseding failure/intent.

| Current cohort state | Recovery behavior |
| --- | --- |
| Original reconnect Agent still Ready/Starting with legitimate pending work | Adopt/activate its valid legacy history and let the **same** fenced work operation retry publication through rollover. Reconcile runtime effect receipts before repeating an external open/connect effect. |
| Reconnect Agent settled `Failed / agent_log_capacity_exceeded` by #1342 | Admit a narrow validated `recover-capacity` lifecycle transition, seal/adopt the exact old prefix and provide a fresh active segment. Persist recovery work in the same transaction; retain original failure evidence. Inspect/adopt/reopen the same native conversation on its original home using recorded provider authority. A new lifecycle recovery operation may have its own key, but all prior input/dispatch keys remain original. Publish Ready only after runtime confirmation. |
| Cleanup Agent still has the already-requested stop/delete pending | Continue that cleanup with its original close/removal receipts and #1342 reserves; activation can supply capacity where valid, but does not change the owner's intent to continued work. |
| Cleanup Agent reached Stopped/Deleted, or owner issued a newer stop/delete | Do not auto-resume it. Retain history. A future owner continuation, where supported, must be an explicit lifecycle admission with existing authority; this feature does not add arbitrary resurrection of terminal/deleted Agents. |
| History corrupt, >physical legacy limit, native evidence absent/ambiguous beyond supported recovery | Leave a named blocked recovery outcome with intact evidence. Do not manufacture a conversation, skip old history or resend input. |

Thus both incident Agents receive a safe disposition, but **both are not promised to become running**: the cleanup Agent's recorded stop must be respected. This is the decision for the apparent conflict between “resume exhausted Agents” and continuity of owner intent.

Never reopen a failed response or resend a terminal/ambiguous command because storage is available. #1342 may have settled undispatched input as `agent_startup_failed` or a dispatched response as `runtime_lost`; those classifications do not prove the input was not executed. Preserve accepted receipt identity, original dispatch binding, known output and uncertainty. Recovery resumes the runtime/log recorder/checkpoint acknowledgment using original receipt reconciliation; it admits no synthetic user turn. New owner work follows the normal new-command API after recovery. Test recovery failure, interruption and duplicate execution at every durable boundary.

## Rollout, reader floor and rollback

Implement after #1316 and #1342 are merged. Follow #1338's migration standard, even if its documentation PR merges in a different batch. Use four independently deployable steps: compatible storage readers, bounded consumers, gated rollover writer, then exhausted-Agent recovery. Each implementation PR copies/maintains the relevant design under `docs/` and records its own migration evidence; this spike creates no code PR or merge.

1. **Expand/readers (gate closed).** Add explicit v1/v2 Agent-envelope readers, head/seal/projection format readers and strict rejection of unknown formats. Audit whole-record replacements and all collection owners. Add planned indexes/validators/grants; existing Agent/log rows stay unchanged. Ship range/SSE compatibility fixtures and integrity audit support. Reader deployment alone creates no segmented Agent.
2. **Bound consumers (gate closed).** Deploy the bounded folds, exact evidence lookups, paginated transcript path and client/SDK updates. Verify retained browser assets, pending journals and original cursors with new APIs. Remove every reachable lifetime scan/materialization from activated-Agent foreground paths. New-format emission remains closed.
3. **Certify the floor.** Record immutable API/worker/recovery image digests and sources covering steps one–three, #1311 tool-output readers, #1318 input semantics, #1316 and #1342; include other formats in the actual release batch. Test actual recovery binaries on both old and new fixtures, not just current serde types. Drain every incompatible API/worker/background writer and exclude its reintroduction through deployment controls. Envelope v2 is defense in depth; raw database writers bypassing codecs require explicit inventory and exclusion.
4. **Enable gated writer cohort.** Introduce `agent_log_segments_v1` admission, default false, selected by Agent/owner authority and revisioned cohort. This is separate from task/UI admission. It covers direct append, lifecycle/prepared startup, conversation/tool output/checkpoint, commands, title/suggestion, reconciliation, maintenance, creation/admission/reuse and every retained compatibility writer. Adopt only a bounded cohort; bootstrap from the still-bounded legacy log. Verify and expand deliberately.
5. **Gate closure semantics.** Closing admission stops new Agent activations and new unaccepted work requiring a fresh rollover. Activated Agents stay segmented; bounded appends that fit and exact accepted retries, uncertainty reconciliation and admitted cleanup must still work. Allow only receipt-bound, already-admitted publications to complete a necessary rollover after closure; record this finite recovery exception and test it. Closing a gate is not a zero-write rollback switch. It does not shrink already-written history or restore old-reader compatibility.
6. **Recover incident cohort separately.** Enable `agent_log_capacity_recovery_v1` only after writer canary evidence and an approved current-state dry-run. Default false. Persist operation receipts/cohort outcome and respect current stop/deletion intent. Readers and exact accepted recovery remain available if recovery admission closes.
7. **No contract in this milestone.** Keep v1 readers for unactivated/retained history, retain new sidecars and all evidence. A future archival/contraction phase is separately reviewed.

The point that raises the reader floor is the **first activated Agent envelope/sidecar write**, even before its history exceeds 10,000. The permitted rollback is a certified segment-aware recovery build; reader-only step one is insufficient if it cannot maintain step two's projections or finish admitted rollover/recovery work. Record the exact allowed image(s), not a label like “latest.” Before activation, ordinary rollback is permitted only within the already-required #1311/#1318/#1342 floor and the tested additive-DDL compatibility set. After activation, close new admissions, preserve logs/receipts/homes/runtime pins, and roll forward or deploy the compatible recovery artifact. Never delete manifests, reset `last_seq`, drop events, remove the format marker, or restore a stale backup over new accepted writes to make an old binary start.

Migration records under `docs/` must include immutable IDs/checksums, source/target formats, prerequisites, all writers, expected indexes/validators/no TTL, responsible service owner/operator, exact image/source and reader-floor receipts, gate/cohort revision and drain proof, bounded CAS/progress/error rules, backup/rehearsal and post-backup write plan, rollback procedure and completion evidence. Proposed IDs: `2026-11-05-am-log-segment-readers-v1`, `2026-11-05-am-log-segment-consumers-v1`, `2026-11-05-am-log-segment-writer-v1`, `2026-11-05-am-log-capacity-recovery-v1`. Do not fabricate execution checksums or digests in this design; implementation/release records fill them with actual reviewed artifacts.

## Test and production acceptance plan

The spike runs no application test suite or production probe because it changes no implementation. These are mandatory acceptance tests for the implementation issues, using existing real-Mongo contracts/failpoints and domain fakes, without weakening current CI gates.

| Area | Required evidence |
| --- | --- |
| Boundaries | 9,999→10,000 fits; next append rolls; BSON-byte rollover below 10,000; exact 64 MiB/64.5 MiB/65 MiB and +1; 64 vs 65 events; 16-MiB document +1; whole batch too large for empty segment; early seal of a small segment; no empty rollover loop; checked integer/JS cursor boundaries. |
| Atomicity/fencing | Two appenders/rollovers, stale head, changed Agent revision, expired/replaced lease, prepared startup fresh lease, gate revision change, channel history seal/retirement, owner revocation and concurrent stop/delete. At every failure, all-or-none event/seal/head/aggregate/projection/receipt state. |
| Unknown outcomes | Fail before commit, commit but drop acknowledgment, delayed majority visibility, process crash/restart, exact retry after a later append, collision with different fingerprint, no receipt yet, lease loss during reconciliation. One durable batch/successor; original input/checkpoint/effect identity retained. |
| History/integrity | Pages before/at/through boundary, bytes-triggered boundaries, old cursors, frozen through under concurrent append, empty/partial pages, first/last/middle corruption and missing manifest, overlap/gap/duplicate/extra tail, forged Agent/incarnation/query fields, wrong digest/counters, complete scrub restart/cancellation and captured-prefix receipts. |
| SSE and authority | `Last-Event-ID` at last old/first new event, disconnect/reconnect, redacted events retaining sequence, shared/visitor reads, revocation during page or stream, no cursor advance over missing/unavailable data, original command/response/steering binding across a boundary. |
| Consumers | Turn/tool output/approval/queued input spanning ≥3 segments; very long single turn; usage generation continuity; exact command settlement/checkpoint acknowledgment; paginated transcript/large-turn rendering, feedback/final answer, title/suggestion bounds; projection rebuild from original events and refusal of inconsistent state. |
| Reserves/recovery | Preserve #1342's >70 cleanup retries followed by success without marker growth; gate-closed cleanup; Failed-capacity recovery and already-requested cleanup dispositions; heads already in reserves; duplicate recovery, current stop/delete supersession, retained native/home identity and no redispatch of uncertain/terminal input. |
| Migration/recovery images | Old data/new reader, new data/every permitted recovery executable, predecessor rejection, mixed-version overlap with writer closed, all ordinary/background writer gates, whole-record field preservation, index drift/partial DDL, missing sidecars, migration restart, backup/restore with post-backup-write reconciliation. |
| Performance | 1/10/100/1,000 segments, page 20/100/200, newest and oldest pages, near-65-MiB active segment, maximum seal BSON, tiny segments/large batches, SSE idle/follow and append/rollover P50/P95/P99. Count rows/bytes/queries and peak memory; no O(lifetime) foreground scan or per-event range call. Demonstrate 100/200-entry pages fit the 15-second authority deadline at the documented load, with headroom stated in evidence. |

Release owner performs deployment/activation under separate production authorization. Verification after that uses read-only owner APIs and existing VictoriaLogs GET queries, with content-free aggregates; no synthetic Agent input, stop, runtime restart or direct database write is part of this verification plan.

- Record source/image, exact reader floor, cohort/gate revisions, schema/index receipts and baseline status/global heads/segment counts/lease metadata. Production diagnostics must expose only authorized aggregate counts and existing protected identifiers, never log payloads, credentials or raw digests of user text in public telemetry.
- Observe natural canary rollover and at least **36 minutes** (two historical 18-minute leases). If no natural rollover occurs, record “rollover not yet observed” and keep activation verification open; elapsed time alone is not success.
- Check one successor per closed predecessor; global head advances exactly by committed event count; no capacity-stranded reconnect cadence, duplicate input dispatch or lost checkpoint acknowledgment. Require exact receipt/effect evidence for cleanup completion and runtime recovery.
- Read metadata/history pages and SSE replay before/through/after a real boundary; compare global cursors, retained prefix/creation identity and redaction. Check page latency/error rates and manifest/hash/gap diagnostics, bounded active-validation bytes, rollover latency/conflicts, reconciliation backlog, projection lag, audit lag and storage/backup growth.
- For the two incident Agents, record current cohort classification and safe outcome: same-Agent reconnect recovery when eligible; completion/continuation of already-requested cleanup or preserved terminal state for the other. Retain aggregate evidence without posting user content/IDs.
- Stop cohort expansion for integrity/identity mismatch, unsupported reader exposure, lost authority, duplicate durable publication/input, increasing unknown-commit backlog, unexplained history gaps, or violated latency/memory bounds. Close new admissions and use the certified recovery build; retain all already-written segments and accepted recovery work. A generic old-image rollback is excluded.
- Close implementation issues only after their deploy/verification exit criteria are recorded; merged PRs and green CI alone are not production completion. Mark observation limits explicitly.

## Independently shippable implementation plan

Four issues carry the full repository-style sections: Background, Purpose, Implementation Specification, Affected Files, Verification Checklist, Definition of Done, plus migration/rollout and production-verification details. The checkout has no `.github/ISSUE_TEMPLATE`; the established six-section structure from #1321 and neighboring issues is used. All issues are assigned to `chronoai-shining`, labeled with existing backend/type/priority/size labels and placed in **Agent log segments**. Each depends on #1342 and #1316 and is related to #1321. The writer additionally depends on readers and bounded consumers; recovery depends on the writer. Reader and consumer changes may be developed independently once their shared format contract is fixed, but ship in dependency order.

Issue links and the GitHub copy of this complete design are recorded below. The full design is included in the reader issue so collaborators do not depend on this local path. That copy is the shared design baseline; implementation PRs bring it into repository documentation without this spike creating a PR.

Duplicate check: searched all open/closed issues for `segment`, `rollover`, and combined log/history terms. Only #1321 covered this exact continuation decision, as background/incident settlement; no independent segment-reader/writer/recovery implementation issue was found. Existing migration tooling issues #1327/#1328/#1329/#1331 are related infrastructure and are not duplicated or made falsely mandatory dependencies.


## Published handoff

Milestone: [Agent log segments](https://github.com/ChronoAIProject/cma/milestone/26).

| Step | Implementation issue | Dependency order |
| --- | --- | --- |
| 1 | [#1348 — Compatible readers, range replay and integrity audits](https://github.com/ChronoAIProject/cma/issues/1348) | #1342 and #1316 |
| 2 | [#1349 — Bound transcript, projection and recovery consumers](https://github.com/ChronoAIProject/cma/issues/1349) | #1348, #1342 and #1316 |
| 3 | [#1350 — Gated atomic rollover with exact commit recovery](https://github.com/ChronoAIProject/cma/issues/1350) | #1348, #1349, #1342 and #1316 |
| 4 | [#1351 — Recover capacity-exhausted Agents without replaying input](https://github.com/ChronoAIProject/cma/issues/1351) | #1350 (and its reader/consumer prerequisites), #1342 and #1316 |

All four are assigned to `chronoai-shining`, labeled `area:backend`, `type:feature`, `priority:P1` and a size label; #1349 additionally carries frontend/SDK labels. All link to #1321. The complete design baseline is included in #1348's issue body. No duplicate implementation issue was found in the pre-publication searches.

Artifact validation for this spike checks required issue sections, dependency/related links, milestone, labels, assignee, GitHub body readback and the complete design copy. Source inspection was pinned to the commit above and the clone was confirmed unmodified. No application build, executable test, production verification or deployment was performed; acceptance plans above are future work. The temporary `src/` clone and `research/` downloads/drafts are removed at final handoff; only this requested design note is retained locally.
