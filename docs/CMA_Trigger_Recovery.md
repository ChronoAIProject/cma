# CMA Trigger recovery and scenario contract

This document is the normative recovery, retry, re-click, and cancellation
contract for CMA GitHub Triggers. It consumes the state and failure vocabulary
in [CMA_Trigger.md](./CMA_Trigger.md) and the deployment controls in
[CMA_Trigger_Operations.md](./CMA_Trigger_Operations.md). It does not create a
second effect executor or a second sandbox-reconciliation authority.

## 1. Invariants

Every recovery path keeps these properties:

- A README URL contains only an immutable public Trigger identifier. It never
  contains a CMA key, GitHub token, prompt, or mutable configuration.
- Anonymous preview is subject-free and has no write port. Authentication,
  connection selection, review, and Start remain separate decisions.
- One contributor and immutable Trigger revision have one current Launch.
  Re-clicking returns it even when rollout is paused or the Launch is terminal.
- A separate Launch exists only after the contributor chooses **New Launch**,
  acknowledges the additional resources, and sends a new idempotent command.
- A public-command retry reuses the exact `Idempotency-Key` and byte-identical
  body. A changed body or changed intent uses a new key.
- A worker commits the effect intent and fence before external I/O. Unknown
  outcomes enter read-only reconciliation. No retry button changes an unknown
  write into another write.
- Forks, branches, commits, pull requests, Launch records, sessions, and
  retained workspaces are never deleted to recover, cancel, roll back, or
  disable Trigger orchestration.
- Trigger cancellation never stops an existing session. It requests capability
  revocation and leaves ordinary session continuity to the session lifecycle.
- Only a positively proven pre-write initial-prompt failure can retry
  automatically. Any other ambiguity holds the dispatch fence.
- A supervisor restart inside the same provider runtime may lose in-memory
  policy without changing the opaque runtime handle. CMA observes that exact
  handle under the session lifecycle fence and redelivers the current signed
  policy or tombstone; it never infers policy from static capabilities.
- Every GitHub request evaluated by Layer 4 has exactly one closed, redacted
  `cma_trigger_github_capability_decision` audit event at target
  `cma_trigger::github_capability_audit` and schema version `1`.

## 2. Resource accounting vocabulary

The matrices use this fixed vector, in this order:

```text
L/F/B/W/X/S/M/C/P
```

| Symbol | Counted resource |
|---|---|
| `L` | Durable Launch records |
| `F` | GitHub fork repositories |
| `B` | Launch-owned GitHub branches |
| `W` | Retained CMA workspace records |
| `X` | Active provider sandboxes |
| `S` | Durable CMA sessions |
| `M` | Initial-task owner turns admitted to the session |
| `C` | CMA-published contribution commits |
| `P` | Cross-fork pull requests |

A vector is the exact delta caused by the row after its stated precondition.
`0` means zero new resources, not merely that an existing resource was
retained. A fork can be reused across Launches, so a healthy Launch owns a fork
binding but need not have created that repository. Runtime replacement may
allocate a replacement sandbox, but the active sandbox count remains one after
the lost runtime is reconciled. The old provider allocation is owned by the
existing session reconciler, not Trigger cleanup.

For an unresolved provider write, the provider-side count is truthfully
`unknown: 0 or 1`; the CMA projection remains zero until positive identity
evidence is attached. The important exact assertion is that CMA issues zero
additional writes while the outcome is unknown.

## 3. Click, browser, and command scenarios

