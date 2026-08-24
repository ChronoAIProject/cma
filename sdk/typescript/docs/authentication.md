# TypeScript Authentication

<!-- nav: 30 Authentication -->

CMA accepts either the deployment's browser session cookie or an HTTP bearer
credential. The SDK does not mint credentials and never persists one. How a user
obtains a credential is deployment-specific; when issuance is not self-service,
ask that deployment's operator or identity administrator.

## Backend bearer credentials

Supply a callback, not a captured header. The callback is invoked for every unary
request attempt and every event-stream reconnect, so replacing the value in the
credential store takes effect without rebuilding the client.

```ts
import { createClient } from '@chronoai/cma-sdk';

const credentialStore: { current?: string } = {};
const cma = createClient({
  baseUrl: 'https://cma.example.com',
  credential: () => credentialStore.current,
  cookies: false,
});

credentialStore.current = process.env.CMA_ACCESS_TOKEN;
void cma;
```

Returning `undefined` sends no Authorization header. Returning an empty string or
throwing from the callback produces `CredentialError` before the request is sent.
The SDK never includes credential values in its own error messages.

## Browser sessions

The reference frontend is served on the same origin as its API proxy. It relies on
the browser cookie and therefore uses the defaults:

```ts
import { createClient } from '@chronoai/cma-sdk';

const sameOriginCma = createClient();
const sessions = await sameOriginCma.listSessions();

console.log(sessions.length);
```

For cross-origin bearer use, configure `baseUrl`, `credential`, and
`cookies: false`. The client supports that transport shape, but the deployment
must also permit the Authorization header through CORS. The current service
contract has an open CORS gap, so use a same-origin browser proxy or a backend
client until the deployment explicitly demonstrates cross-origin bearer support.

## Reading authentication failures

- HTTP `401` becomes `ApiError` with `isUnauthenticated === true`. Refresh or
  replace the credential, then make a new request.
- `CapabilityRefused` is not an authentication failure. The caller was identified,
  but this session was never granted the named operation. Retrying cannot change it.
- HTTP `403` is an authorization refusal. A new credential helps only if it carries
  a different grant.

The complete service authentication contract is in
[REST Authentication](../../../docs/API_REST.md#authentication). See
[Errors](errors.md) for the SDK taxonomy.
