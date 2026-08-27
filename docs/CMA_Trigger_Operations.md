# CMA Trigger operations

This is the deployment and incident runbook for the GitHub Trigger workflow
defined in [CMA_Trigger.md](./CMA_Trigger.md). The design contract remains the
authority when this runbook omits a product rule. `CLAUDE.md` section 5 remains
the authority for session continuity and cannot be waived by a feature flag or
an incident response.

## 1. Non-negotiable boundaries

- A rollout control may stop a new Launch or freeze its next durable effect. It
  must never terminate, restart, rotate, blank, or degrade a running session.
- A capability kill switch affects subsequent GitHub capability use. It does
  not end the conversation or discard the workspace.
- Effective revocation is reported only after Layer 4 acknowledges a higher
  generation tombstone or NyxID confirms agent-key revocation. Until then the
  state is `revocation_pending`. A disconnected stale policy is bounded by its
  60-second lifetime plus at most 5 seconds of accepted clock skew.
- Trigger cleanup deletes only CMA-owned baseline, checkpoint, and diff-artifact
  objects named by the retained Launch plan. It never enumerates or deletes a
  sandbox, fork, branch, commit, pull request, or any other GitHub resource.
- A provider timeout after a possible write is reconciled from numeric identity
  and retained intent. It is never converted into a blind retry.
- No prompt, task input, repository file content or path, request body, OAuth
  value, agent key, cookie, authorization header, or provider response body may
  enter an activity record, support response, or structured worker log.
- Sponsored credentials remain disabled for the MVP. Every public Launch uses
  the contributor's selected NyxID GitHub connection.

## 2. Worker activation prerequisites

The public preview, review, and support surfaces can run without provider
workers. The in-process scheduler starts only when all of these exist:

1. CMA runs in provider authentication mode and constructed the typed NyxID
   GitHub broker.
2. `CHRONO_PLATFORM_BASE_URL`, `CHRONO_STORAGE_CLIENT_ID`,
   `CHRONO_STORAGE_CLIENT_SECRET`, and `CHRONO_TRIGGER_WORKSPACE_BUCKET` produce
   a durable Trigger workspace store.
3. `CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_ID` and
   `CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_PKCS8` produce a usable policy signer.
4. The normal `SessionService` is available for sandbox, session, prompt,
   checkpoint, and runtime-policy effects.
5. The one executor implements every closed `LaunchEffectKind`, including
   `Cleanup`.

Startup logs `Trigger provider workers are disabled` with the exact
`missing_prerequisites` list when any item is absent. Trusted local auth never
starts provider workers. Do not work around this gate by constructing a partial
executor.

The scheduler still starts when `CHRONO_TRIGGER_ENABLED=false`, provided all
prerequisites exist. That is intentional: orchestration records a durable pause
while the independent capability lane can apply the kill switch to reach that
already exists.

## 3. Deployment controls

All controls are non-secret ConfigMap values. Invalid booleans, cleanup modes,
numeric identities, percentages, page sizes, page budgets, and concurrency
values fail startup rather than falling back to a broader policy.

| Key | Initial value | Operational meaning |
|---|---:|---|
| `CHRONO_TRIGGER_ENABLED` | `false` | Environment admission. `false` refuses new publisher and Launch admission and pauses existing orchestration at its next durable boundary. |
| `CHRONO_TRIGGER_ORCHESTRATION_PAUSED` | `true` | Emergency freeze for new Launch effects and not-yet-dispatched prompts. It does not interrupt a session. |
| `CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH` | `true` | Requests capability revocation and prevents renewal. It remains independent from orchestration admission. |
| `CHRONO_TRIGGER_PUBLISHER_SUBJECT_IDS` | empty | Comma-separated exact NyxID subject IDs allowed to publish or launch. Empty denies all. |
| `CHRONO_TRIGGER_REPOSITORY_IDS` | empty | Comma-separated numeric upstream GitHub repository IDs. Names are not accepted. Empty denies all. |
| `CHRONO_TRIGGER_USER_COHORT_PERCENT` | `0` | Stable SHA-256 subject cohort from 0 through 100. Zero admits no contributor. |
| `CHRONO_TRIGGER_CLEANUP_MODE` | `dry_run` | `dry_run` persists an exact plan but has no deletion port. `execute` sends eligible cleanup through the normal fenced effect worker. |
| `CHRONO_TRIGGER_WORKER_INTERVAL_SECONDS` | `5` | Cadence of each independent lane, 1 through 300 seconds. Missed ticks are skipped. |
| `CHRONO_TRIGGER_WORKER_PAGE_SIZE` | `50` | Candidate rows per page, bounded by the public core maximum. |
| `CHRONO_TRIGGER_WORKER_MAX_PAGES` | `20` | Hard page ceiling per lane and pass, 1 through 100. |
| `CHRONO_TRIGGER_WORKER_CONCURRENCY` | `4` | Hard concurrent candidate ceiling per lane, 1 through 32. |

