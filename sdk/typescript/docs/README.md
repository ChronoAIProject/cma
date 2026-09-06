# CMA TypeScript SDK

<!-- nav: 1 Overview -->

This repository-only SDK exposes CMA's public Profiles, Marketplace, Sandboxes,
Workspaces, Worktrees and Agents REST resources. It includes runtime-validated
wire DTOs, owner-scoped reads, durable histories and explicit idempotent commands.
It is not published to a package registry.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });
const deployment = await cma.readConformance();
console.log(deployment.service, deployment.status, deployment.interfaces);
```

The client works in browsers and Node.js environments that provide `fetch`.
No runtime dependency or browser global is required; inject `fetch` when the
host does not supply one. Conversation streaming, Triggers and authentication
resource helpers are not part of this REST resource slice.
