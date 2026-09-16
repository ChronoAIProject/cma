import { useCallback, useState, type Dispatch, type SetStateAction } from 'react';
import type { CmaWebClient } from '../api/client';

export interface AgentDraft {
  readonly content: string;
  readonly setContent: Dispatch<SetStateAction<string>>;
}

/** Keep unsent input above access-refresh remounts, only for this selected identity. */
export function useAgentDraft(
  client: CmaWebClient,
  viewerId: string,
  agentId: string | undefined,
): AgentDraft {
  const [stored, setStored] = useState({ scope: { client, viewerId, agentId }, content: '' });
  const { scope } = stored;
  const current =
    scope.client === client && scope.viewerId === viewerId && scope.agentId === agentId;
  if (!current) setStored({ scope: { client, viewerId, agentId }, content: '' });
  const setContent = useCallback<Dispatch<SetStateAction<string>>>(
    (update) =>
      setStored((previous) => {
        if (previous.scope !== scope) return previous;
        const content = typeof update === 'function' ? update(previous.content) : update;
        return content === previous.content ? previous : { ...previous, content };
      }),
    [scope],
  );
  return { content: current ? stored.content : '', setContent };
}
