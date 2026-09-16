import { useMemo } from 'react';
import type { BridgeViewsClient } from './client';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';

/** Channel use never changes an Agent's immutable creation category. */
export function RetainedReplyPreview({
  client,
  agentId,
  disabled,
}: {
  readonly client: BridgeViewsClient;
  readonly agentId: string;
  readonly disabled: boolean;
}) {
  const read = useMemo(
    () =>
      disabled || agentId === ''
        ? undefined
        : async (signal: AbortSignal) => {
            const provenance = await client.agents.provenance(agentId, { signal });
            if (provenance.agent_id !== agentId)
              throw new Error('Creation origin did not match the selected Agent.');
            return { value: provenance.creation_origin };
          },
    [client, agentId, disabled],
  );
  const origin = useAuthorizedRead(read);
  const category = origin.state.status === 'ready' ? origin.state.value : undefined;
  if (agentId === '') return null;
  return (
    <p>
      {origin.state.status === 'error'
        ? 'Creation origin could not be verified. Only Agents originally created by a channel bot can use channel reply tools.'
        : category === undefined
          ? 'Creation origin is unavailable. Only Agents originally created by a channel bot can use channel reply tools.'
          : category === 'channel_bridge'
            ? 'This Agent was originally created by a channel bot. Sending replies requires channel reply tools and current channel permissions.'
            : category === 'legacy_unknown'
              ? 'This Agent’s creation origin is unknown. Its output remains in CMA, and attaching it to this Bridge does not give it channel reply tools.'
              : 'This Agent was not originally created by a channel bot. Its output remains in CMA, and attaching it to this Bridge does not give it channel reply tools.'}{' '}
      Final output is never sent to the channel automatically.
    </p>
  );
}
