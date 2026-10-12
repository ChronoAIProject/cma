# CMA (ChronoAI Managed Agent)

[![Try it](https://bot.chrono-ai.fun/api/v1/triggers/public/pub_d77caf24c76f4ce188e569a83c71b8a1/badge.svg?label=try_it&theme=auto&placement=plc_08e81ca7da574d86a8addfb61d550ecd)](https://bot.chrono-ai.fun/api/v1/triggers/public/pub_d77caf24c76f4ce188e569a83c71b8a1/open/plc_08e81ca7da574d86a8addfb61d550ecd)

CMA runs managed Agents on ChronoAI infrastructure. A user's Sandbox contains
Workspaces; each Workspace contains isolated git Worktrees, each with exactly
one Agent. Versioned Sandbox, Workspace and Agent Profiles define the environment
and can be published through the marketplace. Codex is the one supported agent
provider. End users install nothing locally.

Read [CLAUDE.md](./CLAUDE.md) before contributing. Its repository layout,
four-layer architecture, ownership rules and CI gates are binding.

## Public Interfaces

Resource management, profiles, marketplace, Triggers and Launches use public
REST. Live conversations use AG-UI (`threadId` is the Agent id) or Responses
(`conversation` is the Agent id). The first-party web client uses those same
public interfaces. There is no terminal plane, separate Chat model, billing,
sponsorship or repository-contribution workflow.

```text
Control plane pod
  layer1-public    REST, AG-UI, Responses; explicit MCP/A2A refusal
  layer2-core      Sandbox -> Workspace -> Worktree -> Agent, profiles, events
  layer3-adapter   SandboxAdapter, shared hosting, Codex conventions
Sandbox pod
  layer4-runtime   provider-neutral supervisor, Workspaces and Agents
```

The control plane never executes agent code; Sandbox pods never serve public
traffic. Software installation finishes before credentials are delivered.
Agent configuration and skills live in per-Agent homes, not globally.

## Repository

| Path | Contents |
|---|---|
| `api/` | Rust workspace: four layers and production composition. |
| `web/` | React client: hierarchy, conversations, Profiles, Marketplace, Trigger journeys and Operations. |
| `sdk/` | Repository-only TypeScript and Rust SDKs, tests and bundled guides; unpublished. |
| `infra/` | Fixed Dockerfile and Kubernetes manifest inventory; kept aligned with code. |
| `docs/` | Public contracts, design authority, operations and evidence. |

The proposed three-service refactor is recorded in
[CMA Service Split](./docs/CMA_Service_Split.md), with separate
[NyxID, database, and frontend boundaries](./docs/CMA_Service_Isolation.md),
[Agent Manager API](./docs/CMA_Agent_Manager_API.md), and
[Sandbox Manager API](./docs/CMA_Sandbox_Manager_API.md).
These are design drafts; the architecture and interfaces described above remain
the currently implemented system.

## Local Development

Backend commands run from `api/`; frontend commands run from `web/`. The
frontend imports the checked-out TypeScript SDK, so install its runtime
dependencies too:

```sh
npm ci --prefix sdk/typescript --omit=dev --ignore-scripts
npm ci --prefix web
npm run dev --prefix web
```

The backend requires the documented configuration and a MongoDB replica set for
transactions. See [CMA Operations](./docs/CMA_Operations.md) before starting a
production-backed configuration. Tests using local substitutes are not evidence
that the deployed provider or credentials work.

## CMA Triggers

A publisher selects exact Sandbox, Workspace and Agent Profile revisions and
their reuse policies. CMA returns a stable public URL and host-specific
integration snippets. For example:

```markdown
[Open in CMA](https://cma.example.com/t/PUBLIC_TRIGGER_ID)
```

The link contains no credential. It opens a read-only landing; the visitor
explicitly starts an Agent of their own, completes any trust or connection
review, and follows recorded preparation. A GET or OAuth callback never starts
work. Reuse always concerns the visitor's resources, never the publisher's.

See [CMA Model](./docs/CMA_Model.md), [CMA Trigger](./docs/CMA_Trigger.md),
[API Overview](./docs/API_Overview.md), [REST API](./docs/API_REST.md), and the
[complete public API list](./docs/CMA_Public_APIs.md).

For the separate event gateway, see [architecture and integration](./docs/CMAEG_Architecture.md),
[protocol](./docs/CMAEG_Protocol.md), and [operations](./docs/CMAEG_Operations.md).

## Branches And Deployment

`develop` is the protected default development branch. Complete implementation
and all required gates before merging. The general release policy is in
[CLAUDE.md](./CLAUDE.md); the milestone-16 deployment is an explicitly authorized
exception: merge completed work to `develop`, build and deploy directly, and
verify production without merging to `main` or invoking the release workflow.
The exact reset/deployment order and safeguards belong to
[CMA Operations](./docs/CMA_Operations.md).

Event ingress and channel delivery belong to the separate
[CMA Event Gateway](CMAEG_Protocol.md). CMA owns provider bindings, environments
and Agent conversations; the former in-CMA Bridge execution pipeline is retired.
