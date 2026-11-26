# CMA (ChronoAI Managed Agent)

Managed agent service by ChronoAI. This file is the contract for anyone — human or agent — working in this repository. Rules here are binding, not advisory.

---

## 1. What CMA is

A cloud-hosted service that runs agents on ChronoAI infrastructure and exposes
their environment and conversations through public interfaces. End users install
nothing. The one supported agent provider is Codex; additional providers require
their own Layer 3 directory and evidence against pinned versions.

**Topology: two pod roles.**

| Pod | Contains | Responsibility |
|---|---|---|
| **Control plane pod** | Layers 1, 2, 3 | Public interfaces, the sandbox model, orchestration, and the sandbox adapter. Staged, ownership-qualified two-replica floor with rolling overlap; operation budgets and visitor quotas are shared through MongoDB. Activation follows private maintainer guidance. |
| **Sandbox pod** | Layer 4 | One isolated CMA Sandbox containing Workspaces, Worktrees, and Agents. Many, provisioned by the sandbox provider. |

The control plane never executes agent code. Sandbox pods never serve public
traffic. A Sandbox is not an Agent: multiple Agents may work concurrently in
distinct Worktrees inside one Sandbox.

**Consumers.** CMA is a backend service for product frontends and backends.
Its own `web/` frontend contains the Agent Manager, Image Manager and Event
Gateway portals, operational dashboard, Trigger publisher, public visitor
journey, and reference client for the public interfaces. The main working
surface is one Agent conversation in its Worktree, Workspace, and Sandbox.
There is no terminal plane, separate Chat model, sponsorship, or contribution-specific workflow.
Sandbox allocation usage is recorded at the lifecycle persistence boundary and
exposed through public usage APIs. Pricing and invoices remain in NyxID’s existing
billing provider; CMA does not maintain an independent price or wallet ledger.

**The conversation surface may call no route a third-party client could not
call.** Instance and inline configuration management use public REST; live Agent
conversations use public AG-UI, and the public Responses interface projects the same Agent
conversation for compatible clients. A missing first-party capability must be
added to the appropriate public interface, never a private route.

New Agent tabs may select public Responses v2 input and rich Agent replay through
the authenticated transport flag. Existing tabs retain their selected transport
through flag changes and reloads; uncertain requests keep their original keys.

A CMA Trigger brings a visitor to an Agent of their own, never the publisher's.
GET, navigation, and an OAuth callback do not start work. An explicit visitor
action supplies consent and admits the durable operation.

---

## 2. Repository layout

The repository root contains **exactly** these entries. Nothing else may exist at root:

```
.github/     CI workflows and repository automation
api/         backend — a single Rust project (workspace root lives here)
web/         frontend — a single React project
CLAUDE.md
AGENTS.md
CONTRIBUTING.md
README.md
LICENSE
NOTICE
.gitignore
.dockerignore
```

Adding any other root-level file or directory is a CI failure. Explicit operator stage patches live in `.github/agent-log-stages/`; their approval
packet is CMA Agent-log activation (private maintainer reference). They are
operator inputs, not additional deployed resources or automatic CI actions.

Tooling config that would normally sit at root (`Cargo.toml`, `Cargo.lock`, `deny.toml`, `rust-toolchain.toml`, `package.json`, `tsconfig.json`, lockfiles) lives inside `api/`, `web/`, as appropriate.

### 2.1 Terminology

The maintainer's definitions below are authoritative. Every resource, API path
and UI noun uses these words. "Session" is retired except for the existing
`GET /auth/session` route naming the browser sign-in session and vocabulary
required by an external protocol.

**1. CMA Environment**
- 1.1 **CMA Sandbox** — the sandbox (pod) CMA starts for a user on Kubernetes.
- 1.2 **CMA Workspace** — a directory within a CMA Sandbox; one sandbox can have multiple workspaces.
- 1.3 **CMA Worktree** — a git worktree opened in a CMA Workspace; one workspace can have multiple worktrees.
- 1.4 **CMA Agent** — the agent (e.g. a codex process) opened in one worktree; one worktree has one and only one agent.

