# TypeScript Quickstart

<!-- nav: 20 Quickstart -->

This example starts from a deployment URL and bearer credential, creates a
conversation session, submits one owner message, and reads durable events until
the turn ends. `startingAfter` is the session's last known sequence, so events
committed between creation and stream connection are replayed rather than lost.

```ts
import { createClient, type SessionEvent } from '@chronoai/cma-sdk';

async function main(): Promise<void> {
  const cma = createClient({
    baseUrl: 'https://cma.example.com',
    credential: () => {
      const credential = process.env.CMA_ACCESS_TOKEN;
      if (credential === undefined) {
        throw new Error('CMA_ACCESS_TOKEN is required');
      }
      return credential;
    },
    cookies: false,
  });

  const session = await cma.createSession({
    environmentProfile: process.env.CMA_ENVIRONMENT_PROFILE,
  });
  if (!session.capabilities.conversation) {
    throw new Error('the session was not granted conversation input');
  }

  const events = cma.streamEvents(session.id, {
    startingAfter: session.lastSeq ?? 0,
  });
  let inputSent = session.lifecycle === 'ready';
  if (inputSent) {
    await cma.sendInput(session.id, {
      text: 'Summarise this repository',
      newline: true,
    });
  }

  for await (const event of events) {
    render(event);
    if (!inputSent && event.event.type === 'status_changed' && event.event.status === 'ready') {
      await cma.sendInput(session.id, {
        text: 'Summarise this repository',
        newline: true,
      });
      inputSent = true;
    } else if (
      !inputSent &&
      event.event.type === 'status_changed' &&
      event.event.status === 'terminated'
    ) {
      throw new Error('the session terminated before it became ready');
    } else if (inputSent && event.event.type === 'turn_completed') {
      break;
    }
  }
}

function render(event: SessionEvent): void {
  console.log(event.seq, event.event.type);
}

void main();
```

The SDK does not wait for provisioning to settle before `createSession` returns.
The granted capability set in that response is already authoritative. Read the
session again when the interface needs a later lifecycle or runtime metadata:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const created = await cma.createSession();
const settled = await cma.readSession(created.id);

console.log(settled.lifecycle, settled.capabilities);
```

In CMA's same-origin browser deployment, `createClient()` is sufficient: relative
API paths and the browser session cookie are the defaults. A Node backend normally
sets `baseUrl`, supplies a rotating credential callback, and sets `cookies: false`.
The optional `CMA_ENVIRONMENT_PROFILE` must name a conversation profile when the
deployment's unqualified default is terminal-shaped.
See [Authentication](authentication.md) before moving a quickstart into a service.
