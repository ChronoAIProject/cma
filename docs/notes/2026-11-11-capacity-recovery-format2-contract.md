# Frozen capacity-recovery carrier contract

Part of #1351. Schema/discovery/index dependency only, based on develop. No
service, worker, migration execution, native activation or production write is
included. S1 owns validator/index installation and immutable migration checksum;
S2 owns shared registration, roles and backup inventory. S5 owns the codec,
discovery consumer and accepted-work service. This freezes the shape already
encoded by S5 `records.rs` / `records/result.rs` at `790ee7458`.

## Collection and version

Collection: `agent_capacity_recovery_work`. The exact Mongo validator definition
is `api/crates/cma/src/store/agent_capacity_recovery/format2.schema.json`.
Install with `validationLevel: "strict"`, `validationAction: "error"` through the
existing explicit additive migration. Collection/index collation is simple.
Startup and read paths perform no DDL. The JSON schema is a Mongo validator,
not an Extended JSON document to insert.

Only BSON **Int64 format_version=2** is accepted. No v1 carrier, automatic v1
upgrade, dual decoder, timestamp rewrite or synthesized result witness is allowed.
An existing unknown/v1 carrier makes migration inspection fail closed; it must
not be silently relabelled. RecoveryWork's separate internal format is Int32 1.

Every object rejects unknown properties. All fields are required except the two
explicitly omitted RuntimeBinding fields below. Null is distinct from omission.
The entire BSON row is at most 32 KiB; decoded work also has the existing 16 KiB
serialized JSON budget. No Agent snapshot, input, credential or output payload is
stored in this carrier. Integer BSON types are part of the codec contract:
serde `u8` becomes Int32; `u32`, `u64` and explicit `i64` become Int64 with bson 3.1.

| Path | Exact BSON shape |
| --- | --- |
| `_id` | string `capacity-recovery:` plus 64 lowercase SHA-256 hex characters |
| `format_version` | Int64 2 |
| `agent_id` | string, original Agent identifier |
| `work` | strict object, fields enumerated below |
| `result` | strict original result witness, fields enumerated below |
| `work.format_version` | Int32 1 |
| `work.request` | strict object: `agent_id`, `creation_key`, `creation_fingerprint`, `failure_seq`, `requested_at` |
| `work.target` | strict object: `agent_id`, `creation_key`, `creation_fingerprint`, `conversation_id`, `conversation_generation`, `runtime`, `home_identity`, `original_open`, `active_turn`, `input_sha256` |
| `work.phase` | string: `admitted`, `attaching`, `completed`, `superseded` |
| `work.revision` | Int64 >=1 |
| `work.attempts` | Int64 0..4294967295 |
| `work.next_fence` | Int64 >=1 |
| `work.lease` | null or strict object: `holder` string, `fence` Int64 >=1, `expires_at` timestamp string |
| `work.ready_at` | canonical timestamp string |
| `work.completed_at` | null or canonical timestamp string |
| `work.target.runtime` | strict RuntimeBinding object: `runtime_id`, `runtime_version`, `bound_at`; optional `boot_id` string and `provision_generation` Int64 >=1 are **omitted**, never null, when absent |
| `work.target.original_open` | strict object: `step` string `open_conversation`, `operation_id`, `request_sha256`, `completed_at` |
| `work.target.active_turn` | required null or Int64 1..failure_seq |
| `result.agent_digest` | BSON BinData, generic subtype 0, exactly 32 bytes |
| `result.logs_digest` | BSON BinData, generic subtype 0, exactly 32 bytes |
| `result.agent_revision` | Int64 >=1; revision of the original published result |
| `result.through_seq` | Int64 >=failure_seq, <=9007199254740991 |
| `result.published_at` | canonical timestamp string from the original publication attempt |

All timestamp strings are the service's exact twenty-character
`YYYY-MM-DDTHH:MM:SSZ` UTC format. Calendar validity remains checked by
`seconds_from_rfc3339`; offsets and fractions are refused. Thus binary string
ordering is chronological. Discovery never converts timestamps or scans with
`$expr`/`$dateFromString`. Digests named `*_sha256` and creation fingerprints are
64 lowercase hex characters. Numeric upper bounds and field size constraints are
in the schema. The codec additionally checks Binary subtype/length, valid calendar
dates, original operation identity, request-key recomputation, work budgets and
publication semantics; Mongo's `binData` schema type is not a receipt proof.

