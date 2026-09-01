# CMA (ChronoAI Managed Agent)

Managed agent service by ChronoAI. This file is the contract for anyone — human or agent — working in this repository. Rules here are binding, not advisory.

---

## 1. What CMA is

A cloud-hosted service that runs coding-agent kernels (Codex, Claude Code, DeepSeek Harness) on ChronoAI infrastructure and exposes them over public interfaces. End users install nothing.

**Topology — two pod roles:**

| Pod | Contains | Responsibility |
|---|---|---|
| **Control plane pod** | Layers 1, 2, 3 | Public interfaces, core session model, kernel adapters. One deployment, horizontally scalable. |
| **Bot pod** | Layer 4 | One sandboxed agent runtime per session. Many, ephemeral, one per active session. |

The control plane never executes agent code. Bot pods never serve public traffic.

**Consumers.** CMA (ChronoAI Managed Agent) is a backend service. Its clients are other products' frontends and backends (NyxID Chat, FKST Chat). CMA's own `web/` frontend is an operational dashboard, the terminal surface, and the **reference client for the priority-1 public interface**. It is not the primary interaction path.

**The chat surface may call no route a third-party client could not call; its conversation wire is the Responses interface, and everything else it does is public REST/AG-UI.** The moment it needs something those public interfaces lack, the answer is to add it to the appropriate public interface, never to add a private route for the first-party client.

That is a stricter rule than the one it replaces, not a relaxation. "Features must not be built there that belong in the API" asked whether a feature *belonged* in the API, which is a judgement somebody makes; this asks whether the API *already projects it*, which is checkable. And the reason for having a first-party client at all is that `acp-kernel`'s stated exit criterion is that a product frontend can build a chat interface on the public interfaces — a claim nobody had tested. A client held to the same constraints as a third party is the only thing that keeps it honest; a client permitted one private route would prove nothing about the one somebody else has to build.

---

## 2. Repository layout

The repository root contains **exactly** these entries. Nothing else may exist at root:

```
.github/     CI workflows and repository automation
api/         backend — a single Rust project (workspace root lives here)
web/         frontend — a single React project
infra/       deployment manifests and Dockerfiles
docs/        all documentation, markdown only
sdk/         repository-only TypeScript and Rust client SDKs
CLAUDE.md
README.md
.gitignore
.dockerignore
```

Adding any other root-level file or directory is a CI failure. Tooling config that would normally sit at root (`Cargo.toml`, `Cargo.lock`, `deny.toml`, `rust-toolchain.toml`, `package.json`, `tsconfig.json`, lockfiles) lives inside `api/`, `web/`, or the SDK it governs.

`sdk/` contains repository-only client SDKs. Each immediate subdirectory is one
self-contained package, with its own manifest, toolchain configuration, lockfile,
source, tests, and package documentation; it does not join the service project
under `api/` or `web/`. Neither SDK is published to an external package registry:
consumers use the checked-out source, while local package and scratch-consumer
checks prove the exported surfaces without creating a distribution channel.

### `infra/` — fixed contents

`infra/` contains these files and only these files:

```
infra/web/Dockerfile
infra/web/deployment.yaml
infra/web/configmap.yaml
infra/web/secret.yaml
infra/web/service.yaml
infra/web/gcpbackendpolicy.yaml
infra/api/Dockerfile
infra/api/deployment.yaml
infra/api/configmap.yaml
infra/api/secret.yaml
infra/api/service.yaml
infra/api/serviceaccount.yaml
infra/api/secretproviderclass.yaml
infra/api/httproute.yaml
infra/api/healthcheckpolicy.yaml
infra/api/gcpbackendpolicy.yaml
infra/runtime/Dockerfile
```

No additional manifests, no Helm charts, no kustomize overlays.

`infra/web/gcpbackendpolicy.yaml` was added after a missing backend timeout was
measured rather than predicted. The browser still uses one origin, but the
managed Gateway now routes its more-specific `/api` and `/auth` paths plus the
two root-level discovery documents directly to `cma-api`; this preserves the
load balancer's authoritative client chain and puts streams under the API
policy. The web image keeps the same-origin nginx proxy as a fallback when it
is reached without that route split, so the web policy remains explicit too.
Without the applicable policy the balancer's thirty-second default cuts a live
event stream while it is healthy. A client can replay durable frames, but the
message being typed and the agent's live reasoning are deliberately not
durable and disappear on each cut.

