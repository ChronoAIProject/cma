# Rust Trigger Workflows

<!-- nav: 55 Triggers -->

A CMA Trigger turns a published repository configuration into an owned Launch.
The README link identifies an immutable public revision and carries no CMA key,
GitHub token, configuration JSON, or prompt. The client authenticates normal CMA
operations; NyxID retains and injects the contributor's GitHub OAuth credential.

## Publisher flow

Draft writes use optimistic concurrency and exact caller-generated idempotency
keys. Publication freezes the content and verified authority into an immutable
revision. The public preview remains anonymous even when this `Client` has a
credential provider. The returned draft retains the exact NyxID UserService that
authorized its latest repository resolution; publication must use that stored
`selected_service_id` unless an explicit, verified draft replacement changes it.

```rust,no_run
use cma_sdk::{Client, CreateTriggerRequest, PublishTriggerRequest, TriggerContentDraft};

# async fn publish(
#     client: &Client,
#     service_id: String,
#     content: TriggerContentDraft,
# ) -> Result<(), Box<dyn std::error::Error>> {
let draft = client
    .create_trigger(
        CreateTriggerRequest {
            selected_service_id: service_id.clone(),
            content,
        },
        "publisher-create-01",
    )
    .await?;
let revision = client
    .publish_trigger(
        &draft.id,
        PublishTriggerRequest {
            expected_version: draft.version,
            selected_service_id: draft.selected_service_id,
        },
        "publisher-publish-01",
    )
    .await?;
println!("{}", revision.public_id);
# Ok(())
# }
```

Use `update_trigger` for an unpublished replacement and
`retire_trigger_revision` to prevent new Launch creation. Retirement never
rewrites a Launch that already captured the revision.

## README click, connection, and Start

Call `read_trigger_preview` first. It suppresses the configured credential and is
side-effect-free. Repeating `create_or_resume_launch` after another README click
returns the contributor's current nonterminal Launch for that immutable revision;
`create_new_launch` is the explicit path for a separate resource-consuming Launch.

Match `preview.estimated_cost` before displaying money. `ExactZero` is exact
under the immutable zero-cost policy. `ContributorEstimate` prices the published
default resources and timeout from the current deployment rate card, but
authenticated review recalculates and pins the effective configuration.
`QuoteUnavailable` carries no amount and must not be treated as zero.

Connection preflight is an authenticated read and takes no idempotency key.
Beginning or reconnecting consent returns a NyxID authorization URL; the caller
redirects the browser there and resumes the same Launch after CMA's callback. The
callback itself never starts work.

`revoke_trigger_connection` is a keyed command over one exact owned UserService.
Reuse the same key only to reconcile the same unresolved request. The returned
public connection projection proves CMA accepted the command, but does not claim
that every previously issued provider credential stopped working at that exact
instant. Revocation preserves Launch, workspace, branch, commit, and pull-request
history; affected Launches recover through `AuthRequired` after another eligible
connection is selected and preflighted.

```rust,no_run
use cma_sdk::{
    Client, Launch, LaunchStartRequest, SelectLaunchConnectionRequest,
};

# async fn start(
#     client: &Client,
#     public_id: &str,
# ) -> Result<Launch, Box<dyn std::error::Error>> {
let preview = client.read_trigger_preview(public_id).await?;
println!("{}", preview.public_summary);

let mut launch = client
    .create_or_resume_launch(public_id, "launch-create-01")
    .await?;
let runtime = launch
    .runtime_admission
    .as_ref()
    .ok_or_else(|| std::io::Error::other("runtime admission is unavailable"))?;
if runtime.kernel != cma_sdk::TriggerKernel::CodexTriggerNativeV1 {
    return Err(std::io::Error::other("unsupported locked Trigger kernel").into());
}
let connections = client.list_trigger_connections().await?;
let connection = connections
    .first()
    .ok_or_else(|| std::io::Error::other("GitHub consent is required"))?;
client
    .preflight_trigger_connection(&connection.service_id)
    .await?;
launch = client
    .select_launch_connection(
        &launch.id,
        SelectLaunchConnectionRequest {
            expected_revision: launch.revision_number,
            service_id: connection.service_id.clone(),
        },
        "launch-connection-01",
    )
    .await?;

let snapshot_digest = launch.effective.digest.clone();
let expected_revision = launch.revision_number;
let acknowledge_workflow_risk = !launch.workflow_paths.is_empty();
client
    .start_launch(
        &launch.id,
        LaunchStartRequest {
            expected_revision,
            snapshot_digest,
            acknowledge_workflow_risk,
        },
        "launch-start-01",
    )
    .await
    .map_err(Into::into)
# }
```

