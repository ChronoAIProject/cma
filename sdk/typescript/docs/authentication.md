# TypeScript Authentication

<!-- nav: 30 Authentication -->

CMA accepts either the deployment's browser session cookie or an HTTP bearer
credential. The SDK does not mint credentials and never persists one. How a user
obtains a credential is deployment-specific; when issuance is not self-service,
ask that deployment's operator or identity administrator.

## Anonymous deployment discovery

`readConformance` is always anonymous, even when the client is configured with a
bearer credential or browser cookies. It reads the deployment's protocol classes,
version, and exact build without presenting either credential:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({
  baseUrl: 'https://cma.example.com',
  credential: () => process.env.CMA_ACCESS_TOKEN,
});

const deployment = await cma.readConformance();
console.log(deployment.conformsTo, deployment.version, deployment.buildSha);
```

Correlation and response observation remain active for discovery. Public access
means that the request omits authorization and cookies; it does not bypass the
client's diagnostics.

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
operator must add the browser application's exact origin to the comma-separated
`CHRONO_SERVER_CORS_ORIGIN` allowlist. CMA then admits the `Authorization` header,
the `Last-Event-ID` cursor used for stream reconnection, and supported methods,
including `PUT`, through the browser preflight.

When the origin is missing, the browser reports a CORS or network failure and the
failed `OPTIONS` response has no `Access-Control-Allow-Origin` header. The API
request is never sent, so this failure does not become an SDK `ApiError`. See the
[cross-origin deployment guide](../../../docs/SDK.md#cross-origin-browser-deployments)
for the operator setting and diagnostics.

The same rule applies to AG-UI's run and session SSE responses. Native
`EventSource` cannot send the configured Bearer credential, so it works only on
the same-origin cookie path. Cross-origin frontends create the transport with
`createAguiTransport(cma.config)` and pass it to `AguiConnection`; that transport
uses `fetch`, sends `Authorization` on every stream and history request, and
parses the response's `ReadableStream`. Pass the same config to `AguiOperations`
for Bearer-authenticated run requests. The default `wireTransport` and
`fetchTransport` values are the same-origin cookie instances. A terminal attach
ticket is only for the WebSocket upgrade and is never an AG-UI credential. The
complete route contract is in [AG-UI Authentication](../../../docs/API_AGUI.md#authentication).

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