The api-side five exist because the control plane depends on them by name. It
runs as a named service account, receives the sandbox key through a named
secret-provider class, and answers on named hostnames. Leaving those applied by
hand meant the deployment had hard dependencies with no source of truth, which
is the divergence §4 forbids — a manifest referencing a service account that no
file declares cannot be validated, and an address the code is configured with
could drift from the route actually serving it.

Their cloud-side halves are still not here and cannot be: the IAM grant letting
the service account read the secret, and the gateway the routes attach to, are
infrastructure this repository consumes rather than owns.

`infra/runtime/` is an image and nothing else. It carries no `deployment.yaml`, no `service.yaml` and no config, and that absence is the point: the bot pod is scheduled by the sandbox provider, not by this repository. CMA hands the provider an image URI and an entrypoint and gets an isolated runtime back. A Deployment here would mean the control plane was scheduling agent workloads itself, which §1 forbids.

### `docs/`

All documentation lives in `docs/`, and every file in it is markdown. The only documentation outside it is this file, `README.md`, and markdown under `sdk/*/docs/`; SDK documentation lives beside its implementation and is the source rendered by the frontend.

---

## 3. The four-layer architecture

CMA is a four-layer system. **Source files are placed under their layer's directory.** The rule is: seeing a file's path must tell you which layer it belongs to, with no further reading.

```
api/crates/
  layer1-public/     Layer 1 — public interfaces          (control plane pod)
  layer2-core/       Layer 2 — core session model + events (control plane pod)
  layer3-adapter/    Layer 3 — ACP-shaped kernel adapters  (control plane pod)
  layer4-runtime/    Layer 4 — sandboxed agent runtime     (bot pod)
  cma/               binary — wires layers 1-3 into the control plane
```

### Layer 1 — public interfaces (control plane pod)

Six interface types, in priority order. Each exposes semantic operations expressed in that protocol's own idiom:

| Priority | Interface | Client | Directory |
|---|---|---|---|
| 1 | **AG-UI** | frontend clients | `layer1-public/src/agui/` |
| 2 | **Responses** (standard OpenAI Responses API) | frontend chat clients | `layer1-public/src/responses/` |
| 3 | **REST** | backend server clients | `layer1-public/src/rest/` |
| 4 | **PTY** | CLI-like streaming clients | `layer1-public/src/pty/` |
| 5 | **MCP** | agent clients | `layer1-public/src/mcp/` |
| 6 | **A2A** | agent clients | `layer1-public/src/a2a/` |

Each interface is a **projection** of the same underlying core session model. Interfaces may expose a *subset* of core operations where the protocol cannot express one — but they must never invent an operation that does not exist in Layer 2, and they must never carry business logic.

### Layer 2 — the session core: models and events (control plane pod)

The single tunnel. Every Layer 1 interface talks to this and only this. It owns:

- the session model and its lifecycle states
- the durable, append-only event log with a per-session monotonic sequence
- what a session was granted, and what it was refused
- approval and permission state
- usage and metering state

Layer 2 is protocol-agnostic, kernel-agnostic and terminal-agnostic. **No protocol name and no kernel name may appear in Layer 2 identifiers, types, or comments**, and no machinery belonging to a domain a layer below owns. If you find yourself writing `agui_` or `codex_` here, the abstraction is wrong; if you find yourself parsing an escape sequence here, the code is in the wrong crate.

Layer 2 is the superset: it models the union of what all interfaces and all kernels need, never the intersection.

#### Capabilities: what a session can do never changes quietly

Two facts have to be told apart everywhere in this service: **"this session can never do that"** and **"that just failed"**. Only the second is worth retrying; only the first is worth telling somebody at creation; a caller who cannot distinguish them does neither well.

So:

- A kernel **advertises** what it can do — hold a conversation, render a terminal, be carried onto a replacement runtime, accept input mid-turn, be interrupted, report usage. Statically, because these describe the kernel rather than the moment.
- A session carries a **granted set**, decided when it is created and recorded rather than recomputed. It is the intersection of what its kernel can do with what its mode needs.
- That set changes **only by an explicit, recorded act of its owner** — moving a session between planes, which is a rotation and is written to its log. Never by a recomputation, never as a side effect, and never without the owner having asked. A caller who reads a session's granted set once and acts on it is right until the owner changes it themselves.
- A request outside that set fails **typed and named** — the ability it lacks, in words a caller can act on — before anything is provisioned, and never as a 404 and never by silently doing nothing.

