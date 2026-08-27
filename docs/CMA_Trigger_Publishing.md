# Publish and use a CMA GitHub Trigger

This guide covers the repository-facing CMA Trigger workflow. It is for a
developer who publishes a Trigger in a public repository and for a contributor
who opens that repository's **Open in CMA** link. The API and recovery details
remain authoritative in [API_REST.md](./API_REST.md) and
[CMA_Trigger_Recovery.md](./CMA_Trigger_Recovery.md).

## What the README link contains

A README link identifies one immutable, public Trigger revision:

```markdown
[Open in CMA](https://cma.example.com/triggers/PUBLIC_TRIGGER_ID)
```

Replace the example origin only with the public origin of the CMA deployment.
The publisher confirmation screen generates the complete Markdown with the
correct origin and public identifier; copy that generated value instead of
constructing it by hand.

The URL contains no CMA API key, GitHub token, prompt, repository credential,
mutable configuration, or contributor identity. Opening it is an anonymous,
side-effect-free preview. CMA authentication and the contributor's NyxID GitHub
OAuth consent happen after the preview, and neither callback starts work.

The generated plain-text link is the canonical form. A repository may present
the link in its own prose, but its destination must remain the exact generated
`/triggers/{public_id}` URL. Do not put a token, prompt, repository URL, branch,
callback, or configuration in query parameters or fragments. Do not route the
link through an untrusted redirect or analytics service.

## Publisher prerequisites

For the public MVP, the publisher needs:

- a CMA account;
- a NyxID `api-github` UserService connected to the GitHub identity that will
  authorize publication;
- `admin` or `maintain` permission on the exact upstream repository, or
  ownership of that personal repository;
- a public repository whose current GitHub and organization policy permits
  forking; and
- a Trigger configuration within the deployment's permission, resource,
  timeout, billing, and operation ceilings.

Generic `push`, `triage`, or organization membership is not enough to publish.
CMA resolves and stores GitHub's numeric repository and account identities
through the selected NyxID service. A typed refusal is not safely fixed by
typing a different owner/name pair or asking for broader scopes.

No Trigger API key is required. CMA authenticates the publisher when the draft
is created and published. NyxID owns the GitHub OAuth access and refresh tokens;
CMA records only the exact UserService and safe authority provenance. The public
MVP always uses the contributor's selected GitHub connection at Launch time.
Publisher-sponsored credentials remain disabled.

## Publish a revision

1. Sign in to CMA and open the Trigger publisher from the account menu.
2. Select or connect the exact NyxID GitHub identity that represents the
   repository.
3. Resolve the repository and source ref. Confirm the canonical owner, name,
   numeric identity, visibility, forkability, source mode, source SHA, and
   contribution base shown by CMA.
4. Define the public task summary, typed contributor inputs, session defaults,
   resource ceilings, timeout, branch policy, commit attribution, and pull
   request defaults.
5. Decide which bounded fields a contributor may change. Repository identity,
   credential routing, operation classes, permission ceilings, setup policy,
   and other safety fields remain locked.
6. Review the required GitHub scope classes and workflow-file policy. Publisher
   setup commands are disabled in the public MVP.
7. Publish explicitly. CMA revalidates the exact repository role and writes an
   immutable revision and digest.
8. Copy the generated **Open in CMA** Markdown into the upstream README and
   verify its anonymous preview in a signed-out browser.

Saving a draft does not expose a public revision. Publishing is the action that
freezes the content. If defaults or policy need to change, edit the mutable
draft and publish a new revision; an existing public revision never changes in
place. Update the README link when the repository should advertise the new
revision.

Retiring a revision prevents new Launch creation through that public identifier.
It does not cancel, rewrite, or delete a contributor's existing Launch, session,
fork, branch, commit, or pull request. Removing the README link changes
discovery only and is not a security revocation or cleanup operation.

## What a contributor sees

Opening **Open in CMA** follows this sequence:

1. CMA shows an anonymous preview with the publisher, canonical repository,
   public task summary, permission and resource ceilings, commercial summary,
   and availability. No fork, branch, session, or prompt exists yet.
2. The contributor signs in to CMA. The return state is short-lived,
   single-use, bound to the initiating browser and the same public revision.
   Signing in does not Start. Expiry, cancellation, or provider failure returns
   to the same preview with a retryable notice; a callback copied into another
   browser cannot consume the original attempt.
3. CMA resumes the contributor's current Launch for that revision or creates an
   editable review draft.
4. The contributor selects an existing NyxID GitHub connection or approves a
   GitHub OAuth handoff in NyxID. CMA then shows the exact account, service
   state, scopes, and preflight result. The OAuth callback still does not Start.
5. The contributor accepts the publisher defaults or changes only the fields
   the revision marked editable. CMA shows locked fields, effective permission,
   price/quota, source SHA, rendered task, branch policy, and workflow warnings.
