import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useCmaClient } from '../api/context';
import type { MessageVote } from '../api/client';
import { pendingFeedback, retainFeedback } from './feedbackRecovery';
import { useFeedbackReasons, type FeedbackReasonControls } from './useFeedbackReasons';

interface FeedbackRow {
  readonly vote: MessageVote;
  readonly pending?: boolean;
  readonly error?: string;
  readonly retryVote?: MessageVote;
  readonly saved?: boolean;
}
export interface MessageFeedbackControls {
  readonly reasons?: FeedbackReasonControls;
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
  const reasons = useFeedbackReasons(agentId, viewerId, supported);
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
        const rows: Record<string, FeedbackRow> = Object.fromEntries(
          items.map((item) => [item.message_id, { vote: item.vote }]),
        );
        for (const [messageId, requested] of Object.entries(pendingFeedback(agentId, viewerId!))) {
          const vote = rows[messageId]?.vote ?? null;
          if (vote === requested) retainFeedback(agentId, viewerId!, messageId, undefined);
          else
            rows[messageId] = {
              vote,
              retryVote: requested,
              error:
                'Your previous vote could not be confirmed. Retry it before changing your choice.',
            };
        }
        setState({
          scope,
          ready: true,
          rows,
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
  }, [scope, supported, client, agentId, viewerId, revision]);

  return useMemo(() => {
    if (!supported) return undefined;
    const current: Pick<MessageFeedbackControls, 'ready' | 'rows' | 'error'> =
      state.scope === scope ? state : { ready: false, rows: {} };
    return {
      ...current,
      reasons,
      reload: () => {
        if (active.current !== scope) return;
        setState((previous) => ({ ...previous, ready: false }));
        setRevision((value) => value + 1);
      },
      setVote: (messageId, vote) => {
        if (active.current !== scope || !current.ready || pending.current.has(messageId)) return;
        const retained = current.rows[messageId];
        if (retained?.error && vote !== retained.retryVote) return;
        if (vote === 'down') reasons?.open(messageId, false);
        const request = new AbortController();
        retainFeedback(agentId, viewerId!, messageId, vote);
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
            retainFeedback(agentId, viewerId!, messageId, undefined);
            reasons?.voteSaved(messageId, result.vote);
            setState((previous) => ({
              ...previous,
              rows: { ...previous.rows, [messageId]: { vote: result.vote, saved: true } },
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
  }, [supported, state, scope, client, agentId, viewerId, reasons]);
}
