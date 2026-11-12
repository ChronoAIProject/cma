# Triggers

<!-- nav: 8 Triggers -->

New inline targets use the system default image. The `image` type remains for historical data compatibility;
the server rejects new custom image selections with HTTP 422 and
`trigger_image_selection_unsupported`. Published targets continue to use their recorded images.
Bot `image` selection is also unsupported: the server returns HTTP 400 / `invalid_bot`,
with `image_selection_unsupported` in the detail. SDK type shapes remain unchanged.

A Trigger publishes immutable profile and reuse choices. It brings a visitor
to their own Agent, never the publisher's. Public preview, badge retrieval,
navigation, and OAuth callbacks do not admit Launch work.

Publishers use `triggers().create`, `save`, `publish`, `retire`, and draft-only
`delete`. `list_revisions` preserves publication history. Placements register
source URLs and labels; `embed` returns server-owned HTML, Markdown, and React
snippets for one registered placement. Use those descriptors unchanged.
`stats` returns aggregate and per-placement outcomes, not invented progress.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey};
use cma_sdk::launches::{CreateLaunchRequest, StartLaunchRequest};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let public_id = std::env::var("CMA_TRIGGER_PUBLIC_ID")?;
let preview = client.read_trigger_preview(&public_id).await?;
// Invoke only after an explicit visitor action.
let key = IdempotencyKey::new(std::env::var("CMA_LAUNCH_KEY")?)?;
let launch = client.launches().create_or_resume(&public_id, &CreateLaunchRequest::default(), &key).await?;
let requirements = client.launches().requirements(&launch.id).await?;
# let _ = (preview, requirements);
// Invoke only after review and explicit consent for the current disclosure.
let start_key = IdempotencyKey::new(std::env::var("CMA_START_KEY")?)?;
let consent = StartLaunchRequest::default();
let started = client.launches().start(&launch.id, &consent, &start_key).await?;
# let _ = started;
# Ok(())
# }
```

Launch creation can return 201 or 202; neither means an Agent is ready. Read
`review`, `resolution`, service requirements, and current trust digest before
starting. Reuse decisions and frozen profile references belong to the admitted
Launch. A changed trust digest requires a fresh explicit decision.

`create_connection` creates an owner-only hosted handoff without visiting its
external URL. Poll `read_connection`, then refresh requirements. The SDK never
calls NyxID directly. `cancel` only applies in supported states and does not
prove external creation has quiesced. Failed terminal Launches can retain
unresolved effect authority; never infer safe resource cleanup from that status.

## Trigger Quick Messages

`TriggerContent.quick_messages` accepts up to eight `TriggerQuickMessage` values,
each with a required name (120 UTF-8 bytes), description (2,000 bytes), and text
(32,768 bytes). Names are unique ignoring case and surrounding whitespace.
Empty arrays preserve legacy content digests. Publish after saving to expose
changes to visitors.

`client.agents().source(agent_id).await` returns immutable creation provenance
and the current published shortcuts. Draft edits remain private, and retired
Triggers expose no shortcut actions. The source read never starts work. Send
the selected text through the existing Agent message operation only following
an explicit user click; do not resend the first prompt on reload or revival.
