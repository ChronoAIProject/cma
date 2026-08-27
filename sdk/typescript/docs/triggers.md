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
concurrency. Publication freezes the content, upstream identity, authority
snapshot, and digest into an immutable revision.

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
  return cma.publishTrigger(draft.id, draft.version, serviceId, {
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
effective configuration and its digest, workflow-file warnings, pricing, selected
connection, and current step state.

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
      branch_suffix: 'documentation-fix',
      optional_checks: [],
    },
    { idempotencyKey: crypto.randomUUID() },
  );

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

The OAuth callback never starts work. After CMA regains control, repeat
`createOrResumeLaunch`, list the connections, select the exact returned
`service_id`, and show the review again. `reconnectTriggerConnection` starts a
new NyxID consent handoff for an inactive or insufficient existing service.

`startLaunch` binds the confirmation to `launch.effective.digest`. Workflow risk
acknowledgement is required when `workflow_paths` is nonempty; it does not grant
new GitHub scopes or claim that GitHub will run a fork's workflows.

## Progress and failure handling

Progress is durable cursor replay, not a transient progress animation. Persist
`next_cursor`, request events strictly after it, and refresh the Launch projection
when an event arrives. Reconnecting with the last accepted cursor is safe.

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

`downloadDiffArtifact` returns the immutable complete artifact after CMA verifies
Launch ownership, retention, byte length, and digest. Treat `delete_after` as the
retention boundary and do not use the download URL as a durable credential.