6. The contributor presses **Start**. This is the first action that authorizes
   resource creation and prompt delivery.
7. Durable progress reports preflight, fork creation or reuse, Launch branch,
   workspace import, sandbox, session, scoped GitHub reach, and initial task
   delivery. The browser may close or refresh without cancelling the work.
8. The session opens when ready. GitHub access is supplied to the session
   through a short-lived, repository/ref-bounded NyxID agent capability; no raw
   GitHub or NyxID credential enters the browser, prompt, workspace, or sandbox.
9. **Contribute** captures a complete read-only diff for review. Publishing the
   reviewed branch is a separate confirmation, and opening or updating a pull
   request is another explicit confirmation.
10. Repository maintainers review and merge on GitHub. CMA never approves,
    merges, bypasses branch protection, or writes directly to the upstream.

Workflow-file changes are called out before Start and before publication.
Acknowledging the warning does not grant missing OAuth scope. A deployment may
refuse the publication path when the required workflow scope is unavailable.

## Re-clicks, retries, and recovery

A README re-click is safe and intentionally useful:

- while signed out, it returns the same inert preview;
- after sign-in, it resumes the contributor's current Launch for the immutable
  revision rather than creating another fork, branch, session, or prompt;
- after completion, it opens the existing session and pull-request summary; and
- a genuinely separate attempt requires **New Launch**, a fresh confirmation,
  and an acknowledgement that another branch and session may be created.

Multiple tabs and devices reconcile from server-owned Launch state and durable
event cursors. A progress spinner is never the authority for whether a step
happened.

For a failed command, keep the same Launch and follow its typed remediation.
Retry an ambiguous HTTP command only with the original idempotency key and a
byte-identical body. A changed body or a new user intent needs a new key. Do not
create a New Launch as automatic recovery.

Provider timeouts after a possible write are reconciled from retained intent
and numeric provider identity. CMA does not blindly repeat a fork, branch,
commit, prompt, or pull-request write. `recovery_required` means automatic
progress stopped because CMA could not prove whether an unsafe effect occurred.

An ambiguous initial task is stricter still. The automatic delivery path is
at-most-once. Only the Launch owner may authorize one replacement delivery after
accepting the visible warning that the task may execute twice. An operator may
request another read-only evidence pass but cannot accept that risk for the
owner. See [CMA_Trigger_Recovery.md](./CMA_Trigger_Recovery.md).

Cancelling settles only work that can be settled without guessing. It never
stops a running session and never deletes a fork, branch, commit, pull request,
or workspace needed for recovery. GitHub work remains owned by the contributor.

## Common outcomes

| Outcome | What it means | Contributor action |
|---|---|---|
| Sign-in required | CMA does not yet have a current contributor session | Sign in and return to the same Trigger |
| GitHub connection required | No suitable active NyxID GitHub service is selected | Connect, reconnect, or select the intended account |
| Scope insufficient | The service cannot perform a required operation class | Review NyxID/GitHub consent; do not paste a token into CMA |
| Upstream not forkable | Current repository or organization policy refuses an MVP fork | Ask the repository owner about policy or stop; CMA does not bypass it |
| Fork network occupied | The account already has another fork in the same network | Review the named safe remediation; CMA never deletes or repurposes it |
| Authentication required | Delegated CMA authority expired or was revoked | Reauthenticate, then resume the same Launch |
| Service inactive | The selected NyxID UserService is inactive | Reconnect that exact service, then resume |
| Rate limited or provider unavailable | GitHub or NyxID asked CMA to wait | Leave the Launch intact and follow its retry time |
| Feature paused | Operators paused new Trigger effects at a durable boundary | Existing session and GitHub work remain available; wait for resume |
| Recovery required | A possible external write or initial delivery is ambiguous | Follow the disclosed recovery flow; do not Start another Launch |
| Stale branch head | The fork branch no longer matches the reviewed publication intent | Review the current head and create a new diff; CMA does not force-push |

Every supportable failure exposes a safe correlation ID. Share the correlation
ID and Launch ID with support. Do not share OAuth values, agent keys, cookies,
authorization headers, prompts, private repository content, or downloaded diff
artifacts in a support issue.

## MVP boundaries

The public Trigger MVP does not provide:

- private or internal upstream repositories;
- publisher-sponsored GitHub credentials;
- publisher-authored setup commands;
- unrestricted session permission mode;
- Git smart HTTP clone or push credentials;
- automatic pull-request creation without contributor review;
- automatic approval, review, merge, force-push, or branch-protection bypass;
- writes to the upstream repository; or
- automatic deletion of GitHub resources.

The complete design and authority model are in
[CMA_Trigger.md](./CMA_Trigger.md). SDK users can follow the matching Trigger
workflow guides in [`sdk/typescript/docs/triggers.md`](../sdk/typescript/docs/triggers.md)
and [`sdk/rust/docs/triggers.md`](../sdk/rust/docs/triggers.md).
