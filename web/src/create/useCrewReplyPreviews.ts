import { useEffect, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { AssistantReplyPreview } from '../api/replyPreview';
import type { NavigationState } from '../shell/navigation';

type Replies = Readonly<Record<string, AssistantReplyPreview>>;
interface PreviewScope {
  readonly client: CmaWebClient;
  readonly viewerId?: string;
  readonly authEpoch?: number;
  readonly scope?: string;
  readonly navigation?: NavigationState;
  readonly visibleAgentIds: readonly string[];
  readonly enabled: boolean;
  readonly visible?: boolean;
}

export function useCrewReplyPreviews({
  client,
  viewerId,
  authEpoch,
  scope,
  navigation,
  visibleAgentIds,
  enabled,
  visible = true,
}: PreviewScope): Replies {
  const snapshot =
    visible &&
    enabled &&
    viewerId &&
    !scope &&
    navigation?.status === 'ready' &&
    !navigation.refreshError
      ? navigation.items
      : undefined;
  const ids = JSON.stringify([...new Set(visibleAgentIds)].slice(0, 3));
  const [state, setState] = useState<{
    client: CmaWebClient;
    viewerId?: string;
    authEpoch?: number;
    snapshot: typeof snapshot;
    ids: string;
    replies: Replies;
  }>({ client, viewerId, authEpoch, snapshot, ids, replies: {} });
  const current =
    state.client === client &&
    state.viewerId === viewerId &&
    state.authEpoch === authEpoch &&
    state.snapshot === snapshot &&
    state.ids === ids;
  if (!current) setState({ client, viewerId, authEpoch, snapshot, ids, replies: {} });

  useEffect(() => {
    if (!snapshot || !client.readAgentReplyPreview || document.hidden) return;
    const request = new AbortController();
    const identity = { client, viewerId, authEpoch, snapshot, ids };
    const selected: string[] = JSON.parse(ids);
    let next = 0;
    const read = async () => {
      while (!request.signal.aborted && next < selected.length) {
        const agentId = selected[next++]!;
        try {
          const result = await client.readAgentReplyPreview!(agentId, { signal: request.signal });
          if (request.signal.aborted || document.hidden) return;
          if (result.kind === 'ready' && result.agentId === agentId)
            setState((current) =>
              request.signal.aborted
                ? current
                : { ...identity, replies: { ...current.replies, [agentId]: result.reply } },
            );
        } catch {
          // An optional history read cannot block another card or task creation.
        }
      }
    };
    const hide = () => {
      if (!document.hidden) return;
      request.abort();
      setState({ ...identity, replies: {} });
    };
    document.addEventListener('visibilitychange', hide);
    void read();
    void read();
    return () => {
      request.abort();
      document.removeEventListener('visibilitychange', hide);
    };
  }, [client, viewerId, authEpoch, snapshot, ids]);

  return visible && snapshot && !document.hidden && current ? state.replies : {};
}
