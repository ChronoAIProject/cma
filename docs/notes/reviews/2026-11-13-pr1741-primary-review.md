# PR #1741 primary review

Exact reviewed head: `821790b422b336684256abbf2b00d30386f40e99`.
PR: https://github.com/ChronoAIProject/cma/pull/1741
Review date: 2026-11-13. Scope: the complete 126-file PR diff against merge base
`affdcb04f72ff34bc37955762e4bdf5bc3fd09b9`, plus the complete 11-file
`8d318edc..821790b4` merge delta. This verdict concerns the original head;
subsequent local corrections require their own qualification and verdict.

**NO-GO: eight distinct blocker groups.** The recovered security report's five
findings overlap the first five primary findings; they are not eleven distinct
bugs. The sixth primary finding is the shared accounting hotspot. This review
also finds the queued-owner-policy and Revive receipt defects below.

## Authority and method

The original primary report was never published and was not used. The handoff's
six findings supplied the starting list. The recovered security review is
`a868a2ab0:notes/reviews/2026-11-12-pr1741-sec-review.md`, which reviewed
`8d318edc0347bca7f46444c8b211a3815cf94a29`. Its conclusions were cross-checked
against the current source and local reproductions, rather than treated as a
review of `821790b4`.

Reviewed the full implementation, tests, fixtures, configuration, SDK changes,
contract, implementation plan and implementation notes. Traced the changed
entry points through their downstream durable admission, recovery, command,
provider and settlement boundaries. The historical plan contains superseded
HTTP-status, accounting and concurrency choices; current contract and owner
rulings take precedence. The exact full-diff SHA-256 is
`50982ae164f886264e30c328eb6ecd977d823f2851fd60a11e5135605093a2d0`.

The linked issue #1496 and its accepted pre-request reservation comment were
rechecked. The budget guarantee requires a valued hold before upstream dispatch,
retains uncertain liability, and derives owner/operation scope from stored
records. Runtime requests outside CMA's broker remain a separate gateway
integration and activation prerequisite; these local results do not certify it.

## Blockers at the reviewed head

| ID | Severity | Finding and concrete consequence | Original source |
| --- | --- | --- | --- |
| B1 | P1 | An evaluator whose period CAS misses can still insert its previously computed closure. Refusal closure insertion also lacks current policy/accounting guards. A completed lift, correction or release can be followed by a stale fleet-wide STOP. | `store/budget/spend_ledger.rs:97`, `store/budget/period.rs:296` |
| B2 | P1 | Positive STOP caches authorize new durable turn cancellations and maintenance pauses without checking current STOP in their committing transaction. A newly admitted turn after lift can acquire an immutable cancellation; keep-alive or agentless Sandboxes can be paused after lift. | `store/budget/source.rs`, `sandbox/reconciliation_platform.rs`, `store/sandbox_maintenance/transaction.rs` |
| B3 | P1 | `admission_state` ignores the clock and treats an absent period as open. Registration and revival can proceed with missing/expired observations or an ended period until evaluation persists a closure. | `store/budget/spend_ledger.rs:110`, consumers in `auth/records.rs` and core lifecycle/control services |
| B4 | P1 | Owners with no applicable valuation take an unreserved path even with global metering active. Auxiliary requests can reach the HTTP provider without a hold and never enter platform accounting. | `store/budget/requests/admission.rs:34`, `store/budget/mod.rs:151` |
| B5 | P1 | Automatic failed-runtime recovery bypasses platform admission. A platform pause failing before suspension can lose its pause intent and reopen through retained-home recovery while STOP persists. | `sandbox/reconciliation_schedule.rs`, `reconciliation_recovery.rs`, downstream unpublished recovery |
| B6 | P1 | Every metered reserve and settlement reads and replaces the same period document; a process mutex also holds unrelated owners behind the full retry loop. The enabled path requires a budget-safe distribution design with measured contention and read-cost tradeoffs. | `store/budget/period.rs`, `store/platform_write_coordination.rs` |
| B7 | P1 | A command reserved under an owner policy skips the queued-turn STOP check if that owner policy is removed before execution. The worker dispatches an upstream request under an already persisted STOP. | `store/budget/mod.rs:355` |
| B8 | P2 | Revive checks platform refusal before its exact-key replay lookup. If the original acknowledgement was lost, a caller following the same-key retry guidance cannot recover its receipt during STOP; it may not know the generated command ID needed for the separate read route. | `layer2-core/src/sandbox/control/service.rs:154`, replay in `cma/src/store/sandbox_controls/transaction.rs` |

Paths in the first seven rows are under `api/crates/cma/src/` unless stated
otherwise. The correction record links the B1–B5 implementations and valid
before/after tests: [correction evidence](2026-11-13-pr1741-fix-evidence.md).

