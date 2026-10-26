import { useCallback, useState, type Dispatch, type SetStateAction } from 'react';
import type { CmaWebClient } from '../api/client';
import type { DraftAttachment } from './attachments';

export interface AgentDraft {
  readonly content: string;
  readonly setContent: Dispatch<SetStateAction<string>>;
  readonly attachments: readonly DraftAttachment[];
  readonly setAttachments: Dispatch<SetStateAction<readonly DraftAttachment[]>>;
}

interface DraftValue {
  readonly content: string;
  readonly attachments: readonly DraftAttachment[];
}

const empty: DraftValue = { content: '', attachments: [] };

/** Six in-memory drafts for this viewer; late callbacks never edit a later visit. */
export function useAgentDraft(
  client: CmaWebClient,
  viewerId: string,
  agentId: string | undefined,
): AgentDraft {
  const [stored, setStored] = useState({
    scope: { client, viewerId, agentId },
    value: empty,
    cache: new Map<string | undefined, DraftValue>(),
  });
  const { scope } = stored;
  const current =
    scope.client === client && scope.viewerId === viewerId && scope.agentId === agentId;
  if (!current) {
    const sameViewer = scope.client === client && scope.viewerId === viewerId;
    const cache = sameViewer ? new Map(stored.cache) : new Map<string | undefined, DraftValue>();
    if (sameViewer && scope.agentId !== undefined) cache.set(scope.agentId, stored.value);
    const value = cache.get(agentId) ?? empty;
    cache.delete(agentId);
    while (cache.size >= 6) cache.delete(cache.keys().next().value);
    setStored({ scope: { client, viewerId, agentId }, value, cache });
  }
  const setContent = useCallback<Dispatch<SetStateAction<string>>>(
    (update) =>
      setStored((previous) => {
        if (previous.scope !== scope) return previous;
        const content = typeof update === 'function' ? update(previous.value.content) : update;
        return content === previous.value.content
          ? previous
          : { ...previous, value: { ...previous.value, content } };
      }),
    [scope],
  );
  const setAttachments = useCallback<Dispatch<SetStateAction<readonly DraftAttachment[]>>>(
    (update) =>
      setStored((previous) => {
        if (previous.scope !== scope) return previous;
        const attachments =
          typeof update === 'function' ? update(previous.value.attachments) : update;
        return attachments === previous.value.attachments
          ? previous
          : { ...previous, value: { ...previous.value, attachments } };
      }),
    [scope],
  );
  return { ...(current ? stored.value : empty), setContent, setAttachments };
}
