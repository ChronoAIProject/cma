# CMA (ChronoAI Managed Agent)

[![Try it](https://cma.nyxid.ai/api/v1/triggers/public/pub_d77caf24c76f4ce188e569a83c71b8a1/badge.svg?label=try_it&theme=auto&placement=plc_08e81ca7da574d86a8addfb61d550ecd)](https://cma.nyxid.ai/api/v1/triggers/public/pub_d77caf24c76f4ce188e569a83c71b8a1/open/plc_08e81ca7da574d86a8addfb61d550ecd)

CMA is a cloud-hosted platform for running AI Agents on ChronoAI infrastructure.
It manages their working environments, conversations, approvals, service access
and recovery. Codex is the one supported agent provider; CMA is the platform
around the Agent, not an AI model. End users work through the web client or
public APIs and install nothing locally.

Read [CLAUDE.md](./CLAUDE.md) before contributing. Its repository layout,
four-layer architecture, ownership rules and CI gates are binding.

## What You Can Do

- **Work with Agents:** create an Agent, exchange messages, follow its work and
  respond to approvals through Agent Manager. Responses v2 conversations can
  queue follow-up messages while a turn runs; interrupting or stopping a response
  remains an explicit action. The composer lists commands and skills reported
  by the running Agent rather than assuming every provider command is available.
- **Configure its environment:** use the system image or an authorized Image
  Manager version, with inline model, reasoning, permission, prompt and skill
  settings. Profiles remain private immutable configuration records and
  compatibility/recovery surfaces, not a user-facing marketplace workflow.
- **Work on a repository:** optionally supply a GitHub repository. New Agent
  creation starts with an empty workspace and passes the repository in the
  first prompt for the Agent to prepare through the GitHub API workflow.
- **Manage service access:** an Agent can request missing NyxID services for
  owner approval. Confirmed scope updates deliver replacement credentials to the
  existing Sandbox without restarting it or replacing its conversations; the
  request itself never grants access.
- **Connect external channels:** use Event Gateway for incoming events, routing
  and reply delivery, separate from Agent execution.
- **Share a starting point:** publish a Trigger link that lets a visitor
  explicitly start an Agent of their own, never enter the publisher's resources.

See service boundaries (private maintainer guidance),
GitHub source and publication workflow (private maintainer guidance), and
deployment readiness (private maintainer guidance). Source code and local
tests alone do not establish that a capability is deployed or provider-verified.

## Architecture

### Services

The unified web client has three portals, backed by separately deployable
application services:

| Service | Responsibility |
|---|---|
| `cma-agent-manager` | Agents, messages, turns, approvals, workspaces, runtime bindings, persistent homes and recovery. |
| `cma-event-gateway` | Channels, sealed credentials, incoming event normalization, routing and reply delivery. |

Each service owns its permissions, records and migrations. Agent Manager uses
the infrastructure-owned Kubernetes `agent-sandbox` controller to provision
runtimes; there is no separate CMA Sandbox Manager service. See
CMA Service Split (private maintainer guidance) and
NyxID, schema, and frontend boundaries (private maintainer guidance).

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

New Agent creation uses the deployment runtime and an empty workspace. Repository
setup is a separate Agent task: the runtime's `gh api` uses the authorized
`api-github` service to obtain a pinned source snapshot while preserving existing
files and Git metadata. That API grant does not authenticate native Git transport,
and a source checkout does not imply imported history or working `git pull` /
`git push`.

The backend keeps public interfaces, domain logic, provider integration and
runtime execution in four layers:

```text
Control plane pod
  layer1-public    REST, AG-UI, Responses, limited Agent access-request tool
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

The general public MCP interface remains unsupported. A separate, authenticated
runtime capability exposes only `agent_key_scope_revision` to request owner
consent; Agents must wait for CMA's confirmed credential update before retrying
the service. A2A exposes provider discovery, not executable Agent tasks. See the
MCP contract (private maintainer guidance) and A2A contract (private maintainer guidance) for the
supported boundaries and explicit refusals.

Start with the API overview (private maintainer guidance),
REST contract (private maintainer guidance) and
public API inventory (private maintainer guidance). The
Agent Manager API proposal (private maintainer guidance) includes target
contracts and explicitly identifies capabilities that are not yet implemented;
it is not a blanket statement of current API availability.

## Repository

| Path | Contents |
|---|---|
| `api/` | Rust workspace: four layers, service implementations and production composition. |
| `web/` | React/TypeScript client: Agent Manager, Event Gateway, Trigger journeys and Operations. |

The backend uses Axum/Tokio and MongoDB; the frontend uses React, TypeScript and
Vite. Dockerfiles live beside their owning components. Deployment configuration and internal documentation are maintained outside this repository.

The Kubernetes workload and canonical Service names use these service names.
Legacy DNS aliases `cma-api` and `cmaeg` preserve existing sealed ChannelPointer
authorities and retained internal callers during deployment. The image repository
names `cma-api`/`cma-cmaeg`, source directories and credential service identities
remain compatible; stored pointers are not rewritten by the rename.

## Local Development

These steps are for contributors, not hosted CMA users. Read
[CLAUDE.md](./CLAUDE.md) before changing code and
Web Style (private maintainer guidance) before changing the frontend.

### Prerequisites

- Node.js and npm compatible with the checked-in lockfiles. The main frontend
  CI jobs use Node.js 26; browser harnesses have their own
  pinned versions in the workflows.
- Rust via rustup, using `api/rust-toolchain.toml` for the backend.
- For backend integration, the documented service configuration and a MongoDB
  replica set for transactions. NyxID and Sandbox-provider setup are separate
  prerequisites, not supplied by installing frontend dependencies.

### Frontend

Backend commands run from `api/`; frontend commands run from `web/`. The
frontend owns its API client. Run these commands from the repository root:

```sh
npm ci --prefix web
npm run dev --prefix web
```

The Vite development server runs the frontend only; it does not start the CMA
services or provide authentication. See CMA Operations (private maintainer guidance)
and service isolation (private maintainer guidance) before configuring backend
connections. Do not use production-backed configuration as an implicit local
development default.

To review a page without any backend, run `npm run dev:fixtures --prefix web`.
The dev server then answers `/auth` and `/api` from local fixtures in
`web/build/fixtures/`, which the LOCAL browser checks under `web/e2e/` share.
Coverage grows page by page and currently includes Event Gateway
(`/event-gateway`). A read without a fixture returns a typed `404` and is
logged in the terminal; writes are refused with `501` and nothing is persisted.
Fixture mode never reaches production and is absent from builds and tests.

### Message Task Audit

Agent Manager audits newly admitted message tasks from `2026-11-24T16:00:00Z`
by default. The default has no admission end time and flags unresolved tasks
for review after 3,600 seconds. Restarts use the same cutoff.

Set `CHRONO_TASK_AUDIT_SELECTION` to a JSON object with `from`, optional `to`,
and `overdue_seconds` to select a different cohort. Use canonical UTC timestamps.
The start is inclusive and the end is exclusive. Set the variable to JSON
`null` to disable background sampling. Invalid values fail startup validation.

Run the `am-command-cohort-index-v1` migration before sampling. The sampler
reads the existing index and reports a sample failure if the index is missing
or the query exceeds its 10,000-task, 16 MiB of receipt data, or 10-second limit.
The task audit CLI takes an explicit selection.

### Local Checks

After installing frontend dependencies, run the core frontend checks from the
repository root:

```sh
npm run format:check --prefix web
npm run lint --prefix web
npm run typecheck --prefix web
npm test --prefix web
npm run build --prefix web
```

For the backend, use the pinned Rust toolchain and checked-in lockfile:

```sh
cd api
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo build --workspace --locked
cargo test --doc --workspace --locked
```

Backend integration tests use `cargo-nextest` and additional CI fixtures; the
workflow runs memory-budget tests separately and provisions disposable MongoDB
and browser dependencies. The commands above are not the full merge gate.
[CI](./.github/workflows/ci.yml) also checks dependency audits, secret scanning, repository contracts and images. Local substitutes do not prove deployed provider behavior.

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

See CMA Model (private maintainer guidance), CMA Trigger (private maintainer guidance),
API Overview (private maintainer guidance), REST API (private maintainer guidance), and the
complete public API list (private maintainer guidance).

For the separate event gateway, see [architecture and integration](./api/crates/cmaeg/assets/docs/public/CMAEG_Architecture.md),
[protocol](./api/crates/cmaeg/assets/docs/public/CMAEG_Protocol.md), and operations (private maintainer guidance).

The external [dining review reference application](https://github.com/ChronoAIProject/frigate-cma-events/tree/main/examples/dining)
shows Home Assistant / Frigate metadata aggregated into bounded candidates,
then routed through an Event Channel to an existing CMA Agent for visual review
and a verified Google Sheets write. Its application repository owns the workflow
and deployment templates; personal settings and credentials are supplied
privately at deployment. It uses the existing `cma_codex` provider.

## Branches And Deployment

`develop` is the protected default development branch. Complete implementation
and all required gates before merging. The general release policy is in
[CLAUDE.md](./CLAUDE.md); the milestone-16 deployment is an explicitly authorized
exception: merge completed work to `develop`, build and deploy directly, and
verify production without merging to `main` or invoking the release workflow.
The exact reset/deployment order and safeguards belong to
CMA Operations (private maintainer guidance).

Event ingress and channel delivery belong to the separate
[CMA Event Gateway](./api/crates/cmaeg/assets/docs/public/CMAEG_Protocol.md). CMA owns provider bindings, environments
and Agent conversations; the former in-CMA Bridge execution pipeline is retired.