Admission requires all of the environment, publisher, numeric repository, and
user-cohort decisions to permit the request. Turning on only the environment
flag cannot expose a Trigger accidentally. A README re-click returns the
contributor's current Launch before new-admission checks, so a pause does not
strand its review or support view.

## 4. Staged activation

Apply one control change at a time and retain its configuration revision in the
change record.

1. Deploy code and database indexes with the initial values above. Confirm all
   three scheduler lanes emit heartbeats and no provider worker prerequisite is
   missing.
2. Keep orchestration paused and the capability kill switch on. Add one approved
   publisher subject and one disposable public fixture's numeric repository ID.
3. Set the user cohort to a small non-zero percentage. Record deterministic
   in-cohort and out-of-cohort fixture subjects; admission remains masked by the
   environment and orchestration gates until the later steps.
4. Leave cleanup in `dry_run`. Review every eligible plan as described in
   section 8.
5. Turn `CHRONO_TRIGGER_ENABLED=true` while orchestration remains paused. Verify
   previews, reviews, re-clicks, support reads, and typed `feature_paused`
   projections.
6. Turn the capability kill switch off only after the signed Layer 4 evaluator,
   expiry, replacement, and revocation evidence gates are attached.
7. Unpause orchestration for the disposable repository and cohort. Verify that
   the recorded in-cohort subject is admitted and the out-of-cohort subject gets
   the typed cohort denial, then run the fault-injected Launch-to-PR drill before
   widening either allowlist or cohort.
8. Promote cleanup to `execute` only after the dry-run inventory and deletion
   canary meet section 8.

Never combine a runtime-image adoption with the Trigger cohort change. A runtime
image change rotates sessions under the normal session continuity protocol and
must be evaluated independently.

## 5. Pause and kill-switch matrix

An external call already in progress may finish or become ambiguous and enter
reconciliation. A flag cannot recall bytes already written. The next effect is
always claimed only after a durable Launch lease and a fresh rollout decision.

| Launch state | Orchestration pause | Capability kill switch | Session and user work |
|---|---|---|---|
| `draft`, `auth_required` | No new Start or provider effect. Existing review remains readable. | Revoke retained reach if one exists; normally none exists here. | No session is created or stopped. |
| `preflight`, `fork_pending`, `fork_ready`, `branch_pending`, `workspace_pending`, `sandbox_pending`, `session_pending` | Finish only the external boundary already in progress, retain its result, then persist `feature_paused` before another claim. | Persist `revocation_pending`, then seek tombstone or agent-key acknowledgement. | A session that already exists stays intact; no inferred rollback deletes earlier resources. |
| `prompt_pending` | Do not claim a not-yet-dispatched prompt. A committed dispatch fence follows the seven-state at-most-once recovery contract and is never blindly resent. | Revoke subsequent GitHub reach without ending the session. | Conversation, workspace, and visible history remain. |
| `ready` | No new automatic GitHub/checkpoint effect is claimed. | Stop renewal and converge to effective revocation. | The session remains available for conversation, inspection, export, and recovery subject to the now-revoked GitHub reach. |
| `contributing`, `pr_pending` | Freeze the next effect. Reconcile a possibly written checkpoint, branch, or PR before settling. | Stop subsequent session GitHub use; CMA-owned ambiguous writes still reconcile under their retained intent. | Workspace, fork, branch, commits, and PR are preserved. |
| `pr_open` | No new Trigger write is claimed. | Revoke remaining session reach. | The open PR remains on GitHub and the session is not terminated. |
| `failed_retryable` | Preserve retry time and durable intent but do not retry while denied. | Revoke reach independently. | Recovery information and session linkage remain visible. |
| `completed`, `failed_terminal`, `cancelled` | No ordinary orchestration. Retention execution is also frozen in `execute` mode until rollout permits it. | Converge any remaining reach to revocation before cleanup eligibility. | GitHub work and sandboxes are never selected by Trigger cleanup. |

