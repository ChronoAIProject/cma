# Managed Agent model selection

The Agent Details model list comes from the current Chrono LLM catalogue through
the Sandbox's delivered NyxID credential. The public Agent endpoint checks owner
lineage before fetching the list. Listing models does not expand service scope.

The current upstream catalogue contains text and image models without modality
metadata. CMA admits catalogue IDs matching `gpt-<numeric generation>` with
letters, digits, dots, underscores or hyphens, plus `codex-auto-review`. It does
not offer `gpt-image-*` for an Agent conversation. Catalogue requests explicitly
identify the CMA client and accept JSON. Upstream failure bodies are not exposed.

The pinned ACP bridge keeps Codex's native model metadata unchanged. When a
managed model absent from that metadata is selected, the bridge verifies its ID
against a fresh catalogue read through the configured loopback model proxy.
Only an HTTP URL on `127.0.0.1` is eligible; redirects, user information, query
strings and fragments are refused. The read has a five-second timeout, a 1 MiB
body limit and a 1,000-entry limit. Neither the credential nor upstream error
content enters bridge diagnostics.

A verified additional alias uses the same native unknown-model fallback as the
default `gpt-6`: its own instructions and tools remain Codex's responsibility.
The ACP selection record supplies the existing fallback reasoning levels and
modalities without borrowing another model's context window or fast mode.
Default startup and changes to native models do not wait on this catalogue read.
A failed catalogue check leaves the current model in place.

RuntimeStatus advertises `agent_model_catalog` for this capability. Older
retained runtimes omit it and are read as unsupported; an Agent Manager upgrade
does not replace those runtimes. Deploy the compatible Agent Manager before
activating the new runtime image because older control planes reject unknown
RuntimeStatus fields. The runtime image build checks this capability together
with its other bootstrap and restart evidence.

The exact-source ACP patch and pinned Codex 0.147.0 test prove selection of
`gpt-6-astra` and `gpt-6.1-sol`, actual model names in inference requests,
reasoning changes, unchanged fallback instructions/tools, rejected absent and
image-only IDs, refusal after loss of catalogue access, and conversation reload.
These are local provider-contract proofs; production catalogue and settings
acceptance must still be performed after the new runtime image is deployed.
