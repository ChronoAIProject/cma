# Decoded JSON and RPC reply ownership

`CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED` is a strict boolean with a
`false` default. Both CMA transport compositions set this trusted policy after
renderer settings. Enabling it requires retained-output capability before ACP
initialization. It does not enable any other resource policy.

The policy reserves capacity before parsing owned JSON, including parser
scratch used by structural preflight. A single process shares a 32 MiB tree
pool and a 13 MiB scratch pool. Admission never waits: pressure returns
`AtCapacity`, and the attachment ends without synthesizing a terminal or ACK.
The remote retained source remains available for supported recovery. A refusal
before admission does not advance the accepted cursor or publish a receipt.

The parser accepts at most 4 MiB encoded bytes, 8,192 nodes (including object
keys and overwritten duplicate entries), and 64 nested containers. Invalid
JSON and excess shape are rejected before owned tree parsing. Both opt-in JSON
policies also refuse Serde's private RawValue and Number keys, including escaped
spellings. The pinned `raw_value` feature otherwise interprets a matching key
as an embedded parser, bypassing an outer JSON node count. Ordinary duplicate
keys retain the pinned parser's last-value behavior.

One admitted immutable tree is reused for bootstrap inspection, session identity
verification and capture. Startup queue records retain it until delivery or
teardown. Pending RPC reply channels carry the same owner; result projection
and reply clones share the entire original tree, including unused siblings.
The final tree allocation is freed before the last storage reservation returns.
There is no public mutable access or operation that extracts the tree separately
from its owner. Intermediate tree clones before typed session/permission/turn
conversion have been replaced by borrowed deserialization.

With both JSON policies off, legacy parsing is preserved. The new retention
flag off path keeps reply/error/missing-result behavior and result projection.
The existing structural-budget opt-in also receives the reserved-key fix. Durable progress deadlines, original payload hashes,
source identity and receipt ACK semantics are unchanged. The new policy shares
admission with all enabled attachments; it supplies no per-tenant fairness claim.

## Allocation derivation

This is a bound on requested allocation bytes for the named parser storage,
not a measurement of allocator resident memory. The qualified representation is
64-bit Rust 1.96.1, `serde_json` 1.0.154 with `preserve_order` and `raw_value`,
`indexmap` 2.14.2 and `hashbrown` 0.17.1. The lockfile pins dependency versions;
changes to those versions, features or representations require requalification.

For encoded length B and preflight node count N, reserve:

- Tree: `2*B + 1024*N + 1024` bytes, held through final tree ownership.
- Scratch: `3*B + 1024` bytes, held through preflight and owned decoding.

The allowances follow these allocation sites:

1. `serde_json/src/read.rs` uses a reusable byte vector to unescape a string.
   Decoded string length cannot exceed its encoded length. Rust's geometric
   byte-vector growth requires less than twice the encountered bytes, with a
   small minimum allocation. The scratch allowance also covers simultaneous
   old and new allocations during growth. Preflight ends and frees its decoder
   before owned parsing begins, so these two scratch vectors do not overlap.
2. `serde_json/src/value/de.rs` copies strings/keys and constructs arrays with
   `Vec::push`. Summed decoded string bytes cannot exceed B, including values
   later replaced by duplicate keys. The `2*B` string term conservatively allows
   copies and growth. Arrays use at most four slots for the first item and
   geometric growth thereafter; including old/new storage during growth, four
   `Value` slots per array element is conservative. `Value` is at most 80 bytes
   in the qualified representation, so this requires at most 320 bytes per
   counted element.
3. `indexmap/src/lib.rs::Bucket` holds a hash, String key and Value; its size is
   at most 112 bytes with these representations. `inner.rs::reserve_entries`
   grows entries to the hash table capacity. For incremental insertion,
   `hashbrown/src/raw.rs::capacity_to_buckets` uses power-of-two storage with a
   small-table minimum and 7/8 large-table load factor. Fewer than four slots
   per inserted entry covers both small tables and growth. Including old/new
   entry vectors gives at most six slots per inserted entry: 672 bytes. The
   index table stores usize indices and control bytes; old/new tables and
   alignment add less than 160 bytes per inserted entry. Thus 1,024 bytes per
   counted node covers container storage even without using the additional
   charge for each map key. Empty containers allocate no dynamic storage.