`Start` is bound to `launch.effective.digest`. A stale `revision_number` or
digest is refused instead of starting a configuration the contributor did not
review. `runtime_admission.kernel` is the closed, server-owned kernel selection
shown before Start; it is independent of the runtime image string and capability
inventory. Workflow acknowledgement is required only when `workflow_paths` is
nonempty and does not grant additional GitHub scope.

The closed vocabulary also decodes `TriggerKernel::Codex` for a retained
pre-versioned Launch. That value is deliberately historical and
`is_legacy_read_only()` returns true. Clients may list and inspect the Launch,
refresh its CMA snapshot and events, follow its existing session, read or
download a retained diff artifact, create a separate New Launch, or cancel it.
They must not offer overrides, connection selection, Start, diff capture,
branch publication, pull-request review/open/update/status refresh, completion,
or initial-prompt recovery. CMA fences those commands even if a stale client
submits one. Unknown kernel strings still fail deserialization.

Publisher policy validation remains HTTP `422` with code
`invalid_trigger_request`. The Problem Details extensions
`validation_category` and `remediation` provide safe next actions for repository
identity/policy/ref, scope/capability, and commercial policy without returning
the rejected value or provider evidence.

## Progress, failure, and recovery

`read_launch_progress` replays durable events strictly after the supplied cursor.
Its limit accepts `1..=500`. Persist `next_cursor`, reconnect from it, and refresh
`read_launch` when state changes. The SDK collapses an immediately repeated event
only when its complete envelope is identical. It rejects conflicting duplicates,
sequence gaps, mixed-Launch pages, and a `next_cursor` that does not equal the
last unique accepted sequence (or the supplied cursor for an empty page). Use the
structured `LaunchFailure` class, code, remediation, correlation id, and retry
time. Resume the same Launch after a retryable or authentication failure. Never
create a duplicate Launch as automatic recovery.

`RecoveryRequired` means CMA cannot prove that an unsafe external effect did not
happen. The fence intentionally prevents blind automatic retry. Cancellation is
bound to the current effective digest and retains the Launch history.

An ambiguous initial task may be replaced only after the owner is shown that
CMA cannot determine whether the prior task started and deliberately accepts
that the task may execute twice. Bind the authorization to the exact retained
revision and delivery payload hash, and retain one idempotency key across an
ambiguous transport retry.

```rust,no_run
use cma_sdk::{Client, Launch, PromptDeliveryState, RecoverInitialPromptRequest};

# async fn recover_initial_task(
#     client: &Client,
#     launch: Launch,
# ) -> Result<Launch, Box<dyn std::error::Error>> {
let delivery = launch
    .delivery
    .as_ref()
    .filter(|delivery| delivery.state == PromptDeliveryState::RecoveryRequired)
    .ok_or_else(|| std::io::Error::other("initial-task recovery is not available"))?;

client
    .recover_initial_prompt(
        &launch.id,
        RecoverInitialPromptRequest {
            expected_revision: launch.revision_number,
            snapshot_digest: delivery.payload_hash.clone(),
            acknowledge_duplicate_execution_risk: true,
        },
        "one-stable-recovery-key",
    )
    .await
    .map_err(Into::into)
# }
```

Acceptance queues one replacement generation. It neither proves the earlier
generation did not execute nor immediately sends the next owner turn. A second
ambiguous generation requires a new disclosure and new idempotency key.

```rust,no_run
use cma_sdk::Client;

# async fn replay(client: &Client, launch_id: &str) -> Result<u64, cma_sdk::Error> {
let progress = client.read_launch_progress(launch_id, 0, 100).await?;
for event in progress.events {
    println!("{} {:?}", event.sequence, event.kind);
}
Ok(progress.next_cursor)
# }
```

## Diff, branch, and pull request

