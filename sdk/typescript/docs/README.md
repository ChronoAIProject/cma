# CMA TypeScript SDK

<!-- nav: 1 Overview -->

This repository-only SDK exposes CMA's public Profiles, Marketplace, Sandboxes,
Workspaces, Worktrees, Agents, Trigger authoring, visitor Launches and operator
REST resources, browser authentication helpers, plus AG-UI and Responses Agent conversations. It includes validated
resource DTOs, owner-scoped reads, durable histories and explicit commands.
It is not published to a package registry.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });
const deployment = await cma.readConformance();
console.log(deployment.service, deployment.status, deployment.interfaces);
```

The client works in modern browsers and Node.js environments with `fetch`,
Web Streams and AbortSignal support. Inject `fetch` when the host does not supply
one. SSE framing uses the pinned `eventsource-parser` runtime dependency; the SDK
does not require a DOM or an interactive sign-in page.

Use `triggerAuthoring` for publisher-owned configuration, `launches` for explicit
visitor admission and recorded progress, and `agui` for an existing Agent's
conversation. Previewing a Trigger or replaying an Agent never submits work.
The SDK does not provide a terminal plane or create a separate Chat resource.
The optional `@chronoai/cma-sdk/react` entry exports `CmaTriggerButton`, an
SSR-safe anchor and image. Only that entry requires the React peer dependency;
ordinary backend clients do not load React.

## Guides

- [Quickstart](quickstart.md): configure a client and inspect existing resources.
- [Authentication](authentication.md): browser identity and backend credentials.
- [Sandboxes](sandboxes.md): pinned environments and their hierarchy.
- [Agents](agents.md): conversations, commands and owner-scoped projections.
- [Profiles](profiles.md): drafts, immutable revisions and Marketplace entries.
- [Streaming](streaming.md): acknowledgements, replay and interruption recovery.
- [Triggers](triggers.md): publishing, embeds and explicit visitor admission.
- [Operations](operations.md): operator controls and deployment discovery.
- [Errors](errors.md): named refusals and uncertain command outcomes.
- [Reference](reference.md): complete helper inventory and wire contracts.

### Latest retained usage views

The latest-view operation (`SandboxMetrics.latest` in TypeScript,
`UsageMetrics::latest` in Rust) accepts a rolling `24h`, `7d`, or `30d` window,
or `custom` with exact `from`/`to`, plus canonical usage filters. It returns
`report`, `generated_at`, `stale`, `requested_range` and independent refresh
state. Render an authorized ready summary immediately while any replacement
runs; read pages/charts using that exact report ID. Keep at most one observation
in flight per page and honor Retry-After. The operation does not automatically
poll, retry, or reuse caller authority.

Use the explicit compatibility predicate (`isUsageViewUnavailable` in
TypeScript, `view_unavailable` in Rust) only for a disabled feature or missing
route. Typed authority/resource refusals, malformed successes and general 503s
must not invoke an old-endpoint fallback. Retained view writers and warming are
separately disabled until the owner approves their staged activation.