**A capability is claimed only after it has been demonstrated against the pinned kernel version.** Every one currently advertised was established by driving the real thing; "the documentation says so" is not evidence. A capability claimed and absent is discovered by whoever tried to use it, which is the failure this whole model exists to remove.

### Layer 3 — the Agent Client Protocol (control plane pod)

Abstracts the agent runtime kernel so Layer 2 does not know or care which kernel is running.

**The contract is the Agent Client Protocol**, not a shape resembling it. Its types are ACP's own, re-exported under this project's names, so a conversation crossing this boundary is already in a vendor-neutral vocabulary rather than translated into one. What the protocol does not define, this layer adds beside it and keeps separate: the runtime lifecycle a hosted sandbox needs, and the terminal plane ACP has no word for.

```
layer3-adapter/src/
  lib.rs            the contract — the trait and its errors
  conversation.rs   ACP's own vocabulary, re-exported
  runtime.rs        what ACP does not define: provisioning, geometry, handles
  hosted/           running any kernel in a provider-managed sandbox
  terminal.rs       folding a runtime's bytes into a screen
  codex/            Codex
  claude_code/      Claude Code
  dsh/              DeepSeek Harness
```

**The abstraction and its implementations are separate.** The trait file must contain no kernel-specific type, constant, or conditional. Adding a kernel means adding a directory, never editing the trait to accommodate it.

**A kernel's directory holds what is true of that kernel and nothing else** — the program its pod runs, where it keeps its conversation, its private words for how much its agent may do unattended, and what it can be relied on to do. Everything about *being hosted* — bringing a sandbox up, the link back, the wire the two ends settle on, the credential that rides on it — is shared, because none of it changes with the agent inside.

A kernel need not speak ACP itself. One does not, and translating its wire is its own directory's work; nothing above Layer 3 knows a third protocol exists.

### Layer 4 — the agent runtimes (bot pod)

The actual agent process plus the ChronoAI-authored supervisor that hosts it, relays its terminal, carries its conversation, and stands between it and every credential it would otherwise hold.

Three kernels run here, each behind its own bridge: **Codex**, **Claude Code**, and the **DeepSeek Harness**.

**The supervisor is kernel-neutral and must stay that way.** It is told what to run, where that program keeps its conversation, and what to write before starting it. It knows none of those things itself, and it reads none of what it carries: a conversation crosses as opaque bytes, deliberately, so a kernel changing its format cannot break the pod hosting it.

Layer 4 runs in the bot pod and is a **separate binary**. It is not linked into the control plane.

### Dependency rule — enforced in CI

```
layer1-public  ──►  layer2-core  ──►  layer3-adapter
layer4-runtime  (independent; shares only wire-protocol types)
```

- Layer 1 **must not** depend on Layer 3.
- Layer 2 **must not** depend on Layer 1.
- Layer 3 **must not** depend on Layer 1 or Layer 2.
- Nothing in the control plane may depend on `layer4-runtime`.

A dependency edge not in this list is a CI failure, not a review comment.

---

## 4. `infra/` must never diverge from the code

**Any change to `web/` or `api/` that alters deployment surface must update `infra/` in the same pull request.** Deployment surface means: environment variables read, config keys consumed, secret names required, ports listened on, health endpoints served, resource needs, image build inputs.

Diverged configuration between source and manifests is never acceptable, including temporarily. A PR that adds a config key without adding it to the relevant `configmap.yaml` is incomplete and must be rejected.

Secrets are **referenced** in `secret.yaml`, never valued. No real secret value enters this repository under any circumstance.

---

## 5. A session in progress is never interrupted

**No deployment, upgrade, restart or rollout may interrupt a session its owner has not stopped.** This is a hard rule and outranks every convenience below it. A session that stops working because CMA was upgraded is a defect of the same severity as losing the data.

What "not interrupted" means, precisely:

- The session keeps its identifier, its history, its configuration and its place in the owner's list.
- Everything it has shown remains visible. A session that returns blank has lost the work as far as its owner can tell.
- Work resumes where it left off — the agent continues its conversation rather than starting a new one.

### The database is the only authority

Which sessions should be running is recorded, never remembered. A control plane process holds no state that matters: on boot, and continuously thereafter, it reads the record and makes reality match it — taking back over the runtimes that are still alive, replacing the ones that are gone, and reclaiming the ones nothing is entitled to.

