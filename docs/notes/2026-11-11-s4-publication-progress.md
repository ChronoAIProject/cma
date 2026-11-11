# S4 publication progress after the signature handoff — 2026-11-11

Part of #1350 and transactional #1349. Branch:
`tl/cma-log-segments/writer-publication`. Develop merge freeze remains active.

## Delivered signatures and dependencies

`453b417426e015459d24cc4e95d9f13586949a03` is the published S4 signature
checkpoint, reported at 18:53 SGT within the requested 20-minute window. It
normally merges S1 `5ed7eaae1` and provides the exact frozen
`writer_commit::{prepare,persist,PublicationInput,AggregateUpdate,PreparedPublication}`
and opaque `writer_bootstrap::BootstrapPlan`. Both temporary bodies in that
checkpoint refuse with typed Unavailable before writing. S5 can pin that SHA.

Subsequent normal merges consumed S2's isolated projection-root registration
`5f20045a6` and S1 root/schema registration `ec6002898`. The unrelated whole
integration tip was not imported. The common registration note assigns S4
`/Users/chronoai/code/work/.tl-worktrees/.heavy-0.lock`; the earlier assignment
question is resolved. Only that build lock may be used, with two Cargo jobs.

## Implementation continued

- `8c937b7ac` adds required `max_opened_generation` and
  `max_retired_generation` to the bounded projection. Each is the maximum
  **event.runtime_generation** for its own event kind, including the post-fence
  value on RuntimeGenerationRetired. They do not substitute current native or
  provider allocation state. Added fixtures cover later native changes, required
  serialization and inconsistent maxima.
- `9dbd8e1e8` uses S1's exact collection and schema ports for projection/receipt
  reads and the existing gate fixture, avoiding duplicate store handles.
- `5046577c1` implements persistence of an opaque prepared publication in the
  caller's existing transaction. It owns the single Agent CAS, exact original
  BSON batch, optional seal, Head, scalar projection, touched evidence and receipt.
  Existing evidence is validated and CAS-replaced; new rows are inserted.
  The caller's domain predicate is combined with source revision/global sequence,
  creation/incarnation/format, quarantine and retained-history guards. Agent
  replacement fields must match the planned result. Every error requires aborting
  the whole caller transaction; this helper never starts, commits or retries it.
- Pure/read-only assembly validates source/request bindings, original admission,
  pre-encoded capacity planning, projection deltas and staged bootstrap proof.
  The bootstrap proof has private fields and no production construction path.
- Capacity-recovery planning requires an empty batch, unchanged conversation and
  worktree identity, and a non-Ready result. S5 still owns eligibility, the durable
  recovery carrier and actual runtime confirmation.

Three focused real-Mongo test sources cover committed rollover; abort after
rollover and late receipt conflict; and stale domain fences, changed result
fields and a different session. They exercise persistence with test-only opaque
plans. They do not stand in for capture/adoption, end-to-end caller, unknown-commit
failpoint or actual old-binary compatibility tests.

`prepare` remains a typed refusal. The assembly function cannot be reached by
production callers, and no production path constructs `PreparedPublication`.
The executable path stays closed until captured-context and staging certification
are available. This is an implementation checkpoint, not a review-ready PR.

## Exact remaining integration requests

1. S1 needs to expose an opaque capture-context check usable by S4 `prepare`,
   proposed as `CapturedLog::validate_context(&ClientSession, CaptureMode) ->
   Result<(), TransactionLogError>`, or an equivalent owner-approved port. The
   frozen contract requires rejecting a different session/mode, but the current
   accessors expose no such check. MongoDB's public driver API exposes session ID
   while keeping transaction state/number private. This contract gap was relayed
   through the user to the integration owner; no private driver inspection or
   unsafe replacement proof has been introduced.
2. S1 still owns the writer child declarations listed in the frozen-signature
   handoff. S1 schema must include required string `appended_at` on PUBLICATIONS;
   its present strict validator rejects that new field. Gates/admissions need
   their reviewed writer schema and inventory registration.
3. S3 still owns serializable/equatable `agent_reads_usage::UsageFold`, plus
   `agent_reads_fold::validate_projection_identity(&Agent, &[AgentLogEntry]) ->
   Result<(), String>` delegating to `identity::validate(agent, None, entries)`.
   These exact missing exports were relayed through the user. Core registration
   is merged, but these support changes are not present in this worktree.
4. Certified staged bootstrap construction and the nine caller conversions,
   complete local/Mongo/crash/compatibility gates and exact-head review remain
   unfinished. Writer/recovery admission stays default-closed.

## Validation and cleanup

Workspace formatting, explicit Rust 1.96.1 formatting of S4 modules and whitespace
checks pass. All new/touched S4 source/test files remain below 500 lines. The
staged secret checks pass. Final file-size and full-history secret checks are
recorded in the milestone issue comment after the final documentation commit.

A focused `cargo check -p layer2-core --tests` launch was attempted under the
assigned heavy-0 lock. The lock was busy, so the nonblocking runner exited before
starting Cargo or creating a target directory. No compilation, clippy, unit-test
or Mongo-test pass is claimed. No Mongo fixture or build output was created.
Requested local notes and their committed mirrors are retained; temporary issue
comment bodies are removed after posting. The dedicated worktree remains for
lead verification and later removal.