| Scenario and precondition | Exact settled result | Resource delta `L/F/B/W/X/S/M/C/P` |
|---|---|---|
| Anonymous first or repeated README click | `TriggerPreview`; no Launch state exists | `0/0/0/0/0/0/0/0/0` |
| First authenticated create-or-resume for a published revision with no current Launch | New `draft` Launch; no work starts | `+1/0/0/0/0/0/0/0/0` |
| Same contributor re-clicks the same revision, including from another device or tab | The exact current Launch and its existing state/revision are returned | `0/0/0/0/0/0/0/0/0` |
| Re-click after `completed`, `failed_terminal`, or `cancelled` | Existing terminal Launch summary opens; no task reruns | `0/0/0/0/0/0/0/0/0` |
| Re-click while orchestration is paused or the revision was retired after Launch creation | Existing Launch remains readable with the same state/block | `0/0/0/0/0/0/0/0/0` |
| Contributor explicitly confirms New Launch | New current `draft` Launch; the prior Launch is preserved and superseded only as the current-click target | `+1/0/0/0/0/0/0/0/0` |
| Browser refresh | `GET` reloads the authoritative Launch and progress cursor | `0/0/0/0/0/0/0/0/0` |
| Two tabs submit the same frozen command | One operation-store claim executes; both settle on the same response | First command's delta only; replay is `0/0/0/0/0/0/0/0/0` |
| Same idempotency key is reused with a changed body | `422 idempotency_key_reused`; Launch is unchanged | `0/0/0/0/0/0/0/0/0` |
| Browser goes offline after submission and later returns | The same key/body is reconciled; current Launch is reloaded before another action | Replay is `0/0/0/0/0/0/0/0/0` |
| CMA authentication expires before command admission | `401 not_authenticated`; command has no Launch or provider write | `0/0/0/0/0/0/0/0/0` |
| Start is accepted | Exact reviewed digest is confirmed and Launch becomes `preflight`; provider effects are only queued | `0/0/0/0/0/0/0/0/0` |

The client must keep an ambiguous command's key and original request closure
until a finite response is obtained. Refreshing the visible Launch does not
authorize a replacement command. An explicit reconciliation control invokes
only that retained closure.

## 4. External-call and lifecycle fault matrix

All rows begin with one existing Launch and the resources already recorded on
it. Unless a row names a positive provider creation, every resource delta is
`0/0/0/0/0/0/0/0/0`.

| Fault boundary | Exact Launch/effect result | Exact resource result |
|---|---|---|
| Worker stops before durable claim | Launch stays at the current step; replacement worker may claim it | Zero new resources |
| Worker stops after claim and before provider call | Current step remains; expired-lease takeover uses `reconciling`, never `execute` | Zero new resources |
| Positively proven pre-write timeout, connection refusal, or provider 5xx | `failed_retryable`, persisted retry time/budget, same effect intent | Zero new resources; a bounded retry may later use the same intent |
| Provider rate limit or capacity/quota refusal | `failed_retryable`, class `retryable_transient` or `resource_quota`, with the provider/reset time when available | Zero new resources |
| Provider outage before a known write boundary | `failed_retryable` with correlation ID and bounded retry; public reads and existing session remain available | Zero new resources |
| Timeout or worker loss after a write may have occurred | Current durable step, effect `reconciling`; after budget exhaustion: `failed_retryable` plus block `recovery_required` | Provider truth is `0 or 1`; CMA performs zero further writes until reconciled |
| Positive reconciliation proves fork creation | `fork_ready`; the same numeric fork is attached to the original intent | `0/+1/0/0/0/0/0/0/0`, with no duplicate fork call |
| Positive reconciliation proves Launch branch creation | `workspace_pending`; exact ref/head is attached | `0/0/+1/0/0/0/0/0/0`, with no duplicate ref call |
| Positive reconciliation proves workspace import | `sandbox_pending`; exact sealed baseline is attached | `0/0/0/+1/0/0/0/0/0`, with no duplicate import |
| Positive reconciliation proves sandbox allocation | `session_pending` | `0/0/0/0/+1/0/0/0/0`; one active sandbox |
| Positive reconciliation proves session creation | `prompt_pending`; exact session ID is attached | `0/0/0/0/0/+1/0/0/0`, with no duplicate session |
| Initial-prompt write becomes ambiguous after the dispatch fence | `prompt_pending` while observing; then `failed_retryable` plus block and delivery `recovery_required` if no terminal evidence arrives | Zero automatic new owner turns; original provider truth is `0 or 1` |
| Positive runtime evidence resolves an ambiguous prompt | Delivery advances to `execution_observed`, then `completed` or `failed`; Launch becomes `ready` only after the effect completes | Exactly one admitted owner turn for that dispatch generation |
| Positive reconciliation proves branch publication | Launch remains `contributing`; published contribution record names the exact reviewed commit | `0/0/0/0/0/0/0/+1/0`, with no force push or duplicate commit generation |
| Positive reconciliation proves pull-request creation | `pr_open`; numeric/node identity and Launch marker are attached | `0/0/0/0/0/0/0/0/+1`, with no duplicate PR |
| GitHub consent was revoked or scope became insufficient | `auth_required`, with `resume_state` equal to the interrupted durable step | Zero new resources; reconnect resumes the same Launch |
| Exact NyxID service is inactive | `auth_required` plus block `service_inactive` | Zero new resources; only that service is rechecked/reconnected |
| Owner credential is unavailable without revocation evidence | `auth_required`, with the interrupted `resume_state` and block `owner_credential_unavailable` retained | Zero new resources; after sign-in, revalidating the same service queues read-only reconciliation of the exact failed intent and resumes without another Start or blind provider write |
| Supervisor restarts inside the same provider runtime and reports an empty policy tuple for the unchanged handle | CMA observes under the session lifecycle fence and redelivers the current signed policy or tombstone to that exact handle; observation or parsing failure stays closed | Zero new resources, sessions, runtime rotations, prompt turns, or widened authority; redelivery alone is not revocation evidence |
| Source, base, or fork branch head is stale | `failed_retryable`, class `conflict_stale_head` | Zero new resources; CMA never force-updates the unexpected head |
| Forking disabled, occupied fork-network slot, workflow scope, or branch policy refuses the action | `failed_terminal`, class `policy_user_action` | Zero new resources; exact remediation is shown |
| Deleted/renamed/reclaimed repository, a Git blob over 16 MiB, or invalid/LFS/unsupported content fails validation | `failed_terminal`, class `terminal_validation` | Zero new resources at the refused boundary; an oversized blob is refused from tree metadata before its provider body is requested; prior resources remain |
| Numeric identity, signature, path, tenant, or capability binding fails | `failed_terminal`, class `terminal_security`; reach revocation is requested where applicable | Zero new resources at the refused boundary; alert/audit evidence remains |
| Session is rotating or recovering | Current Launch step is blocked as `session_rotating` or `session_recovering`; worker spends no retry budget | Zero new resources and zero new prompt turns |
| Runtime is lost after a complete workspace checkpoint | Existing session is reprovisioned from the latest complete Launch-authorized checkpoint | Active counts remain `0/0/0/0/0/0/0/0/0`; one replacement allocation, same Launch/session, no prompt replay |
| Checkpoint is corrupt or incomplete | It is never selected as a recovery point; use the last complete checkpoint or settle `recovery_required` | Zero new GitHub resources, sessions, or prompt turns |

