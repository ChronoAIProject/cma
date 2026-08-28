# TypeScript Trigger Workflows

<!-- nav: 55 Triggers -->

A CMA Trigger turns a published repository configuration into an owned Launch.
The public README link identifies an immutable Trigger revision; it does not
carry a CMA API key, a GitHub token, configuration JSON, or a prompt. CMA
authentication establishes the contributor, and NyxID retains the contributor's
GitHub OAuth credential.

## Security and retry boundaries

`readTriggerPreview` is always anonymous and side-effect-free. The SDK suppresses
both its configured bearer credential and browser cookies for that request. Every
other Trigger or Launch operation is authenticated normally.

`preview.estimated_cost` is a discriminated public contract. `exact_zero` is an
exact USD zero under the immutable Trigger policy. `contributor_estimate` is the
current deployment estimate for the published default resources and timeout;
authenticated review recalculates and pins the effective choices, so treat the
preview amount as provisional. `quote_unavailable` has deliberately no amount:
show that review must calculate the current quote rather than substituting zero.

Every state-changing method requires an `idempotencyKey`. Reuse the exact key only
when retrying the exact same command and body after an ambiguous transport result.
Use a new key for a changed edit, a later confirmation, or any other new intent.
The read-only connection preflight and pull-request review take no idempotency key.

Clicking the same README Trigger again should call `createOrResumeLaunch` again.
CMA returns the contributor's existing nonterminal Launch for that immutable
revision. `createNewLaunch` is the explicit, separately confirmed path for another
resource-consuming Launch.

## Publisher flow

Publisher configuration is ordinary authenticated CMA data. A developer selects
one exact NyxID GitHub service and supplies a `TriggerContentDraft`; no secret is
embedded in the public URL. Draft replacement uses `version` for optimistic
concurrency. The returned draft's `selected_service_id` records the exact
UserService that authorized its latest repository resolution. Keep that binding
when publishing; CMA refuses a different service until an explicit, verified
draft replacement records it. Publication freezes the content, upstream
identity, authority snapshot, and digest into an immutable revision.

```ts
import { createClient, type TriggerContentDraft, type TriggerRevision } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });

async function publishTrigger(
  serviceId: string,
  content: TriggerContentDraft,
): Promise<TriggerRevision> {
  const draft = await cma.createTrigger(serviceId, content, {
    idempotencyKey: crypto.randomUUID(),
  });
  return cma.publishTrigger(draft.id, draft.version, draft.selected_service_id, {
    idempotencyKey: crypto.randomUUID(),
  });
}

void publishTrigger;
```

Use `updateTrigger` to replace an unpublished draft and
`retireTriggerRevision` to prevent new Launches. Retirement never rewrites or
cancels Launches that already captured the revision.

## Contributor review and GitHub consent

Read the preview before asking CMA to create or resume a Launch. The Launch review
contains the immutable revision content, contributor-editable overrides, the
effective configuration and its digest, workflow-file warnings, exact pricing,
the locked kernel and pinned runtime capabilities, quota evidence, selected
connection, and current step state. `pricing`, `runtime_admission`, and `review_quota` must all be
present before enabling Start. `start_admission` appears only after CMA has
revalidated those facts for the accepted Start.

```ts
import { createClient, type Launch } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });

async function reviewAndStart(publicId: string): Promise<Launch> {
  const preview = await cma.readTriggerPreview(publicId);
  if (preview.availability !== 'published') throw new Error('Trigger is retired');

  let launch = await cma.createOrResumeLaunch(publicId, {
    idempotencyKey: crypto.randomUUID(),
  });

  const connections = await cma.listTriggerConnections();
  const connection = connections[0];
  if (connection === undefined) {
    const handoff = await cma.beginTriggerConnection(crypto.randomUUID(), {
      idempotencyKey: crypto.randomUUID(),
    });
    window.location.assign(handoff.authorization_url);
    throw new Error('GitHub consent handoff started');
  }

  await cma.preflightTriggerConnection(connection.service_id);
  launch = await cma.selectLaunchConnection(
    launch.id,
    launch.revision_number,
    connection.service_id,
    { idempotencyKey: crypto.randomUUID() },
  );

  launch = await cma.updateLaunchOverrides(
    launch.id,
    launch.revision_number,
    {
      task_inputs: {},
      work_label: 'Documentation fix',
      optional_checks: [],
    },
    { idempotencyKey: crypto.randomUUID() },
  );

  // `optional_checks` is an empty compatibility field in the public MVP.
  // CMA rejects configured checks until a bounded sandbox runner is enabled.

  if (
    launch.pricing === null ||
    launch.runtime_admission === null ||
    launch.review_quota === null
  ) {
    throw new Error('Launch admission evidence is incomplete');
  }
  if (launch.runtime_admission.kernel !== 'codex-trigger-native-v1') {
    throw new Error('Unsupported locked Trigger kernel');
  }

  return cma.startLaunch(
    launch.id,
    launch.revision_number,
    launch.effective.digest,
    launch.workflow_paths.length > 0,
    { idempotencyKey: crypto.randomUUID() },
  );
}

void reviewAndStart;
```

