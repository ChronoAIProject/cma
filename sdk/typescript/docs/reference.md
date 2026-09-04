# Reference

<!-- nav: 5 Reference -->

`createClient(config)` returns a `Cma`. Its supported high-level method is
`readConformance(options?)`, which performs `GET /api/v1/conformance`
anonymously.

```ts
import { type Conformance, readConformance, resolveConfig } from '@chronoai/cma-sdk';

const config = resolveConfig({ baseUrl: 'https://cma.example.com' });
const deployment: Conformance = await readConformance(config, {
  requestId: 'deployment-check-1',
});
```

`Conformance` contains `service`, `status`, and an array of currently exposed
`interfaces`. Generic transport helpers are exported for diagnostics and SDK
development, but they do not define a resource contract.