The Launch detail action for GitHub authentication recovery deep-links to
`/launches/{launch_id}/recover`. That route reads the exact owned Launch by ID;
it never substitutes a newer current Launch from the same Trigger. A Launch
that is no longer both started and `auth_required` returns to its details
without exposing Start on the recovery route.

Provider-side ambiguity is not rewritten as a guessed count. A deterministic
fault fixture that knows the provider accepted the write must assert the
corresponding `+1` row and the reconciliation read count; a live outage without
that evidence must retain `unknown: 0 or 1` and assert zero replacement writes.

## 5. Cancellation matrix

Cancellation is owner-authenticated, bound to the current
`effective.digest`, guarded by `expected_revision`, and idempotent through the
durable operation store.

| Pre-cancel state/effect | Exact result | Resource delta `L/F/B/W/X/S/M/C/P` |
|---|---|---|
| `draft`, `auth_required`, `preflight`, `fork_pending`, `fork_ready`, `branch_pending`, `workspace_pending`, `sandbox_pending`, `session_pending`, `prompt_pending`, `ready`, `contributing`, `pr_pending`, `pr_open`, or `failed_retryable`; no unsettled external effect | `cancelled`; queued and `failed_retryable` effects become `cancelled`; completed/failed effects remain recorded | `0/0/0/0/0/0/0/0/0` |
| Any state with an effect `claimed`, `reconciling`, or `recovery_required` | Cancellation is refused atomically; state and effect are unchanged until reconciliation settles | `0/0/0/0/0/0/0/0/0` |
| `pr_pending` with a positively recorded open PR and no unsettled effect | PR identity is retained, then final Launch state is `cancelled` | `0/0/0/0/0/0/0/0/0` |
| `completed`, `failed_terminal`, or `cancelled` | Idempotent no-op; exact terminal Launch is returned | `0/0/0/0/0/0/0/0/0` |
| Any cancellable Launch with active reach | Launch becomes `cancelled`; reach becomes `revocation_pending` until acknowledgement or expiry | `0/0/0/0/0/0/0/0/0` |

Cancellation never stops `S`, tears down `X`, deletes `F` or `B`, rewrites
`C`, closes `P`, or removes `W`. Trigger cleanup may later delete only the
closed CMA-owned workspace object classes in the retained cleanup plan.