The closed kernel vocabulary also decodes `codex` for a retained pre-versioned
Launch. That value is historical and read-only. Clients may list and inspect
the Launch, refresh its CMA snapshot and events, follow its existing session,
read or download a retained diff artifact, create a separate New Launch, or
cancel it. They must not offer overrides, connection selection, Start, diff
capture, branch publication, pull-request review/open/update/status refresh,
completion, or initial-prompt recovery. CMA fences those commands even if a
stale client submits one. Any kernel other than exact `codex` or
`codex-trigger-native-v1` is rejected as a protocol error.

The OAuth callback never starts work. After CMA regains control, repeat
`createOrResumeLaunch`, list the connections, select the exact returned
`service_id`, and show the review again. `reconnectTriggerConnection` starts a
new NyxID consent handoff for an inactive or insufficient existing service.
`revokeTriggerConnection` deactivates one exact owned service. It is a keyed
command: retain the same key and body after an unresolved response. Its returned
public connection state is replayable evidence that CMA accepted the command;
it is not a claim that every previously issued provider credential became
unusable at that instant. Existing Launch, workspace, branch, commit, and pull
request records remain intact, and an affected Launch resumes through
`auth_required` after another eligible connection is selected and preflighted.

`startLaunch` binds the confirmation to `launch.effective.digest`. Workflow risk
acknowledgement is required when `workflow_paths` is nonempty; it does not grant
new GitHub scopes or claim that GitHub will run a fork's workflows.

The quote and runtime snapshot are server-owned and included in the effective
digest. Quota is displayed as current evidence and checked again at Start. If
Start returns `trigger_price_changed`, `trigger_runtime_changed`, or
`trigger_quota_exhausted`, disable Start and call `updateLaunchOverrides` with
the same configuration to obtain fresh evidence. Show the changed review and
require a new confirmation; never resubmit the stale digest automatically.

`draft_expires_at` is the server-derived pre-Start deadline. The exact instant
is expired. A direct README re-click can atomically replace a current expired
draft, while a Launch whose expiry was already durably settled uses
`createNewLaunch` after the user acknowledges the additional resource attempt.
`trigger_draft_expired` and `trigger_quote_expired` therefore route to the
retained Launch and explicit New Launch UI. The original deadline does not
prevent authentication recovery after a Launch has started.

## Progress and failure handling

Progress is durable cursor replay, not a transient progress animation. Persist
`next_cursor`, request events strictly after it, and refresh the Launch projection
when an event arrives. Sequence 1 carries the complete stable step table in its
`created` kind. Every event's `progress` field is the authoritative compact
state, failure/recovery, reach, and prompt-delivery snapshot; `step_changed`
replaces one row. Reconnecting with the last accepted cursor is safe. Ignore an
immediately repeated event only when its complete envelope is identical; the
SDK collapses that duplicate. It rejects a sequence gap, conflicting duplicate,
event for another Launch, or `next_cursor` that does not equal the last unique
accepted sequence. Restart replay from sequence 1 after such a protocol error.
The page limit defaults to `100`, accepts `1..=500`, and is rejected locally
before transport when it is outside that range.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });

async function replayProgress(launchId: string, after = 0): Promise<number> {
  const page = await cma.readLaunchProgress(launchId, after, 100);
  for (const event of page.events) {
    console.log(event.sequence, event.kind);
  }
  return page.next_cursor;
}

void replayProgress;
```

Use `launch.failure.class`, `code`, `remediation`, `correlation_id`, and optional
`retry_at` to render failures. Resume the same Launch after authentication,
capacity, or retryable failures. Never create another Launch as an automatic
recovery strategy. `recovery_required` means CMA could not prove an unsafe effect
did not happen; automatic retry is intentionally fenced until explicit recovery.
Cancellation is also digest-bound to the current `effective.digest` and retains
the Launch history.

Admission Problem Details use stable codes and typed remediation:

| Code                             | Meaning                                                      |
| -------------------------------- | ------------------------------------------------------------ |
| `trigger_admission_unavailable`  | CMA could not verify admission safely; retry later.          |
| `trigger_pricing_unavailable`    | No valid server-owned price is available.                    |
| `trigger_price_changed`          | Refresh and explicitly accept the current price.             |
| `trigger_quote_expired`          | Open a New Launch with current evidence.                     |
| `trigger_quota_exhausted`        | Owner or deployment capacity is currently exhausted.         |
| `trigger_capability_unsupported` | The named `capability` is unavailable on the pinned runtime. |
| `trigger_runtime_changed`        | Refresh and explicitly accept the current runtime.           |
| `trigger_draft_expired`          | Follow `remediation` to the New Launch flow.                 |

Treat these as review-state transitions, not transport failures. In particular,
do not retry Start for price, runtime, quota, quote, or draft conflicts.

Publisher validation uses HTTP `422` with stable code
`invalid_trigger_request`. Use `validation_category` and `remediation` to guide
repository identity/policy/ref, scope/capability, and commercial-policy fixes;
do not parse the English `detail` or echo the rejected value yourself.

For an ambiguous initial task, show the owner the retained Launch and state
plainly that CMA cannot determine whether the prior task started. Only after the
owner deliberately accepts that another dispatch may execute the task twice may
the client call `recoverInitialPrompt`. Freeze the revision, retained payload
hash, body, and one idempotency key for the command; same-key reconciliation is
the only safe response to an ambiguous transport result.

```ts
import { createClient, type Launch } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });

