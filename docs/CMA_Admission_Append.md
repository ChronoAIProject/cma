# Bounded Agent message admission

Issue #1591 is the ordinary-message slice of #1475, related to #1543 and #1421.
It does not implement segments (#1348/#1349/#1351), change #1321 capacity limits,
or establish the separate public-page and idle-read targets.

## Stored proof and trust boundary

`agent_log_integrity_v1` is an additive field on the Mongo Agent envelope, outside
the strict `record`. Its version-1 fields are head sequence, exact entry count,
sum of encoded BSON document bytes, SHA-256 of the raw BSON tail, bound Agent
revision, immutable history identity, and `semantic_checkpoint_v1`. The latter
persists active/latest turn, latest message identity/role/nonempty status, gap,
outcome, steering adjacency/timestamp, conversation generation, and bounded queue
identities. Queue content is represented by SHA-256 and nonempty status rather
than copying up to 64 prompts into the envelope again.

History identity uses the Agent id, original creation key/fingerprint/time,
workspace id and `creation_worktree_id()`. Supported worktree recovery preserves
this tuple. A valid initialization must agree with the first `Created` row and
the existing full domain fold. Persisted recovery records remain validated by
`read_agent`.

Metadata is trusted only under the existing trusted-writer/immutable-prefix
contract. A tail hash does not authenticate arbitrary summary fields or middle
payloads. A complete audit detects malformed rows, gaps, and counts; it cannot
prove that an otherwise valid historical payload was never altered before the
first trusted summary was established.

## Decision table

| State in the transaction snapshot | Action |
| --- | --- |
| Supported, matching revision/head/identity and semantic checkpoint | Point-read and hash the tail, probe for extra rows; use summary only for a prepared cohort member |
| Missing or unknown metadata version | Full validation under the original caller; successful append maintains a new summary |
| Older revision or lower head after an old writer | Verify retained identity/tail evidence, then full validation and transactional repair |
| Explicitly invalidated semantic checkpoint | Full legacy validation; a valid complete domain fold can re-establish it |
| Ahead head, malformed supported metadata, conflicting identity/hash, missing tail or extra rows | Refuse; collect full-scan evidence where possible; abort and CAS a durable quarantine marker against the immutable creation incarnation and monotonic observed head |
| Complete semantic audit disproves a matching previously valid checkpoint | Abort and persist majority-acknowledged fenced quarantine; preserve the old proof as evidence |
| Existing quarantine marker | Refuse ordinary appends; never clear it through fallback |

An audit that exceeds the retained physical entry/byte ceiling also quarantines
the history. Ordinary proposed-append capacity refusals do not quarantine it.

An existing zero-head Agent is probed by `agent_id` without a sequence predicate.
An orphan at sequence 1, 2, zero, negative, or a malformed sequence is not empty
history. Fresh admission also uses this indexed existence proof before inserting
the initial row.

The low-level legacy append API and lifecycle publication can expose intermediate
states that the full transcript fold cannot yet project. Such states explicitly
persist `valid: false` and never authorize a semantic shortcut. Their existing
structural append contract remains available. Answer-context admission still
requires a valid full or incremental domain projection and every original
status, queue, policy, resource and authority fence.

Every full checkpoint rebuild uses the same semantic-audit classifier. When a
supported checkpoint still matches the Agent revision, head, immutable identity
and aggregate semantics, a malformed historical timestamp or impossible retained
transition disproves that trusted state. The audit aborts its transaction and
persists quarantine through the existing majority-acknowledged incarnation/prefix
fence before returning the failure. This protects already-enabled processes and
restarted processes. A revision-only write cannot erase the finding. Recognized
unfinished legacy turns and pending steering boundaries remain untrusted and
eligible for full fallback; an invalid projection alone is not corruption.

## Transaction and writer maintenance

Rows, head/count/bytes/hash, reducer checkpoint, aggregate and command receipts
remain in the original snapshot-read/majority-write transaction. The shared
`prepare_append` helper validates final sequences, encodes each row once, and
uses the same raw BSON buffers for capacity, tail hash and insertion. There is no
separate asynchronous metadata write. The existing revision/head/lease/holder/
expiry and authorization CAS predicates still serialize competing writers.

| Writer or reader | Integrity policy |
| --- | --- |
| Fresh admission | Atomic initial row and summary; bounded orphan probe |
| Single-entry append | Trusted summary for a prepared Agent; counted fallback otherwise |
| Conversation publication and command dispatch | Same summary plus independent pre-append latest-turn proof |
| Linear-message preflight | Snapshot-bound latest-started-turn checkpoint; dispatch independently rechecks in the append transaction |
| Answer-context reservation | Same trusted checkpoint or existing complete fold; original prior-message check retained |
| Immediate ordinary-message command settlement | At most 16 suffix rows from its exact dispatch fence after prefix proof; all final command/evidence checks retained |
| Controls, trigger settlement, and longer recovery suffixes | Existing complete evidence fold |
| Worker publication | Full validation, then atomic summary maintenance; ordinary bounded CloseProcess retries retain their separate gate |
| Capacity settlement and zero-entry retries | First markers and capacity state changes retain complete validation. Gated repeated failures use a fresh trusted summary and exact same-snapshot marker, with a complete first and hourly audit as specified below |
| Lifecycle, title, trigger terminal projection, runtime reconciliation, reuse | Existing complete validation, then atomic summary maintenance |
| Work claims, `save_agent`, controls, successor-worktree recovery | Carry only an already matching old revision/head/history with unchanged conversation; otherwise leave stale for counted fallback |
| Envelope fences and receipts without revision/log changes | Preserve summary; existing real writes still serialize authority |
| Flag-off new writers | Maintain metadata; continue complete validation |
| Old writers | Preserve additive fields; head/revision lag forces a new writer to validate before repair |

For #1788, `CHRONO_AGENT_RETRY_SINGLE_MARKER_ENABLED=false` preserves ordinary
failure appends. When enabled, the worker may propose a zero-entry retry only if
the exact tail is `WorkStepFailed` for the same step and failure class. The
observed marker travels as transient publication evidence, never as a persisted
field or new event. The transaction rechecks that marker and reconstructs the
entire permitted mutation before its existing domain validation and CAS.
Capacity and ledger markers keep their prior selection priority and contracts.

Zero-entry retries (including capacity and ledger repeats) may use bounded
verification only with `CHRONO_AGENT_CLOSE_RETRY_BOUNDED_ENABLED` and the existing
Agent cohort. A full scan proves contiguous sequence/count, total encoded bytes,
tail hash and the retained semantic fold. A fresh `agent_log_integrity_v1` carries
those prefix facts; same-snapshot indexed reads recheck incarnation, revision,
head, semantic checkpoint, exact tail/hash, and absence of rows beyond the head.
This relies on immutable middle history: point reads cannot rediscover arbitrary
middle-row damage. Consequently the first repeated retry in each store process,
and each first retry after one hour since its last confirmed complete audit,
fully scans again. Audit age uses a monotonic process clock, is shared by store
clones, and is never persisted; restart and bounded-cache eviction force an audit.
An aborted or ambiguous publication does not postpone the next required audit.
Missing, stale or semantically untrusted proof falls back to full verification;
corruption and quarantine retain the existing refusal policy. Successful effects,
step advances and first markers retain their full proof requirements.

During one publication invocation, the Agent writer lock remains held. After a
known-aborted transaction, its scanned history may be reused only when the new
snapshot has the exact same Agent (including revision, head, lease and identity)
and byte-identical integrity and quarantine BSON. Every attempt still validates
current domain/parent authority and performs the original CAS writes. The parent
Sandbox fence stays after history validation; no earlier fence holds a Sandbox
write lock throughout a scan. The cache ends with the invocation, and unknown
commits never use it as a receipt.

Conversation, worker, lifecycle and recovery ambiguous commits keep their complete
receipt, aggregate, command-dispatch, related-record and pinned-snapshot checks.
Only the single-entry API uses an exact one-row receipt. Missing or undecidable
receipts do not authorize new keys, new sequences or replayed runtime input.

All fallback/audit traversals retain #1584 caller/outcome/document/byte/duration
accounting. Bounded reads are not recorded as complete scans. The shared
`command_admission` label still includes intentional control/recovery/lifecycle
work, so its aggregate production share is not the ordinary-message scan count.

## Read-only proof adapter

`store::agent_log_integrity::read::read_proven_agent(store, agent_id, session)` is
the adapter agreed with the dependent #1320 public-window consumer. It returns
`None` only for an absent Agent; otherwise it returns `(Agent, trusted)`, retaining
the validated aggregate for captured head, immutable creation lineage and counted
read-only fallback without repeating the envelope/proof query. `read_head` is a
test wrapper exposing the current `last_seq` and `trusted`.
It shares the writer's exact metadata parser and bounded proof, including the
activation cohort, zero-head probe, tail hash and immutable lineage checks.
Every query explicitly uses snapshot read concern and the caller's supplied
session. Missing, unsupported, stale or disabled evidence returns an untrusted
head; corruption and dependency failures remain errors.

The shared read-only proof performs no audit, transaction begin/abort/commit,
metadata repair or quarantine write. Writer/audit wrappers retain those effects
and their complete-scan telemetry. The public-window consumer owns its counted
legacy fallback and window validation. This adapter does not activate public
window acceleration by itself; that slice must wire it and pass its combined
snapshot, corruption and document-budget tests. Focused Mongo tests verify the
adapter's pinned snapshot across a concurrent append and inspect the profiler
for forbidden writes and transaction commands in success and refusal cases.

## Preparation, activation and rollback

`CHRONO_AGENT_LOG_ADMISSION_AGENTS` defaults to empty. It selects at most 16 exact
Agent ids as a comma-separated list, without spaces or duplicates. IDs must
start with `agt_` and contain only ASCII letters, digits and underscores (5–128
characters). Before serving, each selected Agent is audited serially with a
10-second per-Agent deadline and a 60-second total deadline. Preparation writes
only metadata under the captured revision and both head fields. Failure or an
unconfirmed commit prevents the process from activating the cohort. Retrying
startup is safe: metadata preparation has no runtime or command side effect.
No unbounded discovery job runs and ordinary requests do not prepare a cohort.

Deploy first with the setting empty. Verify compatibility, the isolated Mongo
contracts and real-message profiler results. Then the release owner can select
the reviewed cohort and roll out. Restart repeats preparation; an old-writer
overlap can cause counted fallback and must not be advertised as the steady
prepared state. Disable by emptying the setting and rolling out. Data and receipts
remain intact. Quarantine writes require majority acknowledgement and fence the immutable
creation incarnation plus the observed prefix. An intervening revision-only
write cannot suppress a finding; persistence failures propagate. An explicit
repair must revalidate after the marker before clearing it. A quarantined Agent
requires explicit validated recovery; this
change provides no automatic clearing or payload repair operation.

Generation 173 (`4f03c35a8`) reads only known envelope fields and deserializes
`record`, while replacement writes use `$set`. Source inspection therefore shows
that the two additive top-level metadata/quarantine fields are ignored by its
readers and preserved by its writers. Schema version and log/command records do
not change. Old-shaped writer tests exercise stale detection; they are not an
executed generation-173 binary qualification. A rollback uses full validation;
forward upgrade revalidates stale metadata before using it. Generation 173 does
not enforce the new quarantine marker: a quarantined Agent requires operational
isolation and validated recovery before use through a rollback binary.

## Acceptance evidence

Target: a prepared ordinary message does not scan retained history, and its
complete admission/dispatch/immediate-settlement path examines at most 100 Mongo
documents. Compare 1,000 and near-limit histories under the same request. The
9,999-to-10,000 single-entry case succeeds; another normal append refuses without
increasing the limit. Full settlement/audit/recovery paths remain explicit
O(history) operations.

The Mongo tests profile all collections during answer-context reservation,
same-key response-loss replay, real command-worker execution, publication and
settlement. The disabled-cohort comparison uses this candidate's complete
fallback, including one preflight scan, rather than an executed old binary.
A separate failpoint test forces five ambiguous single-append commit replies,
proves one insert and a single pinned reconciliation snapshot, and verifies the
next append sequence with acceleration both enabled and disabled. Separate cases cover concurrent append, transaction abort, raw byte
accounting, old revision-only writes, stale repair, restart, tail corruption,
middle-gap and semantic-only audit quarantine, legacy-zero orphans and normal
capacity refusal. Negative single-entry receipt tests reject an absent Agent,
absent row, mismatched full entry and uncommitted/aborted transaction with the
cohort on and off. Existing multi-record and zero-entry receipt tests remain.
Partition tests serialize/reload reducer state at every boundary and compare
the final answer and latest turn with the existing complete fold.

Deployment verification must additionally report request p50/p95/p99, checkout
wait, CPU, circuit openings and reporting 429s against a matched workload. A
lower scan count alone is not proof of CPU improvement or the complete #1475
scale contract. Final production verification deadline is Friday 2026-11-13
15:00 SGT; the preferred merge window is Wednesday 2026-11-11 14:00 SGT, with
Thursday 2026-11-12 06:00 SGT as the fallback.

## Fleet selection extension

The explicit 16-ID startup mode above is retained for compatibility. The separate,
default-off `CHRONO_AGENT_LOG_FLEET_PERCENT` enables stable percentage stages with
lazy bounded preparation instead of startup enumeration. Read the
[fleet activation contract](CMA_Fleet_Log_Activation.md) for concurrency, failure
isolation, restart, telemetry and same-image disablement. Neither mode bypasses
the existing proof or quarantine contract. The public-window reader shares
selection and remains write-free; #1349's independent consumer switches stay off.