An existing `owner_credential_unavailable`, `service_inactive`,
`session_rotating`, `session_recovering`, `recovery_required`, or
`revocation_pending` block is not overwritten by a rollout pause. Only a block
that was durably recorded as `feature_paused` is automatically resumed when the
same Launch becomes permitted again. Activity actions
`trigger_orchestration_pause` and `trigger_orchestration_resume` carry the
stable `rollout_reason` values used by support tooling.

## 6. Scheduler and structured telemetry

The process runs three independent interval loops:

| Lane | Candidate source | Worker |
|---|---|---|
| `effects` | Active Launch states | Normal fenced effect execute/reconcile worker |
| `capabilities` | Launches that may hold short-lived reach | Fenced renew/revoke worker |
| `cleanup` | Terminal Launches past `workspace.delete_after` | Plan-only worker in `dry_run`; normal effect worker in `execute` |

Each page is only an inventory. The worker must still acquire the Launch lease
and optimistic revision. Cursor repetition stops the pass, `max_pages` bounds a
healthy but large inventory, `buffer_unordered` bounds lane concurrency, and
`MissedTickBehavior::Skip` prevents a slow pass from creating a timer backlog.
A slow provider effect therefore cannot delay capability revocation.

Every pass emits the info-level message `Trigger scheduler pass settled` with:

```text
lane, pages, candidates, completed, deferred, paused, failed,
not_acquired, no_work, errors, page_budget_exhausted, actionable,
elapsed_milliseconds
```

Every worker settlement also emits `launch_id`, `correlation_id`, action/effect,
outcome, fence, and safe policy generation or rollout reason. The durable
activity record may additionally contain only these allowlisted identifiers:

```text
subject, launch ID target, correlation ID, revision digest,
numeric upstream/fork/account IDs, exact UserService ID,
Launch state, closed effect kind, provider request ID,
fencing token, policy generation, cleanup target count, rollout reason
```

Use Launch state/event timestamps for conversion and step-latency panels. Use
scheduler heartbeats for queue and process health, effect records for retry and
reconciliation rates, capability events for renewal/revocation, and cleanup
records for retention backlog. Do not add high-cardinality prompt, path, branch
name, URL, or response-body labels.

## 7. SLOs and alerts

Provider-caused waits and explicit user-action blocks are measured separately
from CMA availability. A typed blocked state is a settled result, not a hidden
success and not an infinite spinner.

| Signal | Objective | Alert |
|---|---|---|
| Scheduler heartbeat | Every lane reports within `max(3 * interval, 60s)` | Page if any lane is absent for 2 objective windows while the process is Ready. |
| Candidate pass | No `errors`, `failed`, or repeated cursor in 99.9% of passes | Warn on 3 consecutive affected passes; page on 12 affected passes or 5 minutes. |
| Page budget | `page_budget_exhausted=false` | Warn after 3 consecutive passes; page if it persists for 15 minutes because work is arriving faster than the configured bound. |
| Lease contention | A candidate normally acquires or is stale by the next pass | Warn when `not_acquired / candidates > 0.5` for 5 minutes with no completion; inspect the support lease before intervention. |
| Durable step settlement | 99% of admitted non-user-blocked steps settle, defer with a retry time, or enter a typed block within 10 minutes | Page any Launch with no event and an expired lease for 10 minutes. |
| At-most-once dispatch | Zero automatic repeat after `dispatch_fenced` without positive pre-write failure proof | Page on any recovery-required dispatch that is automatically claimed as execute. |
| Capability expiry/revocation | No accepted stale policy beyond 60 seconds plus 5 seconds skew; confirmed revocation may be earlier | Page on policy acceptance outside the bound, generation rollback, or numeric-resource mismatch. Warn when `revocation_pending` survives beyond the bound plus two scheduler intervals. |
| Cleanup eligibility | 99% of eligible plans appear within 15 minutes; execute completions within 24 hours | Warn on a 15-minute plan backlog; page on a 24-hour execute backlog or any widened/tampered target rejection. |
| Session continuity | 100% across Trigger flags and cleanup | Page immediately if a flag or Trigger cleanup stops, rotates, blanks, or deletes a running session. |
| Credential/content leakage | Zero | Page immediately on any credential, prompt, repository path/content, or provider body detected in logs, activity, support, or artifacts. Preserve the evidence under incident policy and revoke authority. |