**2. CMA Environment Config (internal persistence)**
Users select an image and inline Agent settings. Profiles are private immutable
configuration records, not a user creation or marketplace concept. Historical
profile APIs and records remain compatibility/recovery surfaces; they must not
be reintroduced into the normal portal journey.

- 2.1 **CMA Agent Profile**: model name; thinking effort; permission level; agent skills (Ornn skills, installed at the worktree/agent level in the provider's own convention, never sandbox-global); agent system prompt.
- 2.2 **CMA Workspace Profile**: workspace path — a public GitHub repository URL (default branch cloned to `/home/cma/<repo>`) or a custom directory name (`/home/cma/<name>`, git-initialised); the agent profiles for this workspace (several, one per agent/worktree).
- 2.3 **CMA Sandbox Profile**: cpu (millicores); memory (MiB); NyxID service scope list (the scoped agent key, available to the whole sandbox); **agent provider** (codex; claude code and dsh disabled) and **model access type** (ChronoAI-managed LLM or the user's own credentials) — placed here because both are properties of the pod's home; maximum workspaces; maximum agents per workspace; the workspace profiles for this sandbox; software installation commands (global to the sandbox); environment variables (global).

**3. Mutability.** A Sandbox pins its immutable environment at provisioning.
Owners add Workspaces from inline empty-directory or GitHub configuration and
Agents from inline settings within its recorded limits. Existing-workspace
Agent creation retains the parent image and service scope. Image Manager owns
image builds, immutable versions and private/shared/public visibility.

**4. Interaction.** A user always interacts with one agent, in one worktree, in one workspace, in one sandbox. The research use case is exactly: an agent in a worktree of `/home/cma/trureturing` (cloned from `https://github.com/the-omega-institute/trureturing`) inside a sandbox with the required software installed.

**5. CMA Trigger.** An external button that brings the clicking user to an agent
**of their own** (never the trigger owner's). New authoring selects an image,
inline Agent settings and optional GitHub workspace. The backend stores the
immutable target under a trigger id and gives the owner integration snippets
for web pages and Markdown. The owner lists permitted embedding origins.
Historical profile-based targets retain their recorded reuse policy for replay.

### Public repository boundary

Only application source, tests, component build recipes, and intentionally public
product assets belong here. Internal documentation, runbooks, verification
records, notes, plans and investigations are not committed to this public repository.
Do not create a root documentation directory or copy private material into another
tracked directory. Keep private references outside GitHub.

The web application owns its internal API client under `web/src/api-client`.
There are no separately distributed SDK packages in this repository.

---

## 3. The four-layer architecture

CMA is a four-layer system. **Source files are placed under their layer's
directory.** Seeing a file's path must tell you which layer owns it.

```
api/crates/
  layer1-public/     Layer 1: public interface projections (control plane pod)
  layer2-core/       Layer 2: sandbox model and events     (control plane pod)
  layer3-adapter/    Layer 3: sandbox adapter              (control plane pod)
  layer4-runtime/    Layer 4: sandbox supervisor           (sandbox pod)
  cma/               binary: composition, persistence and production adapters
  cmaeg-core/        pure CMA Event Gateway protocol library
  cmaeg/             CMA Event Gateway service binary
  cma-image-manager/ independent image build, catalog and sharing service
  cma-service-auth/  technical NyxID transport with per-service sessions
```

### Layer 1: public interfaces

Interfaces expose semantic operations in their protocol's own idiom:

| Priority | Interface | Client | Directory |
|---|---|---|---|
| 1 | **AG-UI** | frontend conversation clients; `threadId` is the Agent id | `layer1-public/src/agui/` |
| 2 | **Responses** | compatible conversation clients; `conversation` is the Agent id | `layer1-public/src/responses/` |
| 3 | **REST** | resource management, profiles, marketplace, Triggers and Launches | `layer1-public/src/rest/` |
| 4 | **PTY** | absent in this release; no terminal plane | `layer1-public/src/pty/` (typed refusal only) |
| 5 | **MCP** | Limited runtime capabilities propose owner service consent and publish/update their own document artifacts; broader resource operations remain unsupported | `layer1-public/src/mcp/` |
| 6 | **A2A** | public provider card; JSON-RPC explicitly unsupported, no invented Agent tasks | `layer1-public/src/a2a/` |

Each interface is a **projection** of the same core model. It may expose a
subset of core operations, but must never invent an operation absent from
Layer 2 or carry business logic. Refusals are explicit, typed, and named.

### Layer 2: the sandbox model

The single tunnel. Every Layer 1 interface talks to Layer 2 and only Layer 2.
It owns:

- Sandbox -> Workspace -> Worktree -> Agent aggregates, lifecycle states,
  ownership, limits, durable orchestration and recovery.
- Agent, Workspace and Sandbox configuration snapshots, plus retained Profile
  revisions and trust needed by historical operations.
- Trigger revisions, placements, Launch consent, reuse decisions, connection
  requirements and durable progress.
- Append-only Sandbox and Agent event logs, per-object monotonic sequences,
  approval state, usage, idempotency and optimistic concurrency.
- Platform defaults, count-based quotas, operator settings and audit events.
- User-owned document artifacts, immutable source revisions, explicit sharing,
  comments and bounded runtime-authorized publication. Document ownership is
  independent of the originating Agent's lifecycle.

Layer 2 is protocol-agnostic and provider-agnostic. **No protocol name or
provider implementation name belongs in Layer 2 identifiers, types, or
comments.** A domain provider selection is data; the provider's process,
filesystem conventions and wire vocabulary belong below this layer.
The core models the union of required domain operations, never only the
intersection conveniently exposed by today's interfaces.

A Sandbox pins its complete Sandbox Profile at provisioning. Its service scope,
provider, model access, resources, software and environment do not change
implicitly. The owner may add Workspaces and Agents with inline configuration
within the recorded limits. Published profile changes never mutate existing instances.

An explicit owner service-access command may replace the exact NyxID scope on
an Active Sandbox without restarting its runtime. The latest owner-authorized
scope revision controls new credential issuance while the original profile and
creation scope remain immutable. Credential delivery and old-key revocation
complete before confirmed callback delivery to the original Agent; an active turn
receives durable steering without waiting for that turn to finish. Owners may add or remove optional
services; the platform baseline remains required. Already admitted paused access
workflows remain recoverable. Profile publication never changes instance access.

Retained Sandbox Profile availability remains a mutable inactivity policy,
separate from immutable environments. Always keep alive prevents inactivity
pause; explicit manual pause remains available. Normal portal authoring does
not expose legacy Profile policies.

New warm capacity uses native agent-sandbox pools. Durable targets live in
`native_warm_pool_targets`, pin the deployment runtime image and original Pod
policy, and use leases and revisions. Each owned Sandbox persists its selected
pool, original claim/member identities and adopted home before subsequent
effects. Cleanup proves claim, member and Pod finality before releasing a disk.
Legacy `warm_sandboxes` records remain for already-claimed runtime recovery and
cleanup only; production no longer admits or prepares new Profile warmups.

New spare native capacity uses only the configured runtime image and current
home/Pod policy. Operators cannot re-enable an obsolete target, but may configure
multiple resource sizes for the current image. Release cleanup drains obsolete
spares only after release authority, a durable admission barrier and fresh complete
allocation/Claim checks exclude pending adoption. A settled retained home,
including a failed home, does not require spare capacity; its exact original
attachment must still be proved, or its original Claim/Sandbox/PVC graph proved
fully absent. Its history remains intact. Definitions remain until the separate
full retirement check proves no retained home depends on them. Pool cleanup never
rotates or deletes a claimed Sandbox; startup alone is not allocation finality.
Upgraded homes may outlive their original definitions only after exact retained
Claim/Sandbox/PVC and policy verification proves they can recover independently.
Retired target records preserve immutable release order and cannot be activated
again by an older process. API/Web rollback retains the latest released runtime
default; it does not downgrade `CHRONO_SANDBOX_IMAGE`.

#### Capabilities must be proved

"This can never do that" and "that just failed" are separate typed facts.
Unsupported abilities are refused before provisioning; transient failure is
reported with its operation identity and a supported recovery path.

A runtime advertises only abilities demonstrated against its pinned version.
The control plane admits the exact supported runtime contract before using it.
A new image name is not evidence of a capability, and a deployment does not
silently recompute an instance's authority.

### Layer 3: the sandbox adapter

`SandboxAdapter` abstracts runtime provisioning, install, credentials,
Workspaces, Worktrees, Agent homes, conversations, checkpoints and restore.
Its request types are neutral. Conversation types use the Agent Client
Protocol's own vocabulary; hosting operations sit beside that contract.

```
layer3-adapter/src/
  lib.rs            public contract exports
  adapter.rs        SandboxAdapter
  conversation.rs   ACP conversation vocabulary
  runtime.rs        provisioning, install, credentials and runtime handles
  workspace.rs      Workspace and Worktree operations
  agent.rs          Agent home and conversation operations
  checkpoint.rs     writer-fenced checkpoint and restore operations
  hosted/           shared runtime transport and lifecycle
  provider/         sandbox-provider transport
  codex/            the one supported agent provider's conventions
```

**The abstraction and implementation remain separate.** Provider-specific
conditions never enter the adapter trait or shared hosting machinery. Adding a
provider means adding a directory, not teaching the supervisor or core its name.
Unsupported providers have no selectable runtime or frontend path.

### Layer 4: the sandbox supervisor

The runtime is a separate binary, not linked into the control plane. It runs as
the sandbox user `cma`, with home `/home/cma`, and supervises:

- One-time Sandbox software installation as `cma`, before any credential is
  delivered or the credential-bearing proxy starts.
- Credential activation after install; workspace clone or git initialization
  and workspace setup after credential availability.
- Distinct Worktrees and Agent homes, with per-Agent skills and configuration
  supplied by Layer 3. Agents never execute in the main checkout.
- Conversation bridges, operation receipts, writer fencing, whole-home
  checkpoint/restore, and resource observations.

The supervisor is provider-neutral. It receives process and home plans and
carries conversation data without interpreting a provider's transcript format.
The current runtime contract is version **3**, identified by the image label
`dev.chrono.cma.runtime-contract`; unsupported or missing versions are refused.

### Dependency rule: enforced in CI

```
layer1-public -> layer2-core -> layer3-adapter
layer4-runtime (independent; shares only wire-protocol types)
```

- Layer 1 must not depend on Layer 3.
- Layer 2 must not depend on Layer 1.
- Layer 3 must not depend on Layer 1 or Layer 2.
- Nothing in the control plane may depend on `layer4-runtime`.

A dependency edge not in this list is a CI failure, not a review comment.

### Service extraction in progress

The revised authorized split is Event Gateway, Agent Manager and Image Manager,
specified in private maintainer guidance. There is no CMA Sandbox Manager.
Agent Manager integrates directly with infra-owned agent-sandbox CRDs/controllers,
including resources, persistent mounts and exact CMA Sandbox/Pod UID authority.
It never adopts, pauses or reclaims another application's workloads.

Image Manager owns builds, catalog versions, sharing and image lifecycle in
the configured image registry, using its own
Google identity. The service names are `cma-agent-manager`,
`cma-event-gateway` and `cma-image-manager`. Each has independent NyxID
permissions, schema and migrations. The current authorized deployment shares the
existing the configured service database database credential through explicit `shared-schema` mode;
service stores still access only their named collections. Default `isolated` mode
requires separate collection-scoped credentials. No cross-service collection
access or domain/store implementation dependency is allowed. The old local `cma-sandbox-manager`
foundation is superseded and must not become a fourth deployment.

Agent Manager retains Agent homes, models, prompts, ACP and runtime/conversation
recovery. New Agent creation uses the deployment runtime and an empty workspace;
an optional GitHub repository is passed in its first prompt for the Agent to set
up. New defaults are gpt-6, high reasoning and autonomous permissions, with
chrono-llm-public and ornn-api always included in the key scope. Historical
admitted lifecycle operations retain their recorded setup behavior and defaults.
Existing OpenSandbox resources retain their old
provider authority until verified migration. New crates alone do not declare the
split or deployment complete; live continuity remains mandatory.

### CMAEG crates

CMAEG lives in this repository: `api/crates/cmaeg-core` (pure protocol library,
depends on no other crate of this workspace) and `api/crates/cmaeg` (the service
binary, depends only on `cmaeg-core`, the technical `cma-service-auth` and
`cma-allocator-qualification` libraries, and third-party crates). The std-only
allocator qualification library is also used by CMA; it owns no service or
domain state and exercises each executable's selected allocator. It is a separate
deployment with privately maintained manifests.
Nothing in `layer1-public`, `layer2-core`, `layer3-adapter`, `layer4-runtime` or
`cma` may depend on the CMAEG crates, and CMAEG crates may not depend on them;
CI enforces both directions.

CMAEG owns event ingress, channel configuration and delivery; CMA exposes the
provider binding contract and owner Bot management. Every admitted Activity uses
the same provider turn interface. Gateway code must not interpret a product
workflow, special-case an agent binding or reserve product-specific commands.
Bot creation guidance and business operations belong to the agent skill and CMA.
The legacy in-CMA Bridge execution pipeline and its public routes have been
retired. Persisted history readers retain the original schemas and never
authorize new legacy work.

`api/crates/cmaeg/assets/docs/public/CMAEG_Protocol.md` is the normative CMAEG protocol authority. Changes under
`/v1` must be additive: retained operations, fields, enum values and operation IDs
remain compatible. A non-additive change requires `/v2`. The reviewed baseline
`api/crates/cmaeg/openapi/v1.json` is a required exception to the generated-artifact
rule in §8: regenerate it, review the diff against this rule, and commit it with
the implementation and protocol documentation.

The reviewed SVG diagrams under `api/crates/cmaeg/assets/docs` are another
required exception to the generated-artifact rule: they are bundled source
assets for the public document, which must render without browser JavaScript
or external services. Their Mermaid sources remain in the protocol and
architecture Markdown; regenerate and review the SVGs with
`.github/scripts/render-cmaeg-diagrams.py`. CI verifies source freshness.

---

## 4. Infrastructure stays outside GitHub

Never commit infrastructure or deployment manifests, Kubernetes or Helm YAML,
cloud project, cluster, IAM or service-account identifiers, internal hostnames
or IPs, Terraform state, or environment-specific configuration to this public
repository. Infrastructure lives outside GitHub. The ignored `infra/`, `sdk/`
and `docs/` root paths must stay untracked.

Build recipes live only at `web/Dockerfile`, `api/crates/cma/Dockerfile`,
`api/crates/cmaeg/Dockerfile`, `api/crates/cma-image-manager/Dockerfile`, and
`api/crates/layer4-runtime/Dockerfile`. Their build context remains the repository
root. Use build arguments for environment-specific image inputs; never encode
private registry or project identities in a recipe. Synthetic deployment data
is allowed only in explicit test-fixture directories and must contain no real
identifiers or credentials.

When code changes deployment requirements, communicate them privately to the
release owner. Update the source configuration validation and its tests here;
update deployment configuration outside this repository before rollout.

---

## 5. A sandbox in progress is never interrupted

**No deployment, upgrade, restart or rollout may interrupt a Sandbox its owner
has not stopped.** This rule outranks convenience. Work that disappears because
CMA was upgraded is a data-loss defect.

- The Sandbox, its Workspaces, Worktrees and Agents keep their identifiers,
  configuration, history and place in the owner's hierarchy.
- Everything already shown remains visible. A returning blank conversation has
  lost work as far as its owner can tell.
- Agents resume from their recorded conversations, not new conversations that
  merely look similar.

### The database is the only authority

Desired state, admitted operations, claims and progress are recorded, never
remembered only by a control-plane process. At boot and continuously, the
controller takes back over live runtimes, replaces lost ones, recovers durable
operations and reclaims resources no record owns. Reconciliation is owner- and
writer-fenced; an expired worker must not mutate a newer worker's resources.

Quiet Sandboxes pause according to platform settings after every Agent is quiet,
unless their Profile has Always keep alive enabled. Inactivity admission fences
the mutable Profile policy in the same transaction as its Sandbox/Agent evidence.
A platform spend stop (`SpendStop`) is the one exception: it also pauses
Sandboxes kept alive and Active Sandboxes without live Agents, never busy work,
does not change the preference, and defers managed runtime updates until it
lifts.
Any admitted message, creation or Launch that needs one revives it. The owner
does not need to repair a deployment by clicking a revive control.

### Runtime identity and persistent homes

Existing live Sandboxes retain their recorded image across a runtime release.
Reconciliation adopts each retained runtime using that image. New managed
allocations use the released default for their lane under §6. API/web upgrade
and rollback preserve the existing runtime, native conversation, admitted turns,
approvals and dirty work. An incompatible feature stays disabled until retained
runtime coexistence and uninterrupted interaction are proved.

Managed native homes support two explicit update policies: the owner's Sandbox
Details update command, and a daily check at 02:00 Asia/Singapore. The first
observation of a default runtime waits until the next daily boundary; deployment
or restart does not authorize immediate replacement. A requested update pins
its exact target and waits for all admitted Agent and Workspace work to settle.
It suspends the native Sandbox, changes its image and generation, and reopens
the original conversations on the same PVC/home. IDs and original pool/claim/home
authority remain unchanged. An initially paused Sandbox returns to paused unless
new owner work supersedes that intent. Failures retain their exact phase and
pinned target for retry; a recovery stalled at conversation reopen on a
superseded target is retried on the latest runtime instead. Custom-image
Sandboxes are excluded.

An owner/control/message/creation revival from Paused uses the latest managed
runtime on that same retained native Sandbox and home. Already admitted updates
retain their pinned target; RuntimeLost alone never upgrades an otherwise
unchanged runtime. Native pause/resume changes operating mode and binds the
replacement Pod with a new boot identity. A pooled runtime retains its original
claim and member authority across that transition.
Deletion waits for the claim, Sandbox and every owned Pod to disappear before
releasing its PVC and completing database cleanup.

Production CMA no longer manages S3 checkpoint storage or runs scheduled or
manual archive capture. Historical checkpoint schemas and provider bindings
remain readable for recovery; archive-only paused resources require an explicit
external migration. The initialized VolumeSnapshot remains a prerequisite only
for retained legacy snapshot-seeded homes; new native homes use controller-owned
empty volumes and the pinned runtime initializer. See
private maintainer guidance for activation, acceptance and rollback.

Lifecycle transitions are never silent. The record names the runtime version
and progress. The UI preserves history and explains why
input is temporarily unavailable. It must not accept input that could be lost
with an outgoing runtime, invent progress from elapsed time, or require the
owner to manually restart an Agent after deployment.

## 6. Branches

| Branch | Purpose | Rules |
|---|---|---|
| `develop` | daily development; **default branch** | protected; no force push; accepts PRs from any branch |
| `main` | production; each merge is a release | protected; no force push; accepts PRs **only** from `develop` or from a `release/<name>` branch |
| `release/<name>` | one frozen release candidate | cut from a commit on `develop`'s first-parent history; never receives commits of its own |

### CI gates on every PR into `develop`

**Backend** (`api/`)
- Format — `cargo fmt --all --check`
- Lint — `cargo clippy --all-targets --all-features -- -D warnings`
- Tests — `cargo nextest run`, plus `cargo test --doc`
- Build with `--locked` so a stale `Cargo.lock` fails loudly
- Dependency audit — `cargo deny check` (advisories, licenses, banned crates)

**Frontend** (`web/`)
- Format — Prettier (or Biome for format + lint in one pass)
- Lint — ESLint with `--max-warnings=0`
- Type check — `tsc --noEmit`, as its own gate; the bundler strips types without checking them
- Tests — Vitest
- Production build — `npm run build`; build failures and type failures are different things
- Dependency audit — `npm audit --audit-level=high`
- Lockfile integrity — `npm ci`, never `npm install`

**Cross-cutting**
- Secret scanning — **gitleaks**. No credential, secret, or sensitive value may exist anywhere in the repository, including tests and fixtures. **Nothing may be allowlisted or bypassed.** If a test string trips gitleaks, change the string — never add an exception.
- Docker build for all five images — `api`, `web`, `runtime`, `cmaeg`, `runtime-omarchy`
- Image vulnerability scan
- Repository structure — root contains only the allowed entries; the public-tree guard enforces private-path and manifest exclusions; layer dependency rule holds

### Additional gates on a PR from `develop` into `main`

A release PR's head is `develop` or a `release/<name>` branch. A release branch freezes one `develop` commit, so the release candidate stops moving while the merge queue keeps advancing `develop`; CI and the release gate then run once on exactly what is released. Its head must be a commit on `develop`'s first-parent history, with no commits of its own, and it must contain `main`. A fix goes to `develop` first, and a new release branch is cut from it.

Everything above, plus:

- The PR is merged with a merge commit, never squashed or rebased. Production CD treats the merge's second parent as the released `develop` commit and refuses a merge whose tree differs from it.
- Exactly one of the labels `release:major`, `release:minor`, `release:patch`. The merge derives the version from it; a PR that declares nothing cannot be released.
- A mandatory `## Release Notes` section in the PR body, in exactly this shape:

```markdown
## Release Notes

### What's New
- ...

### What's Fixed
- ...

### What's Changed
- ...
```

All three subsections, spelled and capitalised as above, in that order, each carrying at least one bullet, and nothing but bullets underneath. Anything else is rejected — these notes are the only description a release carries, and they are read by people who were not in the review, so what they contain is not left to whoever is writing them that day.

**No version number appears in the PR.** The version is derived from the release label and the previous tag, and the publisher writes it into the notes as `## Release Notes vX.Y.Z`. A number typed by an author would be a guess at what the merge decides.

That section, and only that section, becomes the annotated tag message and the release notes. It ends at the next `##` heading, so anything addressed to the reviewer belongs outside it and is never published.

`.github/release-notes.sh` both validates and extracts it, and is run by the gate before the merge and by the publisher after it — one script, so the notes that ship are the notes that were reviewed.

### What a merge into `main` produces

The merge is the release. Nothing is tagged or published before it, and nothing further is required after it:

- Nothing is rebuilt. For each of the five images, `ghcr.io/chronoaiproject/cma-{agent-manager,web,runtime,event-gateway,runtime-omarchy}`, the release selects the newest published component version whose source is the merged `develop` commit or one of its ancestors on `develop`'s first-parent history, and only if staging deployment evidence recorded that exact digest as controller-verified. For the components production deploys this is the selection `prod-cd.yml` makes; the runtime images are recorded the same way but not deployed by it. Each image is therefore built and scanned once, by the reviewed staging publication gate, and a release holding an image staging never verified is refused.
- Each selected digest receives the release merge's short commit SHA tag and then the version tag through a digest-preserving copy; the image still records the `develop` commit it was built from. The runtime image contract runs again against the promoted runtime digest before any tag is assigned, and no version tag is written until all five short-SHA tags exist, so a release that fails earlier claims no version. The intended contract is one assignment per tag, backed by an audited exclusive-writer ACL and package history. A tag that already names the selected digest is reused and one that names anything else refuses the release. GHCR exposes no tag compare-and-swap, so stale absence reads on a sequential rerun remain a residual overwrite risk. No image is exported as an Actions artifact.
- An annotated tag `vX.Y.Z` and a GitHub release are created. An active, GET-verified `v*` tag ruleset must reject updates and deletions without a bypass. The release carries the notes under their version heading, the image references and their GHCR package links, and the source archives GitHub generates for the tag.

The tag is the version of record. `api/Cargo.toml` and `web/package.json` are not bumped: a bot writing to `main` would put it ahead of `develop` for no gain, and the version those files carry is not what anything deploys.

Publishing is not deploying. `prod-cd.yml` deploys a release, and only after a required
approval in the `production` environment. Neither it nor the release rebuilds an image:
for each component production enables, it promotes the newest published version whose
source is the merged `develop` commit or one of its ancestors, and only if staging deployment
evidence recorded that exact image as controller-verified. The same deployment helpers and
private controller design as staging do the write. A production release requests its own
one-hour controller budget, because a production rollout takes far longer than staging's,
and the deploy job's timeout outlives it; staging's release and bootstrap budgets are
unchanged. Deployment configuration, including
which components production enables, is maintained privately.
Production selects the application components (`am`, `eg`, `web`) and can select
the Agent runtime after pointer publication is enabled. The runtime must come
from the approved release's history and have an exact staging verification
receipt. The Machine runtime is qualified in staging. Until pointer following
is activated, staging deploys runtime images together with Agent Manager.

For new managed allocations, managed-runtime updates and owner-requested revival
under §5, each lane uses the active digest that Agent Manager has durably
recorded as its released default. `CHRONO_SANDBOX_IMAGE` and
`CHRONO_MACHINE_SANDBOX_IMAGE` bootstrap empty Agent and Machine Included lanes,
respectively, and supply the fixed defaults in deployments without durable lane
authority. After pointer following is activated, the CD runtime lanes advance
their own environment's pointers to verified, finalized digests. Agent Manager
verifies the digest named by its environment's pointer, prewarms the matching
native pool, and promotes that digest only after the pool is ready. Freezing
following preserves the active digest. Existing live Sandboxes retain their
image until an admitted update reaches idle. Deployment alone never starts
immediate image-drift rotation. Persisted update requests require a compatible
reader during rollback; older binaries that reject the new lifecycle metadata
cannot safely be restored after admission.

Full procedure in private maintainer guidance.

---

## 7. Frontend design authority

the shared tokens and components in `web/src/styles` and `web/src/ui` is the **single source of truth** for all frontend design and style. No colour, spacing value, type scale, component pattern, or interaction rule may be introduced in `web/` that is not in that document.

Keep component contracts and their tests aligned with deliberate design changes.

---

## 8. Commits

- **Small and frequent, not bulky and infrequent.** One logical change per commit.
- **No `Co-Authored-By` trailers.** Not for agents, not for pairs, not for anyone.
- Commit messages state what changed and why, in the imperative mood.
- Never commit a secret. Never commit a generated artifact that CI can produce.

---

## 9. Working rules for agents

- Read this file before making structural changes. It governs.
- **Each layer explicitly owns its scope.** Code that is semantically foreign to the layer it sits in is moved to where it belongs — not tolerated, not wrapped, not left with a comment explaining itself. A kernel's name in the supervisor, a protocol's vocabulary in the core, an escape sequence anywhere above Layer 3: each is a defect of the same kind, and each is fixed by moving the code rather than by justifying it. This is checked deliberately, not assumed.
- Never place a source file outside its layer's directory to make an import easier — fix the design instead.
- **Never claim an ability that has not been demonstrated.** A capability, a resume path, a wire method: run it against the pinned version before advertising it. Where something cannot be done, declare the absence — a Sandbox or Agent refused by name at creation is something a caller can plan around, and one that accepts a request and silently drops it is not.
- Never add a root-level file to satisfy a tool; move the tool's config into `api/` or `web/`.
- Never weaken a CI gate to make a build pass. If a gate is wrong, change the gate deliberately in its own PR with a stated reason.
- Never add a gitleaks allowlist entry. Change the offending string.
- Communicate deployment-surface changes privately; keep deployment inputs outside GitHub.
- Prefer many small PRs into `develop` over one large one.
