# EG diagnostic record fault artifact

This inert preparation artifact generates a separately bound patch for the local
N1/N2 admission cases. Healthy source contains no fault module, feature, runtime
switch or fault endpoint. Developer15 owns the records templates; developer08
owns diagnostic composition, collector, authenticated fixture and effect oracle.
The environment owner controls any Mongo fault and execution window. Generation
and the retained unit gates do not qualify a cluster invocation or provider effect.

## Build input and binding

`.github/scripts/eg-admission/records-fault/build_patch.py` reads an immutable
40-character Git revision and verifies every relevant object against
`source-base.json` (reference `1630d06798cbb148d11cad68f9fc43b1f05f280c`). It refuses
changed records, root/store/queue or dependency objects. Equal objects may carry
this patch across a normal composition; that check is not an application review.
The original `aadd212` source is rejected. Original-path reproduction requires a
separate bound adaptation and review; this artifact cannot attribute that failure.

The private fixture bootstrap supplies exactly these JSON fields:

```json
{
  "schema": "eg_records_fault_v1",
  "database_name": "eg_diag_<32 lowercase UUIDv4 hex>",
  "admission_root": "<canonical actual admission UUIDv4>",
  "case_id": "<canonical generated UUIDv4>",
  "action": "pre_submit_refusal"
}
```

The root must be derived by the real admission identity algorithm from the
registered synthetic event, not an invented unrelated UUID. The dedicated database
must be owned by that fixture. These input values are identifiers, not authority.
Do not put connection strings, payloads, credentials or arbitrary names in this
file. The two additional actions are `pre_submit_stall` and `post_commit_unknown`.
Each case has its own bootstrap input, image, database/event and registered schedule.

Generate outside the repository (substitute actual paths and immutable revision):

```sh
python3 .github/scripts/eg-admission/records-fault/build_patch.py \
  --repository /absolute/owned/checkout \
  --revision 1630d06798cbb148d11cad68f9fc43b1f05f280c \
  --config /absolute/private/case.json \
  --output /absolute/private/new-packet
```

The output is `diagnostic.patch`, exact patched source and `binding.json`. The
binding includes base commit/tree, dependency/source hashes, generator/template
hashes, canonical config hash, selectors and patch hash. The generator neither
applies the patch nor builds or invokes anything. It refuses an existing packet.
The build owner must check the clean source, apply that exact patch, verify patched
hashes, and bind compiler, compiled executable, image digest and actual serving
Pod/process identities. Generated packets and binaries stay outside the repository.
Every diagnostic build is distinct from the healthy acceptance image.

## Selector and source boundaries

The patch changes only `mongo_replicas.rs` and adds three diagnostic Rust modules.
It selects a single expiring `INTERACTIONS` create with active Admission projection,
expected version None, no channel/partition/lease guard, and exact database/root
SHA256 matches. Ordinary roots fit this selector; guarded GitHub roots and batched
writes do not. The root hash uses canonical UUID string bytes, matching admission
observations. Database hash uses UTF-8 name bytes; case hash uses UUID bytes;
fixture hash uses sorted compact JSON bytes for the entire config.

A process-local atomic latch permits one firing per process. Each configured
replica can therefore fire once; this is not a cluster-wide latch. No restart is
allowed in a registered one-shot case. Bind Pod UID, container/process start and
restart count, all marker occurrences and actual serving identities. Any restart,
unexpected firing, missing required marker or rejected collector row makes the
case incomplete. Schedule the selected request at the designated ingress and
account for concurrent/replay attempts through both N2 ingresses. The case UUID
labels the build schedule; selection is by exact database/root, not HTTP request.