`_id` is the existing `RecoveryRequest::key()` hash of the original Agent creation
incarnation and failure sequence. It excludes lease, attempt and later timestamps.
The request and target never change. A new holder advances work revision and
Agent native-writer fence; it never allocates a replacement logical recovery key.
`attaching` iff lease is nonnull; `completed` iff completed_at is nonnull.
Completed publication time equals completed_at. A lease fence is below next_fence.

## Original result witness

Use S4's existing publication digest algorithm unchanged. The witness records
`result_digest(original_result_agent)` and `result_digest(original_exact_log_batch)`.
The immutable Publication result digest is:

```rust
publication::result_digest(&("cma.capacity-recovery.result/v2", result_binding, work))
```

The carrier contains the latest work revision's **original** result witness. On
each atomic step, replace work and result together under the exact prior carrier
CAS, alongside the Agent CAS, projections and immutable S4 publication. A later
Agent/head read never replaces the witness. Admission retains global sequence and
uses no events. Claim/retry/supersession use empty log batches; completion uses its
one Failed->confirmed-state StatusChanged event. The original admission receipt
remains at the recovery key; later immutable receipts use `key:step:work.revision`.
Receipt/status reads verify both admission identity and exact current-step result
binding plus certified retained source. A carrier alone cannot establish success.
Missing evidence is Unknown/unavailable, never proof of abort or permission to replay.

## Bounded discovery and exact indexes

The exact three nonunique index specifications are
`api/crates/cma/src/store/agent_capacity_recovery/format2.indexes.json`. Keep Mongo's
ordinary unique `_id_` index. Do not add TTL, partial, sparse, unique-per-Agent or
background history-enumeration indexes. Multiple historical recoveries for one
Agent remain addressable by their distinct original failure keys.

| Purpose / hint | Equality prefix | Ascending range and order |
| --- | --- | --- |
| `agent_capacity_recovery_due` | format_version=Int64(2), work.phase=`admitted` | work.ready_at, _id |
| `agent_capacity_recovery_expired` | format_version=Int64(2), work.phase=`attaching` | work.lease.expires_at, _id |
| `agent_capacity_recovery_agent` | agent_id, format_version=Int64(2) | _id |

Each worker sweep issues two independent indexed keyset queries. Take a fresh
canonical `now`; each query has `limit:32`, `batchSize:32`, `maxTimeMS:1000`, simple
collation and the matching explicit hint. No skip, count, whole-collection fold
or unbounded vector. On a full page, retain its last (time,_id) tuple; on exhaustion
start a new sweep. An already-visited row changed behind the cursor is picked up
by the next sweep. Interleave both streams so expired leases cannot starve.

Due example (BSON `2_i64`; CURSOR predicates omitted for the first page):

```javascript
{
  format_version: NumberLong(2), "work.phase": "admitted",
  "work.ready_at": { $lte: NOW },
  $or: [
    { "work.ready_at": { $gt: CURSOR_TIME } },
    { "work.ready_at": CURSOR_TIME, _id: { $gt: CURSOR_ID } }
  ]
}
// sort {"work.ready_at":1, _id:1}
```

For expired leases replace phase with `attaching` and every ready_at reference
with `work.lease.expires_at`, using the expired index. Discovery projects only
`_id`, `format_version`, `agent_id`, `work.request`, `work.revision`, `work.phase`,
`work.ready_at`, `work.lease`. Each candidate is untrusted metadata. Obtain current
resource authority and re-read/decode the full exact carrier/publications/parents
inside the S5 transaction before claiming. Never attach a runtime from discovery
results. Accepted work continues to be discoverable with admission gates closed.

Authorized per-Agent history/status enumeration uses the agent index, exact
Agent equality, format 2 equality, `_id > cursor`, sort `_id:1`, the same page
budget and fresh authority. Exact operation status uses `_id_` and validates all
request/incarnation fields. Terminal carriers are excluded from worker discovery;
they remain available to exact receipt/status reconciliation. Post-completion
attachment reconciliation must have its own current-authority entry and must
never reopen/resubmit the original input.

S1 qualification must verify the exact index options and validator drift, reject
format 1/missing result/unknown fields, accept the actual format-2 encoded carrier
through all four phases, and explain bounded index plans for both discovery
streams. No install or real-Mongo pass is claimed by this contract-only pin.