A control plane that has just started has therefore no privileged knowledge, and needs none. This is what makes replacing it safe.

### Runtimes are versioned, and rotated to match

The control plane knows which runtime version it expects. Any session running an older one is **rotated**: its agent transcript is snapshotted to chrono-storage, a new pod is brought up on the current image, and the agent resumes from that snapshot.

Rotation is preferred over upgrading a pod in place, because the runtime image changes too and a rotation exercises the same path a lost sandbox does.

Rotation is not silent:

- The record tracks the runtime version each session is on, every rotation that has happened, and the snapshot each one resumed from.
- The snapshot carries a timestamp, and that timestamp is shown to the owner: they are told the point their session is being carried forward from.
- While a rotation is in progress the session is not interactive, and the interface says so. Accepting input that would be lost with the outgoing pod is worse than briefly refusing it.

Never require the owner to do anything to keep a session alive. "Click revive after the deploy" is not an acceptable design.

## 6. Branches

| Branch | Purpose | Rules |
|---|---|---|
| `develop` | daily development; **default branch** | protected; no force push; accepts PRs from any branch |
| `main` | production; each merge is a release | protected; no force push; accepts PRs **only** from `develop` |

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
- Docker build for all three images — `api`, `web`, `runtime`
- Image vulnerability scan
- Manifest validation — `kubeconform`, so malformed YAML or a bad `apiVersion` fails in CI rather than at `kubectl apply`
- Manifest policy — `kube-linter`: resource requests and limits set, liveness and readiness probes present, no `:latest` tag, non-root user, read-only root filesystem
- Repository structure — root contains only the allowed entries; `infra/` contains only the fixed file list; layer dependency rule holds

### Additional gates on a PR from `develop` into `main`

Everything above, plus:

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

- The three images are built, scanned and pushed to `ghcr.io/chronoaiproject/cma-{api,web,runtime}`, tagged with the version and with the short commit SHA. The intended contract is one assignment per tag, backed by an audited exclusive-writer ACL and package history. GHCR exposes no tag compare-and-swap, so stale absence reads on a sequential rerun remain a residual overwrite risk; matching labels and platform shape are validation, not provenance.
- After all three current GHCR digests pass the writer/history verification, each exact root digest is exported as
  an OCI archive plus JSON metadata in a one-day Actions transfer artifact.
  These unencrypted archives are an additional, ephemeral distribution
  surface: anyone who can download this internal repository's Actions
  artifacts can receive the private images, and that audience is not assumed
  to equal the GHCR package ACL. Retention limits duration, not readership.
  The artifacts are not release assets, are never committed, and contain no
  SDK package; both SDKs remain repository-only and unpublished.
- An annotated tag `vX.Y.Z` and a GitHub release are created. An active, GET-verified `v*` tag ruleset must reject updates and deletions without a bypass. The release carries the notes under their version heading, the image references and their GHCR package links, and the source archives GitHub generates for the tag.

The tag is the version of record. `api/Cargo.toml` and `web/package.json` are not bumped: a bot writing to `main` would put it ahead of `develop` for no gain, and the version those files carry is not what anything deploys.

Publishing is not deploying. `infra/` is untouched by a release, and a new image becomes live only when someone changes a manifest to name it. `CHRONO_SANDBOX_IMAGE` is the sharpest case: under §5 it is the runtime version every live session is measured against, so moving it rotates every session, and that has to be a decision rather than a consequence of merging.

Full procedure in `docs/Release.md`.

---

## 7. Frontend design authority

`docs/Web_Style.md` is the **single source of truth** for all frontend design and style. No colour, spacing value, type scale, component pattern, or interaction rule may be introduced in `web/` that is not in that document.

The document carries a changelog at the end recording every change. **The changelog is append-only — no entry may ever be edited or deleted.**

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
- **Never claim an ability that has not been demonstrated.** A capability, a resume path, a wire method: run it against the pinned version before advertising it. Where something cannot be done, declare the absence — a session refused by name at creation is a session somebody can plan around, and one that accepts a request and silently drops it is not.
- Never add a root-level file to satisfy a tool; move the tool's config into `api/` or `web/`.
- Never weaken a CI gate to make a build pass. If a gate is wrong, change the gate deliberately in its own PR with a stated reason.
- Never add a gitleaks allowlist entry. Change the offending string.
- When a change touches deployment surface, update `infra/` in the same commit series.
- Prefer many small PRs into `develop` over one large one.
