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
