# CMA (ChronoAI Managed Agent)

[![Try it](https://bot.chrono-ai.fun/api/v1/triggers/public/pub_d77caf24c76f4ce188e569a83c71b8a1/badge.svg?label=try_it&theme=auto&placement=plc_08e81ca7da574d86a8addfb61d550ecd)](https://bot.chrono-ai.fun/api/v1/triggers/public/pub_d77caf24c76f4ce188e569a83c71b8a1/open/plc_08e81ca7da574d86a8addfb61d550ecd)

CMA is a cloud-hosted platform for running AI Agents on ChronoAI infrastructure.
It manages their working environments, conversations, approvals, service access
and recovery. Codex is the one supported agent provider; CMA is the platform
around the Agent, not an AI model. End users work through the web client or
public APIs and install nothing locally.

Read [CLAUDE.md](./CLAUDE.md) before contributing. Its repository layout,
four-layer architecture, ownership rules and CI gates are binding.

## What You Can Do

- **Work with Agents:** create an Agent, exchange messages, follow its work and
  respond to approvals through Agent Manager.
- **Configure its environment:** use the system image or an authorized Image
  Manager version, with inline model, reasoning, permission, prompt and skill
  settings. Profiles remain private immutable configuration records and
  compatibility/recovery surfaces, not a user-facing marketplace workflow.
- **Work on a repository:** optionally supply a GitHub repository. New Agent
  creation starts with an empty workspace and passes the repository in the
  first prompt for the Agent to prepare through the GitHub API workflow.
- **Connect external channels:** use Event Gateway for incoming events, routing
  and reply delivery, separate from Agent execution.
- **Share a starting point:** publish a Trigger link that lets a visitor
  explicitly start an Agent of their own, never enter the publisher's resources.

See [service boundaries](./docs/CMA_Service_Split.md),
[GitHub source and publication workflow](./docs/CMA_GitHub_API_Workflow.md), and
[deployment readiness](./docs/CMA_Deployment_Readiness.md). Source code and local
tests alone do not establish that a capability is deployed or provider-verified.

## Architecture

### Services

The unified web client has three portals, backed by separately deployable
application services:

| Service | Responsibility |
|---|---|
| `cma-agent-manager` | Agents, messages, turns, approvals, workspaces, runtime bindings, persistent homes and recovery. |
| `cma-event-gateway` | Channels, sealed credentials, incoming event normalization, routing and reply delivery. |
| `cma-image-manager` | Image catalog, builds, immutable versions, sharing and image lifecycle. |

Each service owns its permissions, records and migrations. Agent Manager uses
the infrastructure-owned Kubernetes `agent-sandbox` controller to provision
runtimes; there is no separate CMA Sandbox Manager service. See
[CMA Service Split](./docs/CMA_Service_Split.md) and
[NyxID, schema, and frontend boundaries](./docs/CMA_Service_Isolation.md).

### Agent Environments

```text
Sandbox                  isolated runtime and resource boundary
  Workspace              project directory
    Worktree             isolated Git working tree
      Agent              exactly one Agent per Worktree
```

A Sandbox can contain multiple Workspaces and Agents. Agents work in their
own Worktrees, not in the Workspace's main checkout. Configuration and skills
live in per-Agent homes, not globally.

The backend keeps public interfaces, domain logic, provider integration and
runtime execution in four layers:

```text
Control plane pod
  layer1-public    REST, AG-UI, Responses; explicit MCP/A2A refusal
  layer2-core      Sandbox -> Workspace -> Worktree -> Agent, profiles, events
  layer3-adapter   SandboxAdapter, runtime hosting, Codex conventions
Sandbox pod
  layer4-runtime   provider-neutral supervisor, Workspaces and Agents
```

The control plane never executes agent code; Sandbox pods never serve public
traffic. Software installation finishes before credentials are delivered.

### Public Interfaces

Resource management and Trigger/Launch operations use public REST. Live
conversations use AG-UI (`threadId` is the Agent id) or Responses (`conversation`
is the Agent id). The first-party conversation surface uses the same public
interfaces available to other clients. There is no terminal plane, separate
Chat model, billing, sponsorship or repository-contribution workflow.

Start with the [API overview](./docs/API_Overview.md),
[REST contract](./docs/API_REST.md) and
[public API inventory](./docs/CMA_Public_APIs.md). The
[Agent Manager API proposal](./docs/CMA_Agent_Manager_API.md) includes target
contracts and explicitly identifies capabilities that are not yet implemented;
it is not a blanket statement of current API availability.

## Repository

| Path | Contents |
|---|---|
| `api/` | Rust workspace: four layers, service implementations and production composition. |
| `web/` | React/TypeScript client: Agent Manager, Event Gateway, Image Manager, Trigger journeys and Operations. |
| `sdk/` | Repository-only TypeScript and Rust SDKs, tests and bundled guides; unpublished. |
| `infra/` | Fixed Dockerfile and Kubernetes manifest inventory; kept aligned with code. |
| `docs/` | Public contracts, design authority, operations and evidence. |

The backend uses Axum/Tokio and MongoDB; the frontend uses React, TypeScript and
Vite. Dockerfiles and Kubernetes manifests live in `infra/`.

The Kubernetes workload and canonical Service names use these service names.
Legacy DNS aliases `cma-api` and `cmaeg` preserve existing sealed ChannelPointer
authorities and retained internal callers during deployment. The image repository
names `cma-api`/`cma-cmaeg`, source directories and credential service identities
remain compatible; stored pointers are not rewritten by the rename.

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

A publisher selects an image, inline Agent settings and an optional GitHub
workspace. CMA records the immutable target and returns a stable public URL
and host-specific integration snippets. Historical profile-based targets retain
their recorded reuse policy for replay. For example:

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
[CMA Event Gateway](./docs/CMAEG_Protocol.md). CMA owns provider bindings, environments
and Agent conversations; the former in-CMA Bridge execution pipeline is retired.
