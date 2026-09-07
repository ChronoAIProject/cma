# CMA (ChronoAI Managed Agent)

[![Run with CMA](https://bot.chrono-ai.fun/api/v1/triggers/public/pub_b4a26689ed1a47f7889345d6212bc155/badge.svg?label=run_with_cma&theme=dark&placement=plc_4d84d1127d6a40a0a71ee3545eae3bf5)](https://bot.chrono-ai.fun/t/pub_b4a26689ed1a47f7889345d6212bc155/plc_4d84d1127d6a40a0a71ee3545eae3bf5)

Managed agent service by ChronoAI. Runs coding-agent kernels (Codex, Claude Code, DeepSeek Harness) on ChronoAI
infrastructure and exposes them over public interfaces, so end users install nothing locally.

**Read [CLAUDE.md](./CLAUDE.md) before contributing.** It defines the repository layout, the
four-layer architecture, the branch model, and the CI gates. Those rules are binding.

## Layout

| Path | Contents |
|---|---|
| `api/` | Backend — a single Rust workspace. Layers 1–4. |
| `web/` | Frontend — a single React project. Interactive session, Trigger, contribution, publishing, and terminal surfaces. |
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

## CMA GitHub Triggers

A repository publisher can create an immutable CMA Trigger and place the
generated link in a public README:

```markdown
[Open in CMA](https://cma.example.com/triggers/PUBLIC_TRIGGER_ID)
```

The link contains no API key or GitHub credential. It opens a read-only preview;
the contributor signs in, selects their own NyxID GitHub connection, reviews or
adjusts the permitted settings, and explicitly presses Start before CMA creates
or reuses a fork and opens a managed session. Publishing a branch and opening or
updating a pull request remain separate contributor confirmations.

See [Publish and use a CMA GitHub Trigger](./docs/CMA_Trigger_Publishing.md) for
the publisher and contributor flow, scope requirements, revision lifecycle,
re-click behavior, failure handling, and MVP boundaries. The API contract is in
[API_REST.md](./docs/API_REST.md), and operational recovery is in
[CMA_Trigger_Operations.md](./docs/CMA_Trigger_Operations.md).

## Branches

`develop` is the default branch and takes daily work. `main` is production — every merge into it
is a release. Both are protected. See CLAUDE.md for the full gate list.
