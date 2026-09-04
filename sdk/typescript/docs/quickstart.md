# Quickstart

<!-- nav: 2 Quickstart -->

Create one client for a deployment root and read its anonymous discovery
document:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com/' });
const deployment = await cma.readConformance({ timeoutMs: 5_000 });

if (deployment.status !== 'rebuilding') {
  console.log(deployment.interfaces);
}
```

Omit `baseUrl` for a same-origin browser deployment. A trailing slash is
normalised, and absolute deployment URLs must use HTTP or HTTPS.
