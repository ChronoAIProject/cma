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
| Worker publication | Full validation, then atomic summary maintenance |
| Capacity settlement and zero-entry cleanup retries | Mandatory complete validation and exact retained-marker proof, even with trusted metadata |
| Lifecycle, title, trigger terminal projection, runtime reconciliation, reuse | Existing complete validation, then atomic summary maintenance |
| Work claims, `save_agent`, controls, successor-worktree recovery | Carry only an already matching old revision/head/history with unchanged conversation; otherwise leave stale for counted fallback |
| Envelope fences and receipts without revision/log changes | Preserve summary; existing real writes still serialize authority |
| Flag-off new writers | Maintain metadata; continue complete validation |
| Old writers | Preserve additive fields; head/revision lag forces a new writer to validate before repair |

Conversation, worker, lifecycle and recovery ambiguous commits keep their complete
receipt, aggregate, command-dispatch, related-record and pinned-snapshot checks.
Only the single-entry API uses an exact one-row receipt. Missing or undecidable
receipts do not authorize new keys, new sequences or replayed runtime input.

All fallback/audit traversals retain #1584 caller/outcome/document/byte/duration
accounting. Bounded reads are not recorded as complete scans. The shared
`command_admission` label still includes intentional control/recovery/lifecycle
work, so its aggregate production share is not the ordinary-message scan count.

## Read-only proof adapter

`store::agent_log_integrity::read::read_head(store, agent_id, session)` is the
adapter for the dependent #1320 public-window consumer. It returns `None` only
for an absent Agent; otherwise it returns the current `last_seq` and `trusted`.
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
Agent ids. Before serving, each selected Agent is audited serially with a
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
forward upgrade revalidates stale metadata before using it.

## Acceptance evidence

Target: a prepared ordinary message does not scan retained history, and its
complete admission/dispatch/immediate-settlement path examines at most 100 Mongo
documents. Compare 1,000 and near-limit histories under the same request. The
9,999-to-10,000 single-entry case succeeds; another normal append refuses without
increasing the limit. Full settlement/audit/recovery paths remain explicit
O(history) operations.

The Mongo tests profile all collections during answer-context reservation,
same-key response-loss replay, real command-worker execution, publication and
settlement. Separate cases cover concurrent append, transaction abort, raw byte
accounting, old revision-only writes, stale repair, restart, tail corruption,
middle-gap audit quarantine, legacy-zero orphans and normal capacity refusal.
Partition tests serialize/reload reducer state at every boundary and compare
the final answer and latest turn with the existing complete fold.

Deployment verification must additionally report request p50/p95/p99, checkout
wait, CPU, circuit openings and reporting 429s against a matched workload. A
lower scan count alone is not proof of CPU improvement or the complete #1475
scale contract. Final production verification deadline is Friday 2026-11-13
15:00 SGT; the preferred merge window is Wednesday 2026-11-11 14:00 SGT, with
Thursday 2026-11-12 06:00 SGT as the fallback.
