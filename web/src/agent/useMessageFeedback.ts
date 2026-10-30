import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useCmaClient } from '../api/context';
import type { MessageVote } from '../api/client';

interface FeedbackRow {
  readonly vote: MessageVote;
  readonly pending?: boolean;
  readonly error?: string;
  readonly retryVote?: MessageVote;
}
export interface MessageFeedbackControls {
  readonly rows: Readonly<Record<string, FeedbackRow>>;
  readonly ready: boolean;
  readonly error?: string;
  readonly reload: () => void;
  readonly setVote: (messageId: string, vote: MessageVote) => void;
}

/** Feedback is separate owner data; it never submits or steers a conversation. */
export function useMessageFeedback(
  agentId: string,
  viewerId: string | undefined,
  enabled: boolean,
): MessageFeedbackControls | undefined {
  const client = useCmaClient();
  const supported =
    enabled &&
    viewerId !== undefined &&
    Boolean(client.readMessageFeedback && client.setMessageFeedback);
  const scope = useMemo(
    () => ({ agentId, viewerId, supported, client }),
    [agentId, viewerId, supported, client],
  );
  const active = useRef<object | undefined>(scope);
  const pending = useRef(new Map<string, AbortController>());
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<{
    scope: object;
    ready: boolean;
    rows: Readonly<Record<string, FeedbackRow>>;
    error?: string;
  }>({ scope, ready: false, rows: {} });
  useLayoutEffect(() => {
    active.current = scope;
    const requests = pending.current;
    return () => {
      active.current = undefined;
      for (const request of requests.values()) request.abort();
      requests.clear();
    };
  }, [scope]);
  useEffect(() => {
    if (!supported) return;
    const request = new AbortController();
    void client.readMessageFeedback!(agentId, { signal: request.signal })
      .then((items) => {
        if (request.signal.aborted || active.current !== scope) return;
        setState({
          scope,
          ready: true,
          rows: Object.fromEntries(items.map((item) => [item.message_id, { vote: item.vote }])),
        });
      })
      .catch((cause: unknown) => {
        if (request.signal.aborted || active.current !== scope) return;
        setState({
          scope,
          ready: false,
          rows: {},
          error: cause instanceof Error ? cause.message : 'Message feedback could not be loaded.',
        });
      });
    return () => request.abort();
  }, [scope, supported, client, agentId, revision]);

  return useMemo(() => {
    if (!supported) return undefined;
    const current: Pick<MessageFeedbackControls, 'ready' | 'rows' | 'error'> =
      state.scope === scope ? state : { ready: false, rows: {} };
    return {
      ...current,
      reload: () => setRevision((value) => value + 1),
      setVote: (messageId, vote) => {
        if (active.current !== scope || !current.ready || pending.current.has(messageId)) return;
        const retained = current.rows[messageId];
        if (retained?.error && vote !== retained.retryVote) return;
        const request = new AbortController();
        pending.current.set(messageId, request);
        setState((previous) => ({
          ...previous,
          rows: {
            ...previous.rows,
            [messageId]: { vote: previous.rows[messageId]?.vote ?? null, pending: true },
          },
        }));
        void client.setMessageFeedback!(agentId, messageId, vote, { signal: request.signal })
          .then((result) => {
            if (request.signal.aborted || active.current !== scope) return;
            setState((previous) => ({
              ...previous,
              rows: { ...previous.rows, [messageId]: { vote: result.vote } },
            }));
          })
          .catch((cause: unknown) => {
            if (request.signal.aborted || active.current !== scope) return;
            setState((previous) => ({
              ...previous,
              rows: {
                ...previous.rows,
                [messageId]: {
                  vote: previous.rows[messageId]?.vote ?? null,
                  error: cause instanceof Error ? cause.message : 'Your vote could not be saved.',
                  retryVote: vote,
                },
              },
            }));
          })
          .finally(() => {
            if (pending.current.get(messageId) === request) pending.current.delete(messageId);
          });
      },
    };
  }, [supported, state, scope, client, agentId]);
}
