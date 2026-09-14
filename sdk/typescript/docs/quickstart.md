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

Read a public Trigger without admitting any work:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const preview = await cma.launches.preview('public-trigger-id');
console.log(preview.title, preview.what_happens, preview.availability);
```

Only an explicit visitor gesture may call `launches.createOrResume`. A successful
response is a durable record, not a guarantee that an Agent is ready. A Draft may
need trust or account review; accepted work must be followed until its recorded
resolution supplies the visitor's Agent id. See [Reference](reference.md) for
admission, connections and AG-UI examples.
