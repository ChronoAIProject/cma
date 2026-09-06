# Agents

<!-- nav: 5 Agents -->

One Agent belongs to one allocated Worktree in an owned Workspace and Sandbox.
Its frozen Agent profile controls model, reasoning effort, permissions, and
skills. An Agent's conversation is shared by REST, AG-UI, and Responses.

```rust,no_run
use cma_sdk::{Client, EventOptions, IdempotencyKey};
use cma_sdk::agents::{AgentListOptions, AgentTextRequest};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let agents = client.agents().list(&AgentListOptions::default()).await?;
let id = std::env::var("CMA_AGENT_ID")?;
let key = IdempotencyKey::new(std::env::var("CMA_MESSAGE_KEY")?)?;
let input = AgentTextRequest { text: std::env::var("CMA_INPUT")? };
let accepted = client.agents().input(&id, &input, &key).await?;
let transcript = client.agents().transcript(&id).await?;
let usage = client.agents().usage(&id).await?;
let resources = client.agents().resources(&id).await?;
let history = client.agents().events(&id, EventOptions::default()).await?;
# let _ = (agents, accepted, transcript, usage, resources, history);
# Ok(())
# }
```

`create` takes an immutable Agent profile reference and allocates its Worktree.
`input` admits a user message; `steer` adds guidance to supported active work;
`interrupt` asks the Agent to stop its current turn. `answer_approval` targets
the exact server-issued approval identifier with `Allow`, `Deny`, or `Abort`.
`stop` and `delete` are distinct explicit lifecycle operations. Do not substitute
either for cancellation of local observation.

Durable transcripts and usage survive reconnects. Live reasoning and text
deltas are not durable transcript evidence. Resource counters are Sandbox-wide
and may be unavailable; `None` is not a zero measurement. The terminal helper
returns the named unsupported-operation refusal: this milestone has no PTY.

Render each transcript turn from its authoritative chronological `messages`
array. Every entry has a positive durable `sequence`, a closed `user` or
`assistant` `role`, exact `content`, and an RFC3339 `appended_at` timestamp. This
array preserves owner steering interleaved with Agent answers. `owner_message`
is only the initial owner-message summary; `agent_messages` contains only
assistant-message summaries. Concatenating those summaries loses the actual
conversation order and later owner guidance.
