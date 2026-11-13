import { observeRead } from './transport/readObserver';
import { retrySchedule } from './transport/readRetry';
import { useEffect, useMemo, useState } from 'react';
import { useCmaClient } from '../api/context';
import type { ConversationState } from './conversation';
import { useSuggestedReplies, type SuggestedReplySet } from './suggestions';

export function useContextualSuggestions(
  conversation: ConversationState | undefined,
  viewerId: string,
  agentId: string,
  enabled: boolean,
  provided?: SuggestedReplySet | null,
) {
  const client = useCmaClient();
  const answer = conversation?.messages.at(-1);
  const eligible =
    enabled &&
    answer?.finalAnswer &&
    !conversation?.activeRunId &&
    conversation?.latestTurnId === answer.runId;
  const scope = useMemo(
    () => ({
      client,
      viewerId,
      agentId,
      enabled: Boolean(eligible),
      turn: answer?.runId,
      sequence: answer?.sequence,
      provided,
    }),
    [client, viewerId, agentId, eligible, answer?.runId, answer?.sequence, provided],
  );
  const [saved, setSaved] = useState<{ scope: object; set: SuggestedReplySet | null }>();
  useEffect(() => {
    if (
      !scope.enabled ||
      !client.readSuggestedReplies ||
      provided !== undefined ||
      scope.sequence === undefined ||
      !scope.turn
    )
      return;
    let failures = 0;
    let reads = 0;
    const observer = observeRead(async (signal) => {
      try {
        reads += 1;
        const result = await client.readSuggestedReplies!(
          agentId,
          {
            turn_sequence: Number(scope.turn),
            answer_sequence: scope.sequence!,
          },
          { signal: signal },
        );
        if (signal.aborted) return;
        setSaved({ scope, set: result.status === 'ready' ? result.set : null });
        failures = 0;
        if ((result.status === 'pending' || result.status === 'running') && reads < 50)
          return { next: 1500, cooldown: 1500 };
      } catch (error) {
        if (!signal.aborted) {
          setSaved({ scope, set: null });
          if (reads < 50) return retrySchedule(error, ++failures);
        }
      }
    });
    const deadline = setTimeout(() => observer.dispose(), 75_000);
    return () => {
      observer.dispose();
      clearTimeout(deadline);
    };
  }, [scope, client, agentId, provided]);
  const selected =
    provided !== undefined
      ? provided
      : client.readSuggestedReplies
        ? saved?.scope === scope
          ? saved.set
          : null
        : undefined;
  return useSuggestedReplies(conversation, viewerId, agentId, enabled ? selected : null);
}
