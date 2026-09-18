import type { BridgeViewsClient } from '../bridges/client';

/** Creation provenance records a logical partition hash, not its allocation ID. */
export async function pendingChannelAllocation(
  client: BridgeViewsClient,
  bridge: string,
  generation: number,
  agentId: string,
  signal: AbortSignal,
): Promise<string> {
  let cursor: string | undefined;
  let found: string | undefined;
  let inspected = 0;
  const cursors = new Set<string>();
  do {
    signal.throwIfAborted();
    const page = await client.bridges.listPartitions(bridge, {
      limit: Math.min(25, 256 - inspected),
      cursor,
      signal,
    });
    signal.throwIfAborted();
    inspected += page.items.length;
    if (inspected > 256) throw new Error('Channel allocation lookup exceeded its bound.');
    for (const item of page.items) {
      if (
        item.bridge_id === bridge &&
        item.generation === generation &&
        item.current &&
        item.placement !== 'existing_agent'
      ) {
        if (item.target.status === 'redacted')
          throw new Error('Channel allocation lookup is redacted.');
        if (
          item.target.status !== 'available' ||
          !item.target.present.includes('agent') ||
          item.target.ids.agent_id !== agentId
        )
          continue;
        if (found !== undefined) throw new Error('Channel allocation lookup is ambiguous.');
        found = item.id;
      }
    }
    if (page.next_cursor === null) break;
    if (inspected >= 256 || cursors.has(page.next_cursor))
      throw new Error('Channel allocation lookup is incomplete.');
    cursor = page.next_cursor;
    cursors.add(cursor);
  } while (cursor !== undefined);
  if (found === undefined) throw new Error('Current channel allocation is unavailable.');
  return found;
}
