# chrono-bot

Managed agent service by ChronoAI. This file is the contract for anyone — human or agent — working in this repository. Rules here are binding, not advisory.

---

## 1. What chrono-bot is

A cloud-hosted service that runs coding-agent kernels (Codex, Claude Code) on ChronoAI infrastructure and exposes them over public interfaces. End users install nothing.

**Topology — two pod roles:**

| Pod | Contains | Responsibility |
|---|---|---|
| **Control plane pod** | Layers 1, 2, 3 | Public interfaces, core session model, kernel adapters. One deployment, horizontally scalable. |
| **Bot pod** | Layer 4 | One sandboxed agent runtime per session. Many, ephemeral, one per active session. |

The control plane never executes agent code. Bot pods never serve public traffic.

**Consumers.** chrono-bot is a backend service. Its clients are other products' frontends and backends (NyxID Chat, FKST Chat). chrono-bot's own `web/` frontend is a read-only operational dashboard plus the terminal surface — it is not the primary interaction path, and features must not be built there that belong in the API.

---

## 2. Repository layout

The repository root contains **exactly** these entries. Nothing else may exist at root:

```
.github/     CI workflows and repository automation
api/         backend — a single Rust project (workspace root lives here)
web/         frontend — a single React project
infra/       deployment manifests and Dockerfiles
docs/        all documentation, markdown only
CLAUDE.md
README.md
.gitignore
```

Adding any other root-level file or directory is a CI failure. Tooling config that would normally sit at root (`Cargo.toml`, `Cargo.lock`, `deny.toml`, `rust-toolchain.toml`, `package.json`, `tsconfig.json`, lockfiles) lives inside `api/` or `web/` accordingly.

### `infra/` — fixed contents

`infra/` contains these files and only these files:

```
infra/web/Dockerfile
infra/web/deployment.yaml
infra/web/configmap.yaml
infra/web/secret.yaml
infra/web/service.yaml
infra/api/Dockerfile
infra/api/deployment.yaml
infra/api/configmap.yaml
infra/api/secret.yaml
infra/api/service.yaml
infra/api/serviceaccount.yaml
infra/api/secretproviderclass.yaml
infra/api/httproute.yaml
infra/runtime/Dockerfile
```

No additional manifests, no Helm charts, no kustomize overlays.

The last three exist because the control plane depends on them by name. It
runs as a named service account, receives the sandbox key through a named
secret-provider class, and answers on named hostnames. Leaving those applied by
hand meant the deployment had hard dependencies with no source of truth, which
is the divergence §4 forbids — a manifest referencing a service account that no
file declares cannot be validated, and an address the code is configured with
could drift from the route actually serving it.

Their cloud-side halves are still not here and cannot be: the IAM grant letting
the service account read the secret, and the gateway the routes attach to, are
infrastructure this repository consumes rather than owns.

`infra/runtime/` is an image and nothing else. It carries no `deployment.yaml`, no `service.yaml` and no config, and that absence is the point: the bot pod is scheduled by the sandbox provider, not by this repository. chrono-bot hands the provider an image URI and an entrypoint and gets an isolated runtime back. A Deployment here would mean the control plane was scheduling agent workloads itself, which §1 forbids.

### `docs/`

All documentation lives in `docs/`, and every file in it is markdown. No documentation is placed anywhere else in the repository except this file and `README.md`.

---

## 3. The four-layer architecture

chrono-bot is a four-layer system. **Source files are placed under their layer's directory.** The rule is: seeing a file's path must tell you which layer it belongs to, with no further reading.

```
api/crates/
  layer1-public/     Layer 1 — public interfaces          (control plane pod)
  layer2-core/       Layer 2 — core session model + events (control plane pod)
  layer3-adapter/    Layer 3 — ACP-shaped kernel adapters  (control plane pod)
  layer4-runtime/    Layer 4 — sandboxed agent runtime     (bot pod)
  chrono-bot/        binary — wires layers 1-3 into the control plane
```

### Layer 1 — public interfaces (control plane pod)

