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

Trigger preview is a public read. Admission requires the visitor's authenticated
identity; Launch reads, progress and connection handoffs are owner-checked.
Publisher methods require the separate Trigger-publishing grant. Operator methods
require their distinct operator grants; exposing a client namespace does not grant
authority to use it.

For a same-origin browser, the application may navigate to `/auth/login` with a
canonical local `return_to`; `screen=register` selects registration when supported.
`/auth/signup-options` supplies current registration availability. These browser
routes are not SDK methods. Preserve an explicit visitor intent across sign-in,
with a bounded lifetime and its exact command key/body. An OAuth callback without
that intent must not create a Launch.

`launches.createConnection` returns an opaque handoff id and a NyxID hosted URL,
never service credentials. Validate HTTPS and reject URL user information before
navigating. Treat the hosted URL as sensitive handoff data: do not log it or put it
in the return destination. Poll `readConnection` with the authenticated owner;
CMA does not inspect cross-origin window contents. Manual connection metadata
does not imply that a hosted handoff is available.
