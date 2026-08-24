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
  connection, and conversation fold.
- `@chronoai/cma-sdk/testing` contains deterministic builders, mock transports, and
  transcripts for consumer tests. It is not imported by the production entry point.

Unknown runtime identifiers, event discriminators, recovery states, and future
session lifecycle values cross the boundary according to the forward-compatibility
rules in [Sessions](sessions.md) and [Streaming](streaming.md). Other malformed
known shapes fail closed as `ProtocolError`.
