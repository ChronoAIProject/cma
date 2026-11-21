import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useCmaClient } from '../api/context';
import type { MessageVote } from '../api/client';
import type { FeedbackDetails } from '../api/feedbackTypes';
import { FeedbackReasonsError } from '../api/rest/feedbackReasons';
import {
  feedbackAuthority,
  reasonDraft,
  retainReasonDraft,
  type ReasonDraft,
} from './feedbackRecovery';

interface ReasonRow {
  readonly open: boolean;
  readonly draft: ReasonDraft;
  readonly details?: FeedbackDetails;
  readonly busy?: 'read' | 'save';
  readonly error?: string;
  readonly conflict?: boolean;
  readonly saved?: boolean;
}
export interface FeedbackReasonControls {
  readonly rows: Readonly<Record<string, ReasonRow>>;
  readonly denied: boolean;
  readonly open: (message: string, load?: boolean) => void;
  readonly close: (message: string) => void;
  readonly edit: (message: string, draft: ReasonDraft) => void;
  readonly save: (message: string) => void;
  readonly voteSaved: (message: string, vote: MessageVote) => void;
}
const emptyDraft: ReasonDraft = { reasons: [], comment: '' };

export function useFeedbackReasons(
  agent: string,
  viewer: string | undefined,
  enabled: boolean,
): FeedbackReasonControls | undefined {
  const client = useCmaClient();
  const supported =
    enabled &&
    viewer !== undefined &&
    Boolean(client.readFeedbackDetails && client.setFeedbackReasons);
  const scope = useMemo(
    () => ({ agent, viewer, supported, client }),
    [agent, viewer, supported, client],
  );
  const active = useRef<object | undefined>(scope);
  const requests = useRef(new Map<string, AbortController>());
  const [state, setState] = useState<{
    scope: object;
    rows: Record<string, ReasonRow>;
    denied: boolean;
  }>({ scope, rows: {}, denied: false });
  useLayoutEffect(() => {
    active.current = scope;
    feedbackAuthority(viewer, enabled);
    const pending = requests.current;
    return () => {
      active.current = undefined;
      for (const request of pending.values()) request.abort();
      pending.clear();
    };
  }, [scope, viewer, enabled]);

  return useMemo(() => {
    if (!supported) return undefined;
    const current: Pick<FeedbackReasonControls, 'rows' | 'denied'> =
      state.scope === scope ? state : { rows: {}, denied: false };
    function update(message: string, change: (row: ReasonRow) => ReasonRow) {
      if (active.current !== scope) return;
      setState((previous) => {
        const base: typeof previous =
          previous.scope === scope ? previous : { scope, rows: {}, denied: false };
        return {
          ...base,
          rows: {
            ...base.rows,
            [message]: change(
              base.rows[message] ?? {
                open: false,
                draft: reasonDraft(agent, viewer!, message) ?? emptyDraft,
              },
            ),
          },
        };
      });
    }
    function start(message: string) {
      requests.current.get(message)?.abort();
      const request = new AbortController();
      requests.current.set(message, request);
      return request;
    }
    function valid(message: string, request: AbortController) {
      return (
        !request.signal.aborted &&
        active.current === scope &&
        requests.current.get(message) === request
      );
    }
    function fail(message: string, request: AbortController, cause: unknown) {
      if (!valid(message, request)) return;
      if (cause instanceof FeedbackReasonsError && [401, 403, 404].includes(cause.status)) {
        feedbackAuthority(viewer, false);
        for (const pending of requests.current.values()) pending.abort();
        setState({ scope, rows: {}, denied: true });
        return;
      }
      update(message, (row) => ({
        ...row,
        busy: undefined,
        error:
          cause instanceof FeedbackReasonsError && cause.status === 409
            ? cause.message
            : row.busy === 'read'
              ? 'Reasons could not be loaded. Reload feedback.'
              : 'Reasons could not be confirmed. Retry the same reasons.',
        conflict: cause instanceof FeedbackReasonsError && cause.status === 409,
      }));
    }
    function open(message: string, load = true, reveal = true) {
      if (active.current !== scope || current.denied) return;
      if (!load) {
        update(message, (row) => ({ ...row, open: true }));
        return;
      }
      const request = start(message);
      update(message, (row) => ({
        ...row,
        open: reveal || row.open,
        busy: 'read',
        error: undefined,
        saved: false,
      }));
      void client.readFeedbackDetails!(agent, message, { signal: request.signal })
        .then((details) => {
          if (!valid(message, request)) return;
          if (details.viewer_id !== viewer) throw new FeedbackReasonsError(403);
          const retained = reasonDraft(agent, viewer!, message);
          const confirmed =
            retained?.pending &&
            details.request_id === retained.pending.request_id &&
            details.version === retained.pending.expected_version + 1 &&
            details.comment === retained.pending.comment &&
            JSON.stringify(details.reasons) === JSON.stringify(retained.pending.reasons);
          let draft = retained ?? { reasons: details.reasons, comment: details.comment };
          if (confirmed) {
            retainReasonDraft(agent, viewer!, message, undefined);
            draft = { reasons: details.reasons, comment: details.comment };
          } else if (retained?.pending && details.version !== retained.pending.expected_version) {
            draft = { reasons: retained.reasons, comment: retained.comment };
            retainReasonDraft(agent, viewer!, message, draft);
          }
          update(message, (row) => ({
            ...row,
            draft,
            details,
            busy: undefined,
            conflict: false,
            saved: Boolean(confirmed),
            error: draft.pending
              ? 'Your previous reasons could not be confirmed. Retry the same reasons.'
              : undefined,
          }));
        })
        .catch((cause) => fail(message, request, cause))
        .finally(() => {
          if (requests.current.get(message) === request) requests.current.delete(message);
        });
    }
    return {
      ...current,
      open,
      close: (message) => update(message, (row) => ({ ...row, open: false })),
      edit: (message, draft) => {
        if (active.current !== scope || current.denied) return;
        retainReasonDraft(agent, viewer!, message, draft);
        update(message, (row) => ({ ...row, draft, saved: false }));
      },
      voteSaved: (message, vote) => {
        if (active.current !== scope) return;
        if (vote === 'down') {
          open(message, true, false);
          return;
        }
        requests.current.get(message)?.abort();
        retainReasonDraft(agent, viewer!, message, undefined);
        update(message, () => ({ open: false, draft: emptyDraft }));
      },
      save: (message) => {
        const row = current.rows[message];
        if (
          active.current !== scope ||
          current.denied ||
          !row?.details ||
          row.details.vote !== 'down' ||
          row.busy ||
          row.conflict ||
          Array.from(row.draft.comment).length > 500
        )
          return;
        const body = row.draft.pending ?? {
          expected_version: row.details.version,
          request_id: crypto.randomUUID(),
          reasons: row.draft.reasons,
          comment: row.draft.comment,
        };
        const draft = { ...row.draft, pending: body };
        retainReasonDraft(agent, viewer!, message, draft);
        const request = start(message);
        update(message, (previous) => ({
          ...previous,
          draft,
          busy: 'save',
          error: undefined,
          saved: false,
        }));
        void client.setFeedbackReasons!(agent, message, body, { signal: request.signal })
          .then((details) => {
            if (!valid(message, request)) return;
            if (details.viewer_id !== viewer) throw new FeedbackReasonsError(403);
            retainReasonDraft(agent, viewer!, message, undefined);
            update(message, (previous) => ({
              ...previous,
              details,
              draft: { reasons: details.reasons, comment: details.comment },
              busy: undefined,
              saved: true,
            }));
          })
          .catch((cause) => fail(message, request, cause))
          .finally(() => {
            if (requests.current.get(message) === request) requests.current.delete(message);
          });
      },
    };
  }, [agent, viewer, client, supported, state, scope]);
}