4. The fixed term covers the root tree Arc, its reservation owner and decoder
   error allocations. The tree Arc and last reservation are separate, private
   owners dropped in that order. The reservation control block is accounting
   bookkeeping; it can finish deallocating immediately after returning its
   permits. Semaphore infrastructure, channel/map entries and stack frames are
   outside this parser-storage pool and remain explicit aggregate owners.

The default pools admit an individual maximal compatible frame; concurrent
admissions can refuse immediately. Pools are distinct, and a tree admission
failure drops the scratch permit rather than waiting while holding raw storage.
No task waits for a capacity upgrade.

## Qualification and remaining scope

The test-only `tracking-allocator` 0.4.0 dependency, with optional tracing
features disabled, measures requested allocation
bytes by allocation group without counting unrelated test threads. Fixtures
cover maximal plain/escaped strings, wide arrays/maps, many small objects,
maximum depth, duplicate keys, malformed/over-limit/private-key refusal, shared
capacity, startup retention, delayed RPC receivers, projections, receiver loss
and cancellation. Requested live bytes, peak bytes, allowances and final release
are checked separately. On macOS, the first permit release lazily allocates a
64-byte pthread mutex for each semaphore. A captured allocation stack traced
this to `batch_semaphore::Semaphore::release`, not retained JSON. Tests initialize
that pool infrastructure before frame measurement; both mutexes remain pool
metadata until pool drop. Their bytes are excluded from frame samples and must
be included with accounting infrastructure in the aggregate/RSS qualification.
Measurements support the source calculation; they are not an RSS proof.
The following results were recorded on aarch64 macOS with Rust 1.96.1 at
`5e88b61a67573f2f3b6e0a70c80d15cbf1f5e379` (API tree
`bfdb5266c0df09fb007602a97fc33f0d6f340a05`). All 181 selected contracts passed.
All byte columns measure requested allocation storage or its allowance; none
measure allocator residency. Every fixture returned to exactly zero measured
frame bytes and restored both pools after its final owner dropped. Result
projection and cloning added no retained storage and kept the original root
charged through the last consumer.

| Fixture | Encoded bytes | Nodes | Retained bytes | Peak bytes | Tree allowance | Scratch allowance |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| max-string | 4194304 | 1 | 4194422 | 4194422 | 8390656 | 12583936 |
| max-escaped | 4194304 | 1 | 2097271 | 4194303 | 8390656 | 12583936 |
| small-objects | 21841 | 6826 | 636246 | 636246 | 7034530 | 66547 |
| wide-map | 35746 | 8191 | 834598 | 1205174 | 8460100 | 108262 |
| wide-array | 16383 | 8192 | 589944 | 884736 | 8422398 | 50173 |
| nested | 129 | 65 | 18552 | 18552 | 67842 | 1411 |
| duplicates | 69616 | 8191 | 480 | 480 | 8527840 | 209872 |

The large string fixtures reach the 4 MiB encoded boundary. The wide array
reaches 8,192 counted nodes; the nested fixture reaches 64 container levels.
Duplicate entries are charged during preflight even though only the final
value remains in the parsed tree. Success and refusal contracts additionally
cover escaped private parser keys, malformed JSON, limits, simultaneous
attachments, delayed replies, cancellation and receiver loss. The initial
partial test run is retained separately from this qualified result.


Typed domain/prepared updates, derived persistent strings and diagnostic
formatting, pending RPC bookkeeping, fold/serialization copies, codec fragments,
desktop output and global assemblies remain separate owners under #1702. The
component reservations (raw 16 MiB, permission 8 MiB, live fragment 32 MiB,
parsed tree 32 MiB and scratch 13 MiB) total 101 MiB; the remaining aggregate
128 MiB envelope must include all other owners and allocation metadata. This
slice does not establish the final aggregate or 70% RSS acceptance criterion.

Rollback disables only this flag for new attachments. Existing admitted owners
keep their reservations until they actually drop. Production activation and
acceptance remain coordinated through the dedicated harness and release owner.

Part of #1472. Implementation child #1718 under #1702.