B7 reproduction uses the actual Mongo command worker: reserve a command, remove
its owner's policy while retaining an active global meter, persist STOP, then
run the worker. `a_reserved_queued_turn_checks_stop_after_its_owner_policy_is_removed`
fails with one upstream send where zero is required. The baseline exercises the
same unchecked branch as the exact original source; B1–B5 corrections do not
move this check. Baseline log SHA-256:
`60b717f3a0e77d6e119172730ddbbce1cc1f0bf3289a35cc2787849efb6e72e7`.

B8 reproduction invokes the real control service against the in-memory store:
admit Revive while open, enter STOP, retry the identical key/fingerprint.
`an_admitted_revive_receipt_is_recoverable_by_key_during_stop` fails with
`Budget(PlatformExhausted)` before the store can return its receipt. Baseline
log SHA-256:
`54db09c1ea7850b8fc9ba50459122b301fee457a805640f0f5c97fbc2ce3b19a`.

The lead confirmed on 2026-11-13 that exact-key, same-fingerprint receipt recovery
must precede new-commitment refusal, with current resource authorization checked.
New Revive keys and mismatched fingerprints remain refused during STOP. This
supersedes the deliberate refusal documented in implementation NOTES, review
round 3 (R7). The migration contract's exact-replay guarantee and HTTP same-key
retry guidance govern; returning a retained receipt creates no new commitment.

## Security-head delta and surrounding review

The complete 11-file delta introduces bounded asynchronous Mongo diagnostics,
its privacy/correlation tests and documentation. It does not change budget or
STOP authority and fixes none of B1–B8. The queue is bounded and nonblocking;
actual circuit decisions remain synchronous. Records carry allowlisted metadata,
not command/reply bodies. A stalled sink can delay loss reporting and abrupt
exit can lose queued diagnostics, as documented.

The new admin spend routes require ConfigurePlatform in the core service;
observation POST also checks authorization before validation. Inputs are strict,
bounded, server-period-scoped, and observations are immutable/idempotent. Owner
and turn bindings are recovered from durable records. Public errors and shared
transcripts use fixed guidance; closure diagnostics omit accounting values.
No additional direct cross-owner selector or confidentiality defect was found.

Checked the period arithmetic, cloud projection/freshness boundaries, settled
and retained liability, uncertain commit behavior, original-period settlement,
STOP receipt identity, refusal projection, creation/reuse/Launch guards, auth
callback directory ordering, keep-alive/agentless eligibility and workspace
busy fences. Existing-owner sign-in remains available during metering failure.
The settings singleton's pre-configuration default differs from a configured
meter missing its period; arbitrary database deletion was not used to invent
an additional supported-path defect.

Nonblocking limitations remain explicit: observation persistence and activity
audit are separate writes; an identical retry may duplicate audit records.
The admin spend routes lack OpenAPI/inventory/corpus registration in this head
(the implementation notes acknowledge this documentation gap). The advisory
signup-options response can remain cached after the platform state changes;
the callback is authoritative. Old strict readers are not an eligible rollback
floor after the new durable shapes have been written.

## Qualification and integration limits

At this review, GitHub still reports the exact original head above, OPEN against
`develop`. Its two failed checks are Backend Mongo contracts and the dependent
Backend aggregate, from run `43213243091`. The Mongo failure is the existing
terminal-settlement transaction-count contract: expected two transactions,
observed three due to the inactive adoption preflight. The develop correction
was merged locally, and the original contract passed in the earlier focused
Mongo run. No assertion was weakened. The original PR body has `Refs #1496`,
not the closing link required by this task; it must be aligned before merge.

The local B1–B5 checkpoint passed 62 Mongo tests plus affected-crate clippy.
Those runs qualify corrections, not the unsafe original head. The initial
counter selection passed 72/73; its failing new release fixture lowered only
the turn ceiling, which legitimately retains the old hold. The fixture now
also lowers the owner ceiling below its outstanding liability, retaining the
zero-send assertion. That failed run is not reported as green.

The initial local counter benchmark ran 30 operations per phase across 15
partitions in two waves. Routine root writes fell to zero and all measured
queries used indexes. Concurrent settlement p95 fell from about 65 ms to 14 ms;
admission p95 rose from about 97 ms to 114 ms, with roughly twice the admission
reply bytes. Admission retains the existing settings fence. These are internal
accounting transaction measurements on a shared host, not provider-path or
production throughput claims. The switch remains default OFF; activation is a
separate measured decision. Detailed measurements and final qualification belong
in the correction record. Initial measurement log SHA-256:
`93c2784483964a34721930d702813a289d4636f94c3e7542c261fdc365021429`.