Dashboard panels must include README preview-to-review, review-to-Start, and
Start-to-ready conversion; state duration percentiles; effect execute versus
reconcile; retry/defer/failure class; fork readiness; initial dispatch state;
checkpoint/restore; branch/PR result; capability generation and revocation;
provider status/rate limiting; lease contention; scheduler page budget; and
cleanup backlog. Repository/dashboard infrastructure is owned outside this
repository, so its saved view must use the stable fields above and carry a link
to this runbook.

## 8. Cleanup dry-run and promotion

The `trigger-workspace-v1` policy assigns the workspace a concrete
`delete_after` 30 days after import. Cleanup becomes eligible only when the
Launch is terminal, a workspace exists, the time has passed, no completion was
recorded, and retained reach is absent or effectively revoked.

In `dry_run`, the planner:

1. Acquires the normal persisted Launch lease.
2. Calls the aggregate's closed `plan_cleanup` transition.
3. Retains the exact ordered target set and `cleanup_planned` event.
4. Releases the lease and records `trigger_cleanup_plan`.
5. Has no object-store, GitHub, or sandbox deletion port.

Review the support snapshot for every canary plan. Its targets are represented
as a count to the operator surface; the durable aggregate may contain only
`baseline`, `checkpoint { checkpoint_id }`, and
`diff_artifact { artifact_id }`. Reject promotion if serialized plan vocabulary
contains `sandbox`, `fork`, `branch`, `commit`, `pull_request`, a free-form
object key, or a target from another owner/Launch.

Before changing to `execute`:

1. Keep repository and publisher allowlists at the disposable canary.
2. Confirm every plan's retention policy, `delete_after`, plan timestamp, owner,
   Launch, target count, and effective capability revocation.
3. Confirm object-store deletion treats an already absent exact object as
   success and a tenant/object mismatch as terminal security failure.
4. Run one partial-failure canary and prove reconciliation repeats the identical
   owner/Launch-bound keys.
5. Confirm GitHub and sandbox request counters remain zero for the cleanup.
6. Set `CHRONO_TRIGGER_CLEANUP_MODE=execute` and watch one target set complete.
7. Return to `dry_run` on any unexpected target, delete error class, or audit
   gap. Do not edit a retained plan to make it pass.

Cleanup execution uses the same write-before-effect claim, fencing token,
retry/reconciliation contract, and bounded scheduler as every other effect.
There is no second deletion implementation.

## 9. Support procedure

The caller must hold the narrow `read_activity_log` operator capability. Query
exactly one identifier:

```text
GET /api/v1/admin/triggers/support?launch_id=<uuid>
GET /api/v1/admin/triggers/support?correlation_id=<safe-correlation>
```

Supplying both or neither is `invalid_trigger_support_lookup`. A deployment
without Trigger persistence returns `trigger_support_unavailable`. The response
contains the owner and publisher subject IDs, numeric repository/account IDs,
exact service/session IDs, state/block, safe lease fence and timestamps,
cleanup summary, closed effect records, and at most 500 timeline events. It has
no prompt, input, path/content, credential, raw object reference, or provider
body.

Investigate in this order:

