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

Connection preflight is an authenticated read and takes no idempotency key.
Beginning or reconnecting consent returns a NyxID authorization URL; the caller
redirects the browser there and resumes the same Launch after CMA's callback. The
callback itself never starts work.

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
review. Workflow acknowledgement is required only when `workflow_paths` is
nonempty and does not grant additional GitHub scope.

## Progress, failure, and recovery

`read_launch_progress` replays durable events strictly after the supplied cursor.
Persist `next_cursor`, reconnect from it, and refresh `read_launch` when state
changes. Use the structured `LaunchFailure` class, code, remediation, correlation
id, and retry time. Resume the same Launch after a retryable or authentication
failure. Never create a duplicate Launch as automatic recovery.

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

```rust,no_run
use cma_sdk::{
    Client, LaunchConfirmationRequest, PullRequestConfirmRequest,
    PullRequestOperation, PullRequestReviewRequest,
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
# Ok(())
# }
```

Accepted responses may precede the provider effect. Continue progress replay until
the Launch records the contribution or reconciled pull request. Later changes use
a new diff and the explicit `Update` operation. CMA never force-pushes an
unexpected branch head and never silently changes Open into Update.

`download_diff_artifact` returns the immutable artifact only after server-side
ownership, retention, byte-length, and digest checks. Its `delete_after` value is
the retention boundary.
