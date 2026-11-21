import { useCallback } from 'react';
import type { ShareIdentity } from '@cma/client';
import type { ResourceViewsClient } from './client';
import { ShareRecipientSearch } from '../ui/ShareRecipientSearch';

export function AgentShareRecipients({
  client,
  agentId,
  value,
  onChange,
  disabled,
}: {
  readonly client: ResourceViewsClient;
  readonly agentId: string;
  readonly value: readonly ShareIdentity[];
  readonly onChange: (value: readonly ShareIdentity[]) => void;
  readonly disabled: boolean;
}) {
  const search = useCallback(
    async (query: string, signal: AbortSignal) =>
      (await client.resourceAccess.shareRecipients(agentId, query, { signal })).items,
    [client, agentId],
  );
  return (
    <ShareRecipientSearch search={search} value={value} onChange={onChange} disabled={disabled} />
  );
}