1. Confirm `launch_id`, immutable revision digest, owner, publisher, numeric
   upstream ID, fork ID, account ID, and exact service ID agree.
2. Read the last timeline event and current `blocked_reason`.
3. If a lease exists, compare `expires_at` with canonical UTC. Do not break a
   live lease. After expiry, let the normal worker acquire a higher fence.
4. Match the effect `intent_key`, state, attempt, reconciliation attempt, fence,
   provider request ID, failure code, and next attempt time.
5. For `recovery_required`, attach authoritative provider/runtime evidence and
   use only the explicit recovery operation. Never rewrite the effect state.
6. For `revocation_pending`, establish tombstone acknowledgement, confirmed
   agent-key revocation, or TTL expiry. Do not label it revoked early.
7. Correlate the activity action and structured worker line. Treat a missing
   activity entry as an observability incident even when the Launch ledger
   proves the state transition.

For a generic external effect, the only explicit operator action is:

```text
POST /api/v1/admin/triggers/launches/{launch_id}/reconcile
```

It requires `control_any_session`, one durable `Idempotency-Key`, and exact
`expected_revision`, effect kind, retained intent key, effect fence, and failure
correlation from the support snapshot. Acceptance changes only that effect from
`recovery_required` to `reconciling`; the next pass is read-only. Wrong or stale
evidence is refused atomically. For an ambiguous initial prompt it may inspect
the retained turn evidence, but it cannot authorize a replacement dispatch.

An initial-prompt replacement is authorized only by the Launch owner through
`POST /api/v1/launches/{launch_id}/recover-initial-prompt`, bound to the retained
payload hash and an explicit acknowledgement that the task may execute twice.
An operator must not make that acknowledgement on the owner's behalf. The full
state, resource-count, cancellation, and recovery procedure is
[CMA_Trigger_Recovery.md](./CMA_Trigger_Recovery.md).

## 10. Provider outage and circuit breaking

- Keep public reads, existing session conversation, diff export, support, and
  durable inspection available. Provider health does not fail global CMA
  readiness.
- Persist retry or reconciliation time before releasing a worker lease. Honor a
  bounded provider `Retry-After`; never spin inside one lease.
- A known pre-write transient failure may retry under the effect budget. A
  timeout after possible bytes were written enters reconciliation or
  `recovery_required` according to the effect contract.
- When failures affect multiple Launches, set
  `CHRONO_TRIGGER_ORCHESTRATION_PAUSED=true`. Keep the capability lane operating
  unless its own enforcement endpoint is unavailable.
- If revocation enforcement is unavailable, leave `revocation_pending`, keep the
  kill switch on, and use policy expiry as the bounded disconnected backstop.
- Do not remove the pod from service merely because GitHub or NyxID is down;
  that would hide session and support reads that still work.

The bounded page, concurrency, retry-time, and per-effect reconciliation budgets
are the in-process circuit breakers. Increasing them during an outage usually
amplifies the outage and requires an explicit reviewed configuration change.

## 11. Rollback

Rollback is a forward-safe control sequence, not deletion and not a downgrade
of durable state.

1. Set `CHRONO_TRIGGER_ORCHESTRATION_PAUSED=true`. Confirm each affected Launch
   either finishes its current external boundary or records a durable pause.
2. Set `CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH=true`. Track
   `revocation_pending` until acknowledgement or the TTL bound. Do not stop the
   session to accelerate it.
3. Set `CHRONO_TRIGGER_CLEANUP_MODE=dry_run` to prevent new object deletions.
   Let an already committed delete settle or reconcile; do not manufacture a
   success receipt.
4. Set `CHRONO_TRIGGER_USER_COHORT_PERCENT=0`, clear both allowlists, then set
   `CHRONO_TRIGGER_ENABLED=false`.
5. Verify README re-clicks, Launch/event reads, support, session conversation,
   and safe export still work.
6. Keep the current database schema and a worker-capable image available until
   every claimed effect and capability debt is settled. Do not deploy an older
   writer that cannot read the new aggregate.
7. Re-enable through section 4 after the defect is fixed and the affected
   reconciliation paths are drilled.

