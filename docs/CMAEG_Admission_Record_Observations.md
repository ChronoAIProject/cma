# EG admission record observations

Part of #1966 and #1370. These observations expose the result branches of the
replica record transaction and admission-index retirement. They do not change
the write, retry, guard, timeout, public-response or provider-send contracts.

The event name is `eg_admission_record`, with target
`cmaeg::records::mongo::replica_observation`. Phase starts and successful
operation ends use DEBUG; errors, deadlines, cancellation and failed operation
ends use WARN. Enable DEBUG for this target when a complete phase timeline is
required. Missing DEBUG output cannot establish that a phase did not occur.

## Correlation and timing

The observer captures the current tracing span when the record operation starts.
The ingress-owned `eg_admission_attempt` span supplies `attempt_sha256`,
`process_sha256` and `admission_sha256`. Its `eg_admission_operation` child supplies
`operation_sha256` and the ingress stage. An enclosing `cmaeg.request` span may
supply the HTTP request ID. These spans must instrument futures; an entered span
guard must not be held across an await. The records observer neither creates nor
guesses a missing HTTP/admission parent. Recovery and other callers can have none.

The shared admission digest identifies a logical event, not one HTTP attempt.
`record_operation_sequence` is a process-local ordinal; it needs the enclosing
process identity to distinguish different processes. Multiple records operations
can belong to one ingress operation. Diagnostic identities grant no authority.

`elapsed_ms` uses the record operation's Tokio monotonic origin. It includes all
transaction attempts and backoff for that operation. It is not a cohort, request,
database-command or delivery clock. Do not subtract values from different record
operations, replicas or clock origins.

## Fixed field contract

| Field | Allowed values |
|---|---|
| `record_operation` | `replica_write`, `admission_index` |
| `record_operation_sequence` | Unsigned process-local ordinal |
| `transaction_attempt` | 1 through 5 for a transaction; 0 for the index operation |
| `phase` | `validate`, `session`, `transaction_start`, `channel_guard`, `partition_guard`, `record_update`, `record_insert`, `commit`, `backoff`, `index_update`, `index_legacy_read` |
| `io_class` | `local`, `transaction_write`, `commit`, `write`, `read` |
| `boundary` | `start`, `error`, `end`, `deadline`, `cancelled` |
| `outcome` | `pending`, `ok`, `error`, `deadline`, `cancelled` |
| `error_class` | `none`, `unknown_commit`, `transient_transaction`, `unlabelled_mongo`, `not_found`, `conflict`, `guard_conflict`, `rejected`, `unavailable`, `malformed`, `deadline`, `cancelled` |
| `label_class` | `not_observed`, `unknown_commit`, `transient_transaction`, `unlabelled_mongo` |
| `commit_class` | `unconfirmed`, `unknown`, `aborted`, `acknowledged` |
| `elapsed_ms` | Unsigned monotonic milliseconds, saturating at u64 maximum |
| `mongo_labels_observed`, `transient_transaction_label`, `unknown_commit_label` | Boolean |

`io_class` classifies the client phase. Validation, session/transaction setup and
backoff are `local`; guard and record mutations are `transaction_write`; commit
is `commit`; index update is `write`; legacy index lookup is `read`. A `start`
is emitted before invoking the driver future. It does not prove that a command
reached Mongo or that a server-side write occurred. Use the returned typed
result and independent durable reconciliation to establish that fact.

Only `boundary=error` inspects a Mongo error's labels. Both known label booleans
are retained, and `UnknownTransactionCommitResult` takes precedence over
`TransientTransactionError` when both are present. The latter alone classifies
that failed transaction attempt as `aborted`. The nontransactional index operation
retains the label but stays `unknown`; it cannot establish a transaction abort.
Unlabelled errors remain `unknown`.
No raw error, server text, code name, arbitrary label, record ID, payload, URI,
owner or guard version is emitted. RecordError summaries use fixed variants.

The boolean label fields are meaningful only when `mongo_labels_observed=true`.
An operation-end summary has `label_class=not_observed`; false booleans there do
not assert that an earlier error lacked labels. Consumers must retain the error
row with its phase and attempt ordinal. A failed operation summary remains
conservative (`unknown`); it does not replace the earlier typed attempt result.

`acknowledged` on a successful operation means that its existing API returned
success. A replica transaction completed its commit call; index retirement
either matched its idempotent update or confirmed the accepted legacy path.
It does not establish HTTP acceptance, new FIFO custody or an external effect.
Index retirement continues to test matched count, not modified count, and does
not require the index to be active before clearing it.

## Deadlines and incomplete work

The transaction retains one two-second total budget, including backoff, and a
five-attempt cap. Only the existing known-abort classification permits retry.
Unknown commit, ordinary errors and version/guard conflicts retain their existing
return behavior. Write identity is unchanged across legal retries.

A deadline or dropped future emits the current phase with `commit_class=unknown`.
Cancellation is never proof of server rollback or permission to resend a provider
effect. A dropped future can emit a cancellation row even when an outer timeout
prevents its normal end. Abrupt process death can prevent even that row. Retain
missing/incomplete evidence and reconcile durable custody independently.

These diagnostics do not impose a new index or HTTP deadline. The inner record
budget is separate from response completion, terminal reconciliation, application
drain and harness cleanup. The historical aadd212 refusal's discarded exception
and c08 timeout phase remain unknown. A new controlled failure demonstrates its
bound source path; it cannot identify either historical trigger.

## Validation scope

The focused tests exercise the real direct-root RecordClient adapter with an
instance-local substitute for the Mongo transaction attempt. They preserve both
enclosing span joins across known-abort retry, unknown/both-label outcomes,
commit-before-reply timeout, outer cancellation and concurrent same-event
attempts. A closed inert Mongo driver exercises actual index-error mapping.
Hostile error strings verify that sensitive error data does not reach the event.
These are deterministic local tests, not cluster, real-Mongo transaction,
external-provider, terminal-effect or teardown/transition acceptance.
