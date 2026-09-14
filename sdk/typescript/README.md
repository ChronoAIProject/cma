# CMA TypeScript SDK

Repository-only TypeScript client for CMA's public Profiles, Marketplace,
Sandboxes, Workspaces, Worktrees and Agents REST resources. Wire DTOs retain
their public snake_case names and are validated at runtime. Every mutation
requires an explicit caller-owned idempotency key.

The package is private and is not published to a registry. Consumers use the
checked-out source or a locally built package. Streaming, Triggers and auth
resource helpers are separate forthcoming contract slices.

```sh
npm ci
npm run build
npm test
npm run docs:check
```

The guides live in [`docs/`](docs/README.md).
