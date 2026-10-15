# CMA managed model settings

CMA's managed Agent default is `gpt-6`, with high reasoning effort and autonomous
permissions. The pinned Codex 0.147.0 can execute that configured provider name,
but its model catalogue does not advertise it. Codex ACP 1.4.0 validates settings
changes against that catalogue, so changing the effort or permission mode used to
reject the otherwise working model with HTTP 422.

The runtime image applies `managed_model_catalog.cjs` after the existing
conversation-persistence and subscription-authentication patches. Its input hash
and the final image's `CODEX_ACP_SHA256` pin the complete reviewed package bytes.
The patch adds `gpt-6` to ACP's model choices only when the effective configuration
explicitly selects provider `cma` and model `gpt-6`. The CMA effort contract for
this alias is `low`, `medium`, `high`, and `xhigh`. Other model names continue to
require an advertised runtime entry; unsupported effort values remain rejected.

The native Codex catalogue, its fallback instructions, and its tool configuration
remain the source of execution behavior. Existing advertised models retain their
metadata. A native subscription, another provider, or an ACP gateway override
keeps its original choices. The configuration is read again when the catalogue
is requested, allowing a conversation to switch to an advertised model and back
to its configured CMA alias after a process restart.

`managed_model_catalog.test.cjs` runs against the exact pinned Codex and ACP
programs with isolated homes and a local inference fixture. It demonstrates the
original rejection, successful settings acknowledgements after the patch,
unchanged native fallback instructions and tools, outbound model/effort values,
unknown-model and unsupported-effort refusals, and restoration of the configured
alias after loading the same conversation. The fixture does not establish that
an external model service supports a model; deployment acceptance separately
checks real model responses.

This compatibility patch is part of a new runtime image. Restarting Agent
Manager does not change an existing Sandbox's pinned image or rewrite retained
Agent homes. Deployment acceptance uses a new Sandbox on the final runtime and
a new Agent in that same Sandbox. Existing Sandbox histories, image selections,
and pending access requests are preserved.

### Event-driven workspace metadata and usage

The provider bridge forwards canonical working-directory snapshots from native
thread start/resume responses and `thread/settings/updated` notifications. It
forwards ACP session descriptions from title/history notifications. Running `cd`
inside a single command does not change the conversation's persisted directory.
CMA keeps Workspace and Worktree provisioning paths immutable and exposes the
observed directory separately as owned Agent `runtime_metadata.working_directory`
and its description as `runtime_metadata.description`. Durable
`runtime_metadata_changed` events keep the selected conversation current without
periodic metadata polling. Shared readers do not receive private directory metadata.

Native `thread/tokenUsage/updated` notifications supply consumed input and output
counters, including cached input. CMA stores the source baseline and absolute
lifetime totals in the generation-fenced conversation transaction. Replayed
snapshots do not add consumption; replacing a runtime does not reset accounting.
Context recomputation and compaction may decrease `context_used` without changing
consumed totals. An explicit native context-full sentinel zeros only the source
baseline and contributes no usage. Unexplained regressing snapshots are ignored
rather than guessed. Missing historical provider usage cannot be reconstructed
from transcript text. Retained runtimes keep their prior usage projection until
the newer bridge supplies a verified cumulative snapshot.

The additive `usage_updated` event carries absolute lifetime `input_tokens` and
`output_tokens`, context observations, and `turn_sequence`, `turn_input_tokens`
and `turn_output_tokens`. REST `/api/v1/agents/{id}/usage` also returns optional
`current_turn` with its durable turn sequence and observed input/output. A client
must match that sequence to the running turn before displaying a current-turn
counter. Values advance only when the provider reports usage, usually at model
response completion; no token counts are inferred from streamed text. The older
`usage` event remains readable for retained runtime compatibility.