| Case | Actual source boundary | Expected evidence and limits |
|---|---|---|
| Pre-submit refusal | Before `start_session`, transaction start or any direct-root write | Claim latch and emit before_submission/not_submitted, then return RecordError::Unavailable through the existing public path. Root preparation chunks may already exist. The marker proves only this root attempt did not submit; independent root/FIFO/effect reconciliation must establish the event's outcome, including other replicas. |
| Pre-submit stall | Same boundary, inside existing `replica_retry::run` | Emit before_submission/not_submitted and await a pending future. The unchanged two-second total deadline cancels it. No new retry grant or timeout. Record actual HTTP, terminal, teardown and transition bounds separately. |
| Post-commit acknowledgement loss | Immediately after the actual awaited `commit_transaction` returned Ok, before returning versions | Claim latch only now; emit after_commit_ack/acknowledged. Construct a fixed typed UnknownTransactionCommitResult, observe it and pass the existing conversion. The label is injected client evidence. The marker records the acknowledged root commit boundary, not a provider effect. Readback may reconcile to healthy 202; preserve the actual result and prove one terminal effect across replay. |
| Known-abort recovery | Owner-operated Mongo command fault or actual transaction conflict | No fake AttemptError::Aborted is supplied by this patch. Require real transaction phase/labels, attempt ordinals, unchanged event/write identity, server outcome and successful retry, plus durable/effect reconciliation. |

The retry helper, two-second budget, five-attempt cap, label precedence, guards,
payloads and index behavior are unmodified. The existing test AttemptFixture still
bypasses real Mongo attempts: unit controls prove hook/timeout/classification
behavior only. They are not a committed transaction, HTTP acceptance or sink proof.
This patch does not inject lookup/chunk/readback/enqueue/index or lifecycle stalls;
the driver must map those branches to focused tests or additional reviewed cases.

The separately bound owner review packet carries proposed Mongo failpoint options.
They are not installed by this artifact. A client-wide appName selector does not
select one admission root: actual transaction phase/outcome and event correlation
are required, and any fault consumed by another operation stays incomplete.

## Collector contract

The target is `cmaeg::records::mongo::replicas::diagnostic_fault`, fixed message
`Diagnostic record fault fired`, and event `eg_fixture_record_fault`. Fields are
`fixture_sha256`, `case_sha256`, `database_sha256`, `admission_sha256`, `action`,
`boundary` and `commit_class`. Hashes are lowercase 64-character hex strings.
Allowed action/boundary/commit tuples are exactly the first three rows above.
These are fixed `&str` tracing fields, not Display of an error or arbitrary object.

The collector must preserve the real enclosing attempt and ordered operation spans,
require root_write ancestry, and require marker admission hash equality with the
enclosing attempt. The same field occurs in span and event: do not flatten them.
Native requests may lack an HTTP parent. Retain an explicit synthetic marker flag;
never reinterpret this event as server labels, final HTTP status, FIFO custody or
external effects. The preexisting collector at `882a5714` does not parse this event;
developer08's separately reviewed extension is a required execution prerequisite.
No extra metrics labels are added. Missing or unparsed evidence stays incomplete.

## Acceptance and retained validation

Five pure Python tests cover exact patch application for all three actions,
configuration rejection, deterministic binding, changed-source rejection and the
CLI's immutable output-packet contract.
Six Rust controls cover selector exclusions, atomic one-shot contention,
pre-submit refusal, exact paused-clock two-second stall, post-commit unknown/no
retry, and nested marker correlation/privacy. The last control's commit is
simulated and explicitly supplies no real-server claim. Each generated action
must pass pinned package fmt, strict all-target/all-feature Clippy, these controls
and the existing 17 records retry/observation tests. The receipt separates repeated
cohorts from unique tests and retains setup failures. No healthy source is committed
with the patch; transient gate application is restored and verified afterward.

Execution still requires a registered N1/N2 case inventory, budgets/concurrency,
reviewed driver/collector, authenticated provider fixture, independent durable and
sink ledgers, exact image/config/window binding, and cleanup custody. All HTTP
attempts, including 503/timeouts, proceed into reconciliation. Unique events
partition into proven-unadmitted, custody-established and admission-uncertain;
established custody partitions into direct-terminal, reconciled-terminal and
unresolved. No-loss completion requires zero uncertainty/unresolved work and exact
expected effects without duplicate/missing/stale/wrong-owner effects. A safety
PASS that preserves Unknown does not establish completion PASS. Shared teardown
and next-cohort success/stall evidence remains separate from the inner deadline.
Historical aadd212 exception and c08 missing phase remain unknown.
