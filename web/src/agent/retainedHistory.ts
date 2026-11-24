import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { raceWithAbortSignal } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import type { AgentDetail } from '../api/types';
import { conversationFromTranscript, type ConversationState } from './conversation';
import type { RetiredAgentIdentity, RetiredAgentPresentation } from './retiredPresentation';
import { errorMessage } from './controllerError';
import { rateLimitDelay } from './transport/readRetry';

type Reading =
  | { readonly status: 'loading' }
  | { readonly status: 'error'; readonly message: string; readonly retryAt: number }
  | { readonly status: 'ready'; readonly conversation: ConversationState };
export type RetainedAgentRead = (
  | { readonly status: 'unconfirmed' }
  | (Reading & { readonly detail: AgentDetail; readonly identity: RetiredAgentIdentity })
) & { readonly retry: () => void };

export async function loadRetainedHistory(
  client: CmaWebClient,
  identity: RetiredAgentIdentity,
  signal: AbortSignal,
): Promise<ConversationState> {
  signal.throwIfAborted();
  const transcript = await raceWithAbortSignal(signal, () =>
    client.readAgentTranscript(identity.agentId, {
      signal,
      cache: 'no-store',
      maxPages: 256,
      maxBytes: 16 * 1024 * 1024,
    }),
  );
  signal.throwIfAborted();
  if (transcript.agentId !== identity.agentId)
    throw new Error('The retained conversation did not match this Agent.');
  const conversation = conversationFromTranscript(transcript);
  return {
    ...conversation,
    recovery: conversation.recovery.map((notice) =>
      notice.runId && conversation.turnOutcomes?.[notice.runId] === 'failed'
        ? {
            ...notice,
            detail:
              'This recorded Agent run failed. Its delivery or task effects may be incomplete.',
            nextStep: undefined,
          }
        : notice,
    ),
  };
}

/** The controller owns this read; it has no runtime, command or journal capability. */
export function useRetainedHistory(
  client: CmaWebClient,
  presentation: RetiredAgentPresentation | undefined,
): RetainedAgentRead {
  const { viewerId, authEpoch, agentId, worktreeId, workspaceId, sandboxId } =
    presentation?.identity ?? {};
  const scope = useMemo(
    () => ({ client, viewerId, authEpoch, agentId, worktreeId, workspaceId, sandboxId }),
    [client, viewerId, authEpoch, agentId, worktreeId, workspaceId, sandboxId],
  );
  const active = useRef<typeof scope | undefined>(undefined);
  const currentRead = useRef<{ scope: typeof scope; value: Reading } | undefined>(undefined);
  const [reading, setReading] = useState<typeof currentRead.current>();
  const [revision, setRevision] = useState(0);
  useLayoutEffect(() => {
    active.current = scope;
    return () => {
      active.current = undefined;
    };
  }, [scope]);
  useEffect(() => {
    if (
      !viewerId ||
      authEpoch === undefined ||
      !agentId ||
      !worktreeId ||
      !workspaceId ||
      !sandboxId
    )
      return;
    const request = new AbortController();
    let timedOut = false;
    const publish = (value: Reading) => {
      if (active.current !== scope) return;
      currentRead.current = { scope, value };
      setReading(currentRead.current);
    };
    publish({ status: 'loading' });
    const timeout = window.setTimeout(() => {
      timedOut = true;
      request.abort();
    }, 120_000);
    void loadRetainedHistory(
      client,
      { viewerId, authEpoch, agentId, worktreeId, workspaceId, sandboxId },
      request.signal,
    )
      .then((conversation) => {
        if (!request.signal.aborted) publish({ status: 'ready', conversation });
      })
      .catch((error: unknown) => {
        if (request.signal.aborted && !timedOut) return;
        publish({
          status: 'error',
          message: timedOut ? 'The retained conversation read timed out.' : errorMessage(error),
          retryAt: Date.now() + rateLimitDelay(error, 0),
        });
      })
      .finally(() => window.clearTimeout(timeout));
    return () => {
      window.clearTimeout(timeout);
      request.abort();
    };
  }, [client, scope, viewerId, authEpoch, agentId, worktreeId, workspaceId, sandboxId, revision]);
  const retry = () => {
    const current = currentRead.current;
    if (
      active.current !== scope ||
      current?.scope !== scope ||
      current.value.status !== 'error' ||
      Date.now() < current.value.retryAt
    )
      return;
    currentRead.current = { scope, value: { status: 'loading' } };
    setReading(currentRead.current);
    setRevision((value) => value + 1);
  };
  if (!presentation) return { status: 'unconfirmed', retry };
  return {
    ...(reading?.scope === scope ? reading.value : { status: 'loading' as const }),
    detail: presentation.detail,
    identity: presentation.identity,
    retry,
  };
}
