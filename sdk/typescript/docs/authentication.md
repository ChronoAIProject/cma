# Authentication

<!-- nav: 3 Authentication -->

The currently supported discovery request is deliberately anonymous. It sends
neither the configured bearer credential nor browser cookies.

The transport configuration still accepts a rotation-aware credential callback
so callers do not need to restructure client construction when authenticated
resource methods return in issue #819:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({
  baseUrl: 'https://cma.example.com',
  cookies: false,
  credential: () => process.env.CMA_ACCESS_TOKEN,
});

await cma.readConformance(); // The callback is not invoked for this public read.
```

Never place a credential in `baseUrl`; URLs containing user information are
rejected.
