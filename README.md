# CMA (ChronoAI Managed Agent)

Managed agent service by ChronoAI. Runs coding-agent kernels (Codex, Claude Code, DeepSeek Harness) on ChronoAI
infrastructure and exposes them over public interfaces, so end users install nothing locally.

**Read [CLAUDE.md](./CLAUDE.md) before contributing.** It defines the repository layout, the
four-layer architecture, the branch model, and the CI gates. Those rules are binding.

## Layout

| Path | Contents |
|---|---|
| `api/` | Backend — a single Rust workspace. Layers 1–4. |
| `web/` | Frontend — a single React project. Read-only dashboard plus the terminal surface. |
| `infra/` | Dockerfiles and Kubernetes manifests. Fixed file list; must never diverge from the code. |
| `docs/` | All documentation, markdown only. |

## Architecture

Two pod roles. The control plane never executes agent code; bot pods never serve public traffic.

```
control plane pod
  layer1-public    AG-UI · REST · PTY · MCP · A2A
  layer2-core      the session core: models, events, capabilities
  layer3-adapter   the Agent Client Protocol, and the kernels behind it
bot pod
  layer4-runtime   kernel-neutral supervisor, and the agent it hosts
```

## Local development

```sh
# backend
cd api && cargo build --workspace --locked && cargo test --workspace

# frontend
cd web && npm ci && npm run dev
```

## Branches

`develop` is the default branch and takes daily work. `main` is production — every merge into it
is a release. Both are protected. See CLAUDE.md for the full gate list.