Five interface types, in priority order. Each exposes semantic operations expressed in that protocol's own idiom:

| Priority | Interface | Client | Directory |
|---|---|---|---|
| 1 | **AG-UI** | frontend clients | `layer1-public/src/agui/` |
| 2 | **REST** | backend server clients | `layer1-public/src/rest/` |
| 3 | **PTY** | CLI-like streaming clients | `layer1-public/src/pty/` |
| 4 | **MCP** | agent clients | `layer1-public/src/mcp/` |
| 5 | **A2A** | agent clients | `layer1-public/src/a2a/` |

Each interface is a **projection** of the same underlying core session model. Interfaces may expose a *subset* of core operations where the protocol cannot express one — but they must never invent an operation that does not exist in Layer 2, and they must never carry business logic.

### Layer 2 — core session model and events (control plane pod)

The single tunnel. Every Layer 1 interface talks to this and only this. It owns:

- the session model and its lifecycle states
- the durable, append-only event log with a per-session monotonic sequence
- approval and permission state
- usage and metering state

Layer 2 is protocol-agnostic and kernel-agnostic. **No protocol name and no kernel name may appear in Layer 2 identifiers, types, or comments.** If you find yourself writing `agui_` or `codex_` in Layer 2, the abstraction is wrong.

Layer 2 is the superset: it models the union of what all interfaces and all kernels need, never the intersection.

### Layer 3 — ACP-shaped kernel adapters (control plane pod)

Abstracts the agent runtime kernel so Layer 2 does not know or care which kernel is running.

```
layer3-adapter/src/
  lib.rs            the trait and shared types — the abstraction
  codex/            Codex implementation
  claude_code/      Claude Code implementation
```

**The abstraction and its implementations are separate.** The trait file must contain no kernel-specific type, constant, or conditional. Adding a kernel means adding a directory, never editing the trait to accommodate it.

Vocabulary is modelled on the Agent Client Protocol (session, prompt turn, streaming update, tool call, permission request, terminal) so the internal model converges with a vendor-neutral shape rather than any one vendor's.

### Layer 4 — sandboxed agent runtime (bot pod)

The actual agent process — Codex, Claude Code — plus the ChronoAI-authored agent that supervises it and relays its terminal.

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

## 5. Branches

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
- Docker build for both images
- Image vulnerability scan
- Manifest validation — `kubeconform`, so malformed YAML or a bad `apiVersion` fails in CI rather than at `kubectl apply`
- Manifest policy — `kube-linter`: resource requests and limits set, liveness and readiness probes present, no `:latest` tag, non-root user, read-only root filesystem
- Repository structure — root contains only the allowed entries; `infra/` contains only the fixed file list; layer dependency rule holds

### Additional gates on a PR from `develop` into `main`

Everything above, plus:

- A git tag for the release, carrying release notes with exactly three sections: **What's new**, **What's fixed**, **What's changed**.

---

## 6. Frontend design authority

`docs/Web_Style.md` is the **single source of truth** for all frontend design and style. No colour, spacing value, type scale, component pattern, or interaction rule may be introduced in `web/` that is not in that document.

The document carries a changelog at the end recording every change. **The changelog is append-only — no entry may ever be edited or deleted.**

---

## 7. Commits

- **Small and frequent, not bulky and infrequent.** One logical change per commit.
- **No `Co-Authored-By` trailers.** Not for agents, not for pairs, not for anyone.
- Commit messages state what changed and why, in the imperative mood.
- Never commit a secret. Never commit a generated artifact that CI can produce.

---

## 8. Working rules for agents

- Read this file before making structural changes. It governs.
- Never place a source file outside its layer's directory to make an import easier — fix the design instead.
- Never add a root-level file to satisfy a tool; move the tool's config into `api/` or `web/`.
- Never weaken a CI gate to make a build pass. If a gate is wrong, change the gate deliberately in its own PR with a stated reason.
- Never add a gitleaks allowlist entry. Change the offending string.
- When a change touches deployment surface, update `infra/` in the same commit series.
- Prefer many small PRs into `develop` over one large one.
