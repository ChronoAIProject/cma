# CMA TypeScript SDK

<!-- nav: 10 Overview -->

`@chronoai/cma-sdk` is the typed browser and Node client for CMA. It covers the
REST session API, the reconnecting durable event stream, the AG-UI conversation
transport, anonymous deployment conformance discovery, environment profiles,
usage, resources, pricing, request correlation, four secret-free sponsorship page
reads, and the operations needed to drive a session. The CMA dashboard consumes
this package rather than a private client implementation.

This package is repository-only, deliberately marked private, and is not published
to npm. CMA's web application consumes it through its local file dependency. A
checkout can build and pack the same archive exercised by the scratch-consumer CI
check with:

```sh
cd sdk/typescript
npm ci
npm run build
npm pack
```

The archive is a local integration boundary, not a publication artifact.

The package is ESM-only and uses the platform `fetch`, `Headers`, `Request`, and
`Response` implementations. Current browsers and Node provide those globals. A
test or another JavaScript runtime can inject a compatible `fetch` function.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({
  baseUrl: 'https://cma.example.com',
  credential: () => process.env.CMA_ACCESS_TOKEN,
  cookies: false,
  fetch: globalThis.fetch,
});

const deployment = await cma.readConformance();
console.log(deployment.version, deployment.buildSha);
```

Conformance discovery deliberately omits both bearer credentials and cookies,
even on an authenticated client. See [Authentication](authentication.md) for that
boundary and [Reference](reference.md#request-correlation) for per-operation
request ids and response observation.

## Guides

The navigation comment immediately after each H1 is build metadata for CMA's
Developer surface. Its form is `<!-- nav: <order> <label> -->`; the documentation
check requires unique orders and labels and compiles every TypeScript fence.

| Guide                               | Use it for                                                    |
| ----------------------------------- | ------------------------------------------------------------- |
| [Quickstart](quickstart.md)         | Create a session and consume its stream                       |
| [Authentication](authentication.md) | Browser cookies, bearer credentials, and rotation             |
| [Sessions](sessions.md)             | Creation, lifecycle, capabilities, and management             |
| [Streaming](streaming.md)           | Cursors, reconnects, events, and forward compatibility        |
| [Triggers](triggers.md)             | Publish repository Triggers and run contribution Launches     |
| [Operations](operations.md)         | Input, approvals, control, terminal REST, profiles, and usage |
| [Sponsorships](sponsorships.md)     | Secret-free credential, policy, and grant page reads          |
| [Errors](errors.md)                 | Typed failures and retry decisions                            |
| [Reference](reference.md)           | Generated declarations and public entry points                |

## Protocol boundary

These guides explain how to use the SDK. The wire contracts remain authoritative:

- [REST interface](../../../docs/API_REST.md)
- [AG-UI interface](../../../docs/API_AGUI.md)
- [PTY interface](../../../docs/API_PTY.md)
- [API overview](../../../docs/API_Overview.md)

The package validates success bodies, problem documents, event envelopes, SSE
metadata, identifiers, and size limits before returning them. Do not reach into
wire JSON alongside it. A field absent from the typed result is either deliberately
unprojected or an SDK gap that should be fixed at this boundary.