No rollback step deletes or resets a fork, branch, commit, PR, workspace record,
or session. Removing a README link prevents discovery but is not a state or
credential rollback.

## 12. Schema, migration, and backfill

Startup creates bounded indexes for owner listings, immutable public revisions,
current Launch uniqueness, lease lookup, current-only worker inventory,
current-only non-revoked capability inventory, correlation lookup, and retention
lookup. Deploy with all rollout gates closed and verify index creation before
admitting the canary.

The added blocked reason, effect, capability, cleanup, lease-support, and event
fields are additive. Missing optional fields retain their documented defaults.
There is no bulk backfill that infers provider effects, capability revocation,
cleanup completion, or user confirmation from older data. Such inference would
invent authority. A pre-feature database has no Trigger rows and needs only the
indexes. Experimental Trigger rows that fail validation are quarantined from
public exposure and handled as migration evidence, not rewritten in place.

Before and after deployment, record:

```text
collection document counts; index names and readiness;
Launch counts by state and blocked reason;
live versus expired lease counts; effect states and recovery debt;
active/revocation_pending/revoked reach counts; cleanup eligible/planned/completed counts
```

Any count change not explained by a user action, worker event, or retention
receipt blocks rollout widening.

## 13. Retention, export, and deletion

- Request activity is retained by `CHRONO_ACTIVITY_RETENTION_DAYS` and the
  MongoDB TTL policy. The allowlist controls content independently of duration.
- Immutable revisions and Launch metadata are audit/reconciliation authority
  and are not silently expired. Retirement prevents a revision from creating a
  new Launch; it does not rewrite an existing Launch.
- Initial-prompt outbox/effect state is embedded in the Launch ledger. It is
  retained as the proof that prevents duplicate automatic dispatch and is not
  separately deleted as an orphan row.
- Workspace baseline/checkpoint/diff bytes follow the snapshotted
  `trigger-workspace-v1` plan and the fenced cleanup procedure above.
- GitHub forks, branches, commits, and PRs are user/provider-owned and remain
  outside CMA deletion, including privacy deletion and rollback.
- Sandbox teardown remains exclusively in the existing session reconciliation
  lifecycle and is never enumerated by Trigger retention.

A subject export uses the authenticated Trigger draft/revision/Launch/event and
diff endpoints for that subject. An operator may add the redacted activity and
support projections under the normal privacy authorization process. Never use
raw object-store keys or credentials as export material.

For an approved erasure request, first retire publisher revisions or cancel
contributor Launches under the subject's authority, request capability
revocation, wait for positive enforcement or expiry, and let eligible CMA-owned
workspace objects complete fenced cleanup. Preserve the minimum immutable
security/audit tombstone required by policy. This MVP exposes no public command
that deletes another subject's metadata and claims no legal-hold capability;
any database-level metadata erasure requires the platform's separately approved
data-controller procedure and must not break effect deduplication or credential
revocation evidence.

## 14. Required drills and evidence

Before public exposure, attach redacted evidence for:

- disabled, paused, publisher-denied, repository-denied, and cohort-denied
  admission, plus a README re-click of an existing Launch in each mode;
- a pause at every effect boundary, resume from `feature_paused`, and proof that
  every pre-existing non-rollout block remains unchanged;
- capability tombstone acknowledgement, agent-key revocation acknowledgement,
  provider failure, disconnected runtime, expiry, and generation rollback;
- worker crash before claim, after claim/before call, after possible write, and
  after provider success/before completion commit;
- page-budget exhaustion, cursor repetition, lease contention, provider rate
  limiting, and scheduler recovery on the next pass;
- cleanup dry-run vocabulary, tenant mismatch, partial deletion, idempotent
  repeat, and zero GitHub/sandbox calls;
- control-plane replacement while a Trigger session is running, proving the
  same session ID, history, workspace, and conversation continue; and
- support lookup and activity redaction with planted prompt, input, path,
  credential, and provider-body markers that never appear.

Evidence must name the build revision, environment, UTC interval, configuration
revision, numeric fixture repository, expected result, actual result, and
redaction applied. A production restart is never performed solely to collect a
drill transcript.
