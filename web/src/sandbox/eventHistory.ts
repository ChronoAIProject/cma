import type { CmaWebClient } from '../api/client';
import type { SandboxEvent } from '../api/types';

export async function readSandboxHistory(
  client: CmaWebClient,
  id: string,
  signal: AbortSignal,
  previous: readonly SandboxEvent[] = [],
): Promise<readonly SandboxEvent[]> {
  const events = [...previous];
  let cursor = events.at(-1)?.seq ?? 0;
  for (;;) {
    signal.throwIfAborted();
    const page = await client.readSandboxEvents(id, { cursor, signal });
    if (page.length === 0) return events;
    for (const event of page) {
      if (event.sandboxId !== id || !Number.isSafeInteger(event.seq) || event.seq <= cursor)
        throw new Error('Sandbox events did not advance.');
      events.push(event);
      cursor = event.seq;
    }
  }
}
