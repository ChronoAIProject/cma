# TypeScript Reference

<!-- nav: 80 Reference -->

The API reference is generated from the package source, JSDoc, and declarations;
this file does not duplicate signatures by hand. Run:

```sh
npm run build
```

The generated declaration entry point is `dist/index.d.ts`, with declaration maps
back to `src/`. In an installed package, editors and TypeScript resolve that file
through the package's `types` and `exports` fields. In the repository, start at the
[public export list](../src/index.ts).

```ts
import {
  Cma,
  createClient,
  type ClientConfig,
  type Session,
  type SessionEvent,
} from '@chronoai/cma-sdk';

const config: ClientConfig = { baseUrl: 'https://cma.example.com', cookies: false };
const client: Cma = createClient(config);
const consume = (session: Session, event: SessionEvent): string =>
  `${session.id}:${String(event.seq)}`;

void client;
void consume;
```

Two package entry points are public:

- `@chronoai/cma-sdk` is the production client, types, errors, event parser, AG-UI
  connection, secret-free sponsorship projections, and conversation fold.
- `@chronoai/cma-sdk/testing` contains deterministic builders, mock transports, and
  transcripts for consumer tests. It is not imported by the production entry point.

Unknown runtime identifiers, event discriminators, recovery states, and future
session lifecycle values cross the boundary according to the forward-compatibility
rules in [Sessions](sessions.md) and [Streaming](streaming.md). Other malformed
known shapes fail closed as `ProtocolError`.

The sponsorship exports intentionally provide only four secret-free page reads:
owner credentials, owner policies with accounting, contributor grants, and sponsor
grants. They include exact allowlist decoders and role-bound continuation checks,
but no mutation client. The checked-in binary composition keeps its sponsorship
service absent and composes no provider, billing, pricing, scheduler, worker, or
runtime adapter, so these contract-first reads remain unavailable. See
[Sponsorships](sponsorships.md) for the complete boundary.

## Request correlation

Every operation sends an `X-Request-Id`. By default the SDK generates a UUID, but
callers can supply an exact id through the operation's final options argument:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example.com' });
const sessions = await cma.listSessions({ requestId: 'Support.Case/488:Exact' });

void sessions;
```

The per-operation `requestId` takes precedence over `requestIdProvider`. Otherwise,
the provider runs exactly once for each caller-visible logical operation. The
resolved text is reused byte-for-byte for every page of an aggregate list and every
REST SSE or AG-UI reconnect belonging to that operation. Request ids must already
be nonempty visible HTTP header values; the SDK rejects values that a `Headers`
implementation would trim, normalize, or otherwise change.

Configure `responseObserver` to receive the HTTP status and usable response
correlation id as soon as each response handshake completes:

```ts
import { createClient, type ResponseMetadata } from '@chronoai/cma-sdk';

const responses: ResponseMetadata[] = [];
const cma = createClient({
  requestIdProvider: () => crypto.randomUUID(),
  responseObserver: (metadata) => {
    responses.push(metadata);
  },
});

await cma.readConformance();
```

The observer runs before retry inspection and before status or body decoding. A
throw or rejected promise becomes `ResponseObserverError`. `metadata.requestId`
comes only from the response's HTTP `X-Request-Id` header; it is `undefined` when
that header is missing or malformed. Problem-body `request_id` fields are never
used for correlation. Structured API errors expose the same header-derived value
as `error.requestId`.