async function recoverAmbiguousInitialTask(
  launch: Launch,
  idempotencyKey: string,
): Promise<Launch> {
  if (
    launch.state !== 'failed_retryable' ||
    launch.blocked_reason !== 'recovery_required' ||
    launch.delivery?.state !== 'recovery_required'
  ) {
    throw new Error('This Launch is not awaiting initial-task recovery');
  }

  return cma.recoverInitialPrompt(launch.id, launch.revision_number, launch.delivery.payload_hash, {
    idempotencyKey,
  });
}

void recoverAmbiguousInitialTask;
```

The method sends `acknowledge_duplicate_execution_risk: true`; the application
is responsible for obtaining the deliberate owner acknowledgement before
calling it. Acceptance queues one replacement generation. It does not prove the
earlier generation did not execute and it does not itself send another turn.

## Contribution and pull request

Contribution is four distinct decisions:

1. `requestLaunchDiff` asks CMA to capture and seal a read-only workspace diff;
   it is bound to `launch.effective.digest` and writes nothing to GitHub.
2. `readLaunchDiff` reviews the sealed artifact, validation results, workflow
   paths, and any existing contribution or pull request.
3. `publishLaunchBranch` explicitly publishes exactly the reviewed
   `artifact.digest` to the guarded Launch branch.
4. `reviewPullRequest` seals an Open or Update proposal without writing GitHub;
   `confirmPullRequest` is a separate keyed command bound to its
   `intent_digest`.

Each reviewed file always includes paired `additions` and `deletions`. They are
exact numbers for text even when the bounded response omits `patch`, and both are
`null` for binary content whose line counts are unavailable. Do not reconstruct
statistics from `patch` or substitute zero for `null`.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });

async function contribute(launchId: string): Promise<string | null> {
  let launch = await cma.readLaunch(launchId);
  await cma.requestLaunchDiff(launch.id, launch.revision_number, launch.effective.digest, {
    idempotencyKey: crypto.randomUUID(),
  });

  const review = await cma.readLaunchDiff(launch.id);
  if (review.artifact === null) throw new Error('Diff capture is still in progress');
  launch = await cma.publishLaunchBranch(
    launch.id,
    launch.revision_number,
    review.artifact.digest,
    { idempotencyKey: crypto.randomUUID() },
  );

  const proposal = await cma.reviewPullRequest(launch.id, 'open');
  launch = await cma.confirmPullRequest(
    launch.id,
    'open',
    launch.revision_number,
    proposal.intent_digest,
    {},
    { idempotencyKey: crypto.randomUUID() },
  );
  return launch.pull_request?.canonical_url ?? null;
}

void contribute;
```

The accepted responses can precede the external effect. Continue progress replay
until the Launch projection records the sealed diff, published contribution, or
reconciled pull request. For later changes, request a new diff and use the
`update` proposal and confirmation. CMA never force-pushes an unexpected head and
never silently changes an Open request into Update or the reverse.

An opened PR remains `pr_open`, and the same Launch remains available for
another reviewed diff and explicit `update`. `completed` has two paths: a later
read-only provider observation positively reports it merged, or the contributor
explicitly finishes CMA orchestration while the provider PR is still open:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });

async function completeCmaWork(launchId: string): Promise<void> {
  const openLaunch = await cma.readLaunch(launchId);
  await cma.completeLaunch(openLaunch.id, openLaunch.revision_number, openLaunch.effective.digest, {
    idempotencyKey: crypto.randomUUID(),
  });
}

void completeCmaWork;
```

Explicit completion performs no provider call, converges remaining reach to
revocation, and leaves the fork, branch, commit, and open PR untouched. It closes
further CMA contribution/Update/status commands for that Launch. CMA observes
open PRs independently of the browser. To request an earlier observation, call
`refreshPullRequestStatus(launch.id, launch.revision_number, { idempotencyKey })`.
The `202` response means the observation was queued, not that GitHub was read.
Reuse the same key only to reconcile that exact command, then follow Launch
progress for `pull_request_status_refresh_requested` and
`pull_request_status_changed`.

`pull_request.status_failure` carries a safe status-read failure,
`provider_request_id` carries provider correlation, and `head_matches_launch`
reports manual head drift. When the head match is false, keep the PR readable but
disable Update. `closed_unmerged` remains top-level `pr_open` and should offer a
New Launch action; CMA does not reopen it.

`downloadDiffArtifact` returns the immutable complete artifact after CMA verifies
Launch ownership, retention, byte length, and digest. Treat `delete_after` as the
retention boundary and do not use the download URL as a durable credential.
