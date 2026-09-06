# Quickstart

<!-- nav: 2 Quickstart -->

Create one client for a deployment root and list the authenticated owner's
Agents:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com/' });
const agents = await cma.agents.list({ status: 'ready', limit: 20, timeoutMs: 5_000 });
for (const agent of agents.items) {
  console.log(agent.id, agent.workspace_id, agent.profile.revision);
}
```

Omit `baseUrl` for a same-origin browser deployment. A trailing slash is
normalised, and absolute deployment URLs must use HTTP or HTTPS.

Resource values use the public wire's snake_case field names. A page's optional
`next` value is opaque; pass it as `cursor` on the next read. Event histories
instead accept a numeric `after` sequence. A request accepts `signal`,
`timeoutMs` and `requestId`; correlation ids are separate from command keys.
