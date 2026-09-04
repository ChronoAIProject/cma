# CMA TypeScript SDK

<!-- nav: 1 Overview -->

The temporary SDK reads the public state of a CMA deployment while its resource
interfaces are rebuilt. It does not expose an old or speculative resource
model.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });
const deployment = await cma.readConformance();
console.log(deployment.service, deployment.status, deployment.interfaces);
```

The client works in browsers and Node.js environments that provide `fetch`.
