# Authentication

<!-- nav: 3 Authentication -->

Resource methods use browser cookies by default. Backend callers can disable
cookies and provide a rotation-aware credential callback; it is resolved for
each HTTP request. The callback should read from the host's credential store,
never from a URL or committed constant:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({
  baseUrl: 'https://cma.example.com',
  cookies: false,
  credential: () => process.env.CMA_ACCESS_TOKEN,
});

await cma.agents.list();
```

Never place a credential in `baseUrl`; URLs containing user information are
rejected.

`readConformance` remains deliberately anonymous: it sends neither bearer
credentials nor cookies and does not invoke the credential callback. The SDK
does not perform an interactive sign-in, create API keys or store credentials.