The counter change owns its narrow retirement-census reader extension in this
PR, following #1754's independent accounting classification. #1754 must land
first, or whichever change lands second must resolve and verify the integration.
No production activation, push or merge is certified by this report.

## Reviewed PR inventory

Every file below belongs to the exact 126-file diff. The additional 11-file
security-head delta is identified above and in the correction record.

- `CLAUDE.md`
- `api/crates/cma/src/auth/flow.rs`
- `api/crates/cma/src/auth/flow/callback.rs`
- `api/crates/cma/src/auth/flow_registration_tests.rs`
- `api/crates/cma/src/auth/mod.rs`
- `api/crates/cma/src/auth/records.rs`
- `api/crates/cma/src/auth/records_tests.rs`
- `api/crates/cma/src/auth/signup_options.rs`
- `api/crates/cma/src/commands/preparation.rs`
- `api/crates/cma/src/composition.rs`
- `api/crates/cma/src/composition/credentials.rs`
- `api/crates/cma/src/composition/stores.rs`
- `api/crates/cma/src/config/features.rs`
- `api/crates/cma/src/config_tests.rs`
- `api/crates/cma/src/sandbox/composition.rs`
- `api/crates/cma/src/sandbox/reconciliation.rs`
- `api/crates/cma/src/sandbox/reconciliation_platform.rs`
- `api/crates/cma/src/sandbox/reconciliation_schedule.rs`
- `api/crates/cma/src/sandbox/reconciliation_sweep.rs`
- `api/crates/cma/src/sandbox/reconciliation_sweep_fixtures.rs`
- `api/crates/cma/src/sandbox/reconciliation_sweep_policy_tests.rs`
- `api/crates/cma/src/sandbox/reconciliation_telemetry.rs`
- `api/crates/cma/src/store/agent_admission.rs`
- `api/crates/cma/src/store/agent_admission/errors.rs`
- `api/crates/cma/src/store/agent_admission_mongo_integration_tests/creation_revival.rs`
- `api/crates/cma/src/store/agent_commands.rs`
- `api/crates/cma/src/store/agent_conversation_store.rs`
- `api/crates/cma/src/store/agent_conversation_store/admission_tests.rs`
- `api/crates/cma/src/store/agent_conversation_store/checkpoint.rs`
- `api/crates/cma/src/store/budget/cancellation.rs`
- `api/crates/cma/src/store/budget/mod.rs`
- `api/crates/cma/src/store/budget/period.rs`
- `api/crates/cma/src/store/budget/requests.rs`
- `api/crates/cma/src/store/budget/requests/admission.rs`
- `api/crates/cma/src/store/budget/requests/settlement.rs`
- `api/crates/cma/src/store/budget/source.rs`
- `api/crates/cma/src/store/budget/spend_ledger.rs`
- `api/crates/cma/src/store/contention.rs`
- `api/crates/cma/src/store/mod.rs`
- `api/crates/cma/src/store/platform_write_coordination.rs`
- `api/crates/cma/src/store/sandbox_admission.rs`
- `api/crates/cma/src/store/sandbox_maintenance/evidence.rs`
- `api/crates/cma/src/store/sandbox_maintenance/mongo_integration_tests.rs`
- `api/crates/cma/src/store/sandbox_maintenance/platform_stop_integration_tests.rs`
- `api/crates/cma/src/store/sandbox_maintenance/transaction.rs`
- `api/crates/cma/src/store/sandbox_mongo.rs`
- `api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests.rs`
- `api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests/budget_execution_tests.rs`
- `api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests/budget_tests.rs`
- `api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests/execution_tests.rs`
- `api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests/platform_commitment_tests.rs`
- `api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests/platform_period_tests.rs`
- `api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests/platform_unbudgeted_tests.rs`
- `api/crates/cma/src/store/sandbox_mongo_integration_tests/command_ledger_tests/request_budget_tests/settlement_tests.rs`
- `api/crates/cma/src/store/trigger_definitions/launch_mongo_integration_tests.rs`
- `api/crates/cma/src/store/trigger_definitions/launch_mongo_integration_tests/platform_spend.rs`
- `api/crates/cma/src/store/trigger_definitions/launches.rs`
- `api/crates/cma/src/store/trigger_definitions/launches/admission.rs`
- `api/crates/cma/src/store/workspace_admission.rs`
- `api/crates/cma/src/store/workspace_admission/retry.rs`
- `api/crates/layer1-public/src/conversation/commands/errors.rs`
- `api/crates/layer1-public/src/conversation/commands_tests/refusals.rs`
- `api/crates/layer1-public/src/responses/v2/controls/shapes.rs`
- `api/crates/layer1-public/src/responses/v2/error.rs`
- `api/crates/layer1-public/src/responses/v2/projection.rs`
- `api/crates/layer1-public/src/rest/admin.rs`
- `api/crates/layer1-public/src/rest/admin/auto_pause.rs`
- `api/crates/layer1-public/src/rest/admin/budget_tests.rs`
- `api/crates/layer1-public/src/rest/admin/platform_spend_tests.rs`
- `api/crates/layer1-public/src/rest/admin/settings/body.rs`
- `api/crates/layer1-public/src/rest/admin/settings/budget.rs`
- `api/crates/layer1-public/src/rest/admin/spend.rs`
- `api/crates/layer1-public/src/rest/instance_problem.rs`
- `api/crates/layer1-public/src/rest/instances_problem.rs`
- `api/crates/layer1-public/src/rest/sandbox_commands.rs`
- `api/crates/layer1-public/src/rest/sandbox_commands_tests.rs`
- `api/crates/layer1-public/src/rest/trigger_problem.rs`
- `api/crates/layer1-public/tests/fixtures/public-contract/admin.json`
- `api/crates/layer2-core/src/budget.rs`
- `api/crates/layer2-core/src/budget/ledger.rs`
- `api/crates/layer2-core/src/budget/period.rs`
- `api/crates/layer2-core/src/budget/period_tests.rs`
- `api/crates/layer2-core/src/budget/platform.rs`
- `api/crates/layer2-core/src/budget/rollout_tests.rs`
- `api/crates/layer2-core/src/budget/spend.rs`
- `api/crates/layer2-core/src/budget/spend_service.rs`
- `api/crates/layer2-core/src/budget/spend_service_tests.rs`
- `api/crates/layer2-core/src/budget/tests.rs`
- `api/crates/layer2-core/src/platform_service.rs`
- `api/crates/layer2-core/src/platform_service/budget_rollout_tests.rs`
- `api/crates/layer2-core/src/sandbox/agent_io/work.rs`
- `api/crates/layer2-core/src/sandbox/agent_operation_tests/budget_recovery_tests.rs`
- `api/crates/layer2-core/src/sandbox/agent_reads/visibility.rs`
- `api/crates/layer2-core/src/sandbox/agent_service.rs`
- `api/crates/layer2-core/src/sandbox/agent_service/support.rs`
- `api/crates/layer2-core/src/sandbox/always_on_tests.rs`
- `api/crates/layer2-core/src/sandbox/control/service.rs`
- `api/crates/layer2-core/src/sandbox/first_message/startup.rs`
- `api/crates/layer2-core/src/sandbox/inactivity.rs`
- `api/crates/layer2-core/src/sandbox/launch_service.rs`
- `api/crates/layer2-core/src/sandbox/lifecycle_service.rs`
- `api/crates/layer2-core/src/sandbox/lifecycle_service_tests.rs`
- `api/crates/layer2-core/src/sandbox/mod.rs`
- `api/crates/layer2-core/src/sandbox/service.rs`
- `api/crates/layer2-core/src/sandbox/store/maintenance.rs`
- `api/crates/layer2-core/src/sandbox/store/maintenance_tests.rs`
- `api/crates/layer2-core/src/sandbox/store/memory/agent_admission_tests/sandbox_controls_tests.rs`
- `api/crates/layer2-core/src/sandbox/store/memory/agent_admission_tests/sandbox_controls_tests/platform_tests.rs`
- `api/crates/layer2-core/src/sandbox/store/memory/maintenance.rs`
- `api/crates/layer2-core/src/sandbox/store/memory/maintenance_tests.rs`
- `api/crates/layer2-core/src/sandbox/store/memory/platform_stop_pause_tests.rs`
- `api/crates/layer2-core/src/sandbox/workspace_service.rs`
- `api/crates/layer2-core/src/sandbox/workspace_service/support.rs`
- `api/crates/layer2-core/src/trigger_domain/launch_service.rs`
- `api/crates/layer2-core/src/trigger_domain/launch_service_support.rs`
- `api/crates/layer2-core/src/trigger_domain/launch_store.rs`
- `docs/CMA_Model_Budget_Controls.md`
- `docs/CMA_Operations.md`
- `docs/CMA_Platform_Spend_Metering.md`
- `docs/CMA_Sandbox_Usage.md`
- `docs/plans/platform-spend/IMPLEMENTATION.md`
- `docs/plans/platform-spend/NOTES.md`
- `infra/api/configmap.yaml`
- `sdk/typescript/docs/operations.md`
- `sdk/typescript/src/admin/settings.ts`
- `sdk/typescript/tests/admin-availability.test.ts`

VERDICT #1741 @ 821790b422b336684256abbf2b00d30386f40e99: NO-GO (8 blockers)