## 6. Owner-authorized initial-task recovery

The owner control is available only when all of these server facts agree:

```text
Launch.state = failed_retryable
Launch.blocked_reason = recovery_required
Launch.failure.class = recovery_required
Launch.resume_state = prompt_pending
Launch.delivery.state = recovery_required
the retained InitialDispatch effect is recovery_required
no worker lease is held
```

The UI additionally requires a nonempty retained `delivery.payload_hash`. It
states that CMA cannot determine whether the prior task started and that a
replacement may execute the task twice. The owner must deliberately acknowledge
that disclosure.

```http
POST /api/v1/launches/{launch_id}/recover-initial-prompt
Idempotency-Key: <one stable key for this authorization>
Content-Type: application/json

{
  "expected_revision": 12,
  "snapshot_digest": "<retained delivery.payload_hash>",
  "acknowledge_duplicate_execution_risk": true
}
```

Only the Launch owner may call this route. Acceptance returns `202` with the
Launch in `prompt_pending`, one `recover_prompt` confirmation, and one queued
replacement effect. Authorization itself has resource delta
`0/0/0/0/0/0/0/0/0`. The session outbox advances
`recovery_generation` only when it admits that authorized replacement. At most
one new owner turn can be admitted for each generation, so dispatch has delta
`0/0/0/0/0/0/+1/0/0`. Replaying the same command or worker claim has zero
delta. A later ambiguity requires a new disclosure, new command key, and next
generation; it is never inferred from the earlier acknowledgement.

If transport fails ambiguously, the client keeps the dialog mounted, disables
dismissal, and offers only same-key reconciliation. It must not mint a new key
or silently create a New Launch.

## 7. Operator-authorized effect reconciliation

Generic external-effect recovery is an operator-only read path. It requires
the existing `control_any_session` capability. It never changes an effect to
`queued`, increments its execution attempt, or invents a new intent.

The operator first reads exactly one redacted support snapshot:

```http
GET /api/v1/admin/triggers/support?launch_id=<launch_id>
GET /api/v1/admin/triggers/support?correlation_id=<failure-correlation-id>
```

Support requires `read_activity_log`. The operator must obtain the exact Launch
revision, effect kind, retained intent key, effect fence, and recovery-required
failure correlation from that snapshot and authoritative provider/runtime
evidence. A live lease must expire or settle; it is never broken manually.

```http
POST /api/v1/admin/triggers/launches/{launch_id}/reconcile
Idempotency-Key: <one stable key for this evidence decision>
Content-Type: application/json

{
  "expected_revision": 17,
  "effect": "pull_request",
  "intent_key": "<exact retained intent>",
  "expected_fence": 9,
  "evidence_correlation_id": "<exact retained failure correlation>"
}
```

Acceptance returns `202`, restores the retained `resume_state`, and changes
only the exact effect from `recovery_required` to `reconciling`. It schedules a
read-only provider inspection at the current time. Its resource delta is always
`0/0/0/0/0/0/0/0/0`. A body with a stale revision, wrong effect, wrong intent,
wrong fence, wrong evidence correlation, live lease, or non-recovery state is
refused atomically. Same-key/body replay returns the same response; same key
with a changed body is refused.

Initial-prompt replacement is not available through this operator route. It may
schedule another read-only inspection of the retained initial-turn evidence,
but only the owner disclosure route authorizes a new owner turn.

## 8. Recovery runbook

1. Query one Launch or correlation through the support route. Record build,
   environment, UTC interval, configuration revision, numeric repository, and
   redaction applied.
2. Confirm owner, publisher, revision digest, numeric upstream/fork/account,
   exact NyxID service, session, current state/block, and last event agree.
3. Inspect the lease. If it is live, wait for its normal settlement. If it is
   expired, allow the ordinary worker to acquire a higher fence.
4. Inspect the exact effect state, intent, attempt, reconciliation attempt,
   fence, provider request ID, external numeric identifiers, failure code,
   retry time, and failure correlation.
5. For a known pre-write failure, use the ordinary bounded retry path. For any
   possibly written call, query provider state by numeric identity, marker,
   ref, SHA, or provider request ID before doing anything else.
6. If positive evidence resolves the effect, let the ordinary reconciler attach
   that one result. If evidence is still unavailable and the budget is not
   exhausted, retain `reconciling` with a future retry time.