The contribution flow deliberately separates four authorities: request a
read-only diff with the effective-configuration digest, inspect the sealed
artifact, publish exactly its digest to the guarded branch, then review and
confirm one exact Open or Update pull-request proposal. Review writes nothing to
GitHub and takes no idempotency key. Confirmation uses the proposal's
`intent_digest` and a new key.

Every reviewed file carries paired `additions` and `deletions`. They are exact
`Some` counts for text even when the bounded response omits its patch, and both
are `None` for binary content whose line counts are unavailable. A client must
not derive counts from the optional patch or interpret `None` as zero.

```rust,no_run
use cma_sdk::{
    Client, LaunchConfirmationRequest, PullRequestConfirmRequest,
    PullRequestOperation, PullRequestReviewRequest,
    PullRequestStatusRefreshRequest,
};

# async fn contribute(
#     client: &Client,
#     launch_id: &str,
# ) -> Result<(), Box<dyn std::error::Error>> {
let mut launch = client.read_launch(launch_id).await?;
client
    .request_launch_diff(
        &launch.id,
        LaunchConfirmationRequest {
            expected_revision: launch.revision_number,
            snapshot_digest: launch.effective.digest.clone(),
        },
        "diff-review-01",
    )
    .await?;

let review = client.read_launch_diff(&launch.id).await?;
let artifact = review
    .artifact
    .ok_or_else(|| std::io::Error::other("diff capture is still in progress"))?;
launch = client
    .publish_launch_branch(
        &launch.id,
        LaunchConfirmationRequest {
            expected_revision: launch.revision_number,
            snapshot_digest: artifact.digest,
        },
        "branch-publish-01",
    )
    .await?;

let proposal = client
    .review_pull_request(
        &launch.id,
        PullRequestReviewRequest {
            operation: PullRequestOperation::Open,
            title: None,
            body: None,
        },
    )
    .await?;
client
    .confirm_pull_request(
        &launch.id,
        PullRequestOperation::Open,
        PullRequestConfirmRequest {
            expected_revision: launch.revision_number,
            snapshot_digest: proposal.intent_digest,
            title: None,
            body: None,
        },
        "pull-request-open-01",
    )
    .await?;

let launch = client.read_launch(&launch.id).await?;
client
    .refresh_pull_request_status(
        &launch.id,
        PullRequestStatusRefreshRequest {
            expected_revision: launch.revision_number,
        },
        "pull-request-status-refresh-01",
    )
    .await?;
# Ok(())
# }
```

Accepted responses may precede the provider effect. Continue progress replay until
the Launch records the contribution or reconciled pull request. Later changes use
a new diff and the explicit `Update` operation. CMA never force-pushes an
unexpected branch head and never silently changes Open into Update.

An opened PR stays `PrOpen` and the same Launch remains available for another
reviewed diff and explicit `Update`. `Completed` has two paths: a later read-only
provider observation positively reports the PR merged, or the contributor calls
`complete_launch` while the provider PR is still open:

```rust,no_run
# use cma_sdk::{Client, LaunchConfirmationRequest};
# async fn finish(client: &Client, launch_id: &str) -> cma_sdk::Result<()> {
let launch = client.read_launch(launch_id).await?;
client
    .complete_launch(
        &launch.id,
        LaunchConfirmationRequest {
            expected_revision: launch.revision_number,
            snapshot_digest: launch.effective.digest,
        },
        "finish-open-pr-handoff-01",
    )
    .await?;
# Ok(())
# }
```

Explicit completion performs no provider call, converges remaining reach to
revocation, and leaves the fork, branch, commit, and open PR untouched. It closes
further CMA contribution/Update/status commands for that Launch. The refresh
command above only queues an immediate fenced observation and returns `202`; it
does not call GitHub on the request thread. Reuse its key only for the exact same
optimistic revision, then read progress for
`PullRequestStatusRefreshRequested` and `PullRequestStatusChanged`.

The PR projection's `status_failure`, `provider_request_id`, and
`head_matches_launch` fields distinguish a safe read failure, provider
correlation, and manual head drift. A false head match keeps the PR readable but
disables CMA Update. `ClosedUnmerged` remains top-level `PrOpen` and should offer
New Launch; CMA does not reopen it.

`download_diff_artifact` returns the immutable artifact only after server-side
ownership, retention, byte-length, and digest checks. Its `delete_after` value is
the retention boundary.
