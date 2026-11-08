# Mongo response telemetry dependency

CMA needs returned BSON byte counts without serializing records or making the
driver decode a second owned copy of every command and reply. The published
MongoDB 3.9.1 command event API eagerly converts raw BSON to `Document`; its raw
events are private. CMA therefore retains the locked 3.9.1 release under
`api/vendor/mongodb` with an optional `response-summary` feature. No upstream
query, retry, cursor, session or transaction policy is changed.

The base is the published `mongodb-3.9.1.crate`, SHA-256
`af84443ae9878f59a6818686f339064479f42cdf293c4c637d8fc9c0a8c5eed6`.
The Apache-2.0 license is retained from upstream tag `v3.9.1`. Registry extraction
markers and the dependency's own Cargo.lock are omitted; the service workspace
lock remains authoritative. `api/vendor/mongodb-response-summary.patch` is the
complete functional delta against the published package, including the new
module. Imported dependency sources keep upstream formatting and layout; CMA's
new source modules remain subject to the normal source-size gate.

The hook classifies only command and collection names into a caller-owned
numeric token before I/O. CMA maps both through static allow-lists immediately.
No query, pipeline, database name, cursor identity, error detail or record is
passed to the observer. After a response arrives, the driver reads the borrowed
raw BSON length and the lengths of documents in `firstBatch`, `nextBatch`, or
`findAndModify`'s `value`. It skips document contents using BSON lengths. It does
not allocate or clone BSON, deserialize records, serialize values, or enable
the normal command event API. The feature is inert on clients without an
observer and excludes commands subject to the driver's existing redaction.

Received batches count even if the caller abandons a cursor. Retries count each
actual response. Network failures contribute zero received bytes; errors before
command I/O, heartbeats and authentication traffic are outside this boundary.
The success bit reflects the server's command result, not subsequent domain
validation. Full reply bytes include BSON envelopes and metadata but exclude
wire framing, compression, TLS and other Pod traffic. Scalar command results
such as `distinct` contribute response bytes, not cursor-document counts.

When upgrading the dependency, compare the small patch against the new raw
response path, preserve redaction, update the package checksum and patch, and
rerun the raw-summary and real-Mongo regressions. Prefer removing the patch
when upstream provides an equivalent borrowed, content-free response hook.
Do not replace it with full command monitoring to simplify an upgrade.

The regressions cover encoded lengths, first and subsequent batches, explicit
sessions, abandoned cursors, failed commands, fixed labels and content-free
exports. The ignored `raw_summary_overhead_for_five_thousand_documents` probe
compares summary/counter overhead with an owned BSON decode locally. It has no
timing assertion and needs no database. Run it under the shared build lock and
the normal two-job Cargo limit; use optimized results for overhead assessment.

Operational metric names and interpretation are in
[CMA Operations](CMA_Operations.md#api-read-and-stream-diagnostics).