7. After a budget is exhausted, keep `recovery_required`. Use the operator
   endpoint only to authorize another read-only evidence pass. Never edit the
   aggregate or call the provider write endpoint by hand.
8. For an ambiguous initial prompt, show the owner the duplicate-execution
   disclosure. Do not authorize it on the owner's behalf.
9. For `revocation_pending`, wait for a Layer 4 tombstone acknowledgement,
   confirmed agent-key revocation, or the signed-policy TTL bound. Do not mark
   it revoked from request intent alone.
10. When the runtime handle is unchanged but readiness reports the explicit
    empty policy tuple, keep the lifecycle fence, redeliver the current policy
    or tombstone to that same handle, and re-observe it. Do not recreate the
    session, rotate the runtime, infer revocation, or replay the prompt.
11. Confirm the final Launch/event/activity projections carry the same safe
    correlation and that each evaluated Layer 4 GitHub call produced exactly
    one target `cma_trigger::github_capability_audit`, event
    `cma_trigger_github_capability_decision`, schema-version `1` decision. Its
    closed fields may contain Session/Launch and approved numeric repository
    and owner identities, never path, body, canonical name, service ID,
    credential, header, or provider response. Escalate missing or malformed
    audit/support evidence even if the effect itself reconciled.

During a provider-wide outage, pause Trigger orchestration at durable effect
boundaries, leave public reads and existing sessions available, keep the
capability revocation lane operating when possible, and do not increase retry
budgets to push more calls into the outage.

## 9. Deterministic evidence map

The following tests are the executable anchors for this contract:

| Contract | Test anchor |
|---|---|
| README re-click and explicit New Launch | `a_readme_reclick_resumes_current_and_new_launch_explicitly_supersedes_it` |
| Re-click while paused | `a_readme_reclick_can_open_the_current_launch_while_rollout_is_paused` |
| Durable claim, takeover, and no blind execute | `effect_claims_persist_one_intent_and_reconcile_after_a_crash`, `replacement_worker_reconciles_the_same_intent_and_fences_the_old_claim` |
| Retry deadlines and budgets | `retry_deadlines_and_budgets_are_durable_and_do_not_spend_early` |
| Prompt delivery fence and ambiguity | `trigger_prompt_fence_and_owner_message_are_durable_before_runtime_io`, `ambiguous_dispatch_never_resends_and_late_evidence_resolves_it` |
| One replacement turn per explicit recovery | `each_explicit_prompt_recovery_authorizes_exactly_one_new_generation` |
| Payload-bound owner recovery REST command | `prompt_recovery_is_explicit_payload_bound_and_replays_one_authorization` |
| Read-only operator recovery | `operator_recovery_can_only_schedule_read_only_reconciliation`, `trigger_effect_recovery_requires_and_replays_one_durable_operation_key` |
| Cancellation and resource preservation | `cancellation_settles_pending_work_and_preserves_every_external_record`, `cancellation_refuses_every_unsettled_external_outcome_atomically`, `cancellation_records_pending_capability_revocation_without_stopping_the_session` |
| Browser command ambiguity | `retains_the_key_and_original_request_closure_until_a_finite_response`, plus the Trigger review, Launch, and contribution reconciliation tests |
| Launch-authorized workspace selection for replacement provisioning | `runtime_replacement_restores_the_latest_launch_authorized_checkpoint` |
| Unplanned Trigger runtime loss without a durable conversation/workspace pair fails closed | `coordinated_trigger_runtime_loss_requires_verified_conversation_continuity`, `compatibility_trigger_runtime_loss_requires_verified_conversation_continuity` |

The workspace-selection anchor proves only that provisioning resolves the
latest complete Launch-authorized archive. It does not kill a runtime, restore
an agent conversation, or prove that the conversation and workspace were
captured as one recoverable pair. The runtime-loss anchors therefore preserve
the session in `recovery_required` instead of starting a blank conversation.
The row in section 3 that promises automatic recovery after a complete
checkpoint remains a release requirement until an end-to-end deterministic
test proves the paired capture, loss, restore, and no-prompt-replay path.

These deterministic tests prove local contracts. They do not replace the gated
real-provider evidence required by `#673`, and this document makes no claim that
a staging, canary, GitHub, NyxID, or production drill ran merely because a mock
or aggregate test passed.
