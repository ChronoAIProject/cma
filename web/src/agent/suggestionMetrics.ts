import { useEffect, useRef } from 'react';
import type { AgentController, MessageAcceptance } from './controllerTypes';
import type { ConversationState } from './conversation';
import type { SuggestedReplySet } from './suggestions';

export type SuggestionSendMode = 'original' | 'edited';
type Scope = {
  viewerId: string;
  agentId: string;
  setId: string;
  answerId: string;
  strategyVersion: string;
};
export type SuggestionMetric = Scope &
  (
    | { kind: 'exposure' }
    | {
        kind: 'send_confirmed';
        itemId: string;
        mode: SuggestionSendMode;
        receipt: MessageAcceptance;
      }
    | {
        kind: 'task_outcome';
        itemId: string;
        mode: SuggestionSendMode;
        receipt: MessageAcceptance;
        outcome: 'completed' | 'failed' | 'interrupted';
      }
  );
const pending = new Map<
  string,
  { metric: Extract<SuggestionMetric, { kind: 'send_confirmed' }>; at: number }
>();
const emitted = new Map<string, number>();
const attempts = new Map<
  string,
  { scope: Scope; itemId: string; mode: SuggestionSendMode; requestKey: string; at: number }
>();
const limit = 100;
const lifetime = 30 * 60 * 1000;

function scope(set: SuggestedReplySet): Scope {
  return {
    viewerId: set.viewerId,
    agentId: set.agentId,
    setId: set.id,
    answerId: set.answerId,
    strategyVersion: set.source.version,
  };
}

function emit(key: string, metric: SuggestionMetric) {
  const now = Date.now();
  for (const [identity, at] of emitted) if (now - at > lifetime) emitted.delete(identity);
  if (emitted.has(key)) return false;
  emitted.set(key, now);
  if (emitted.size > limit) emitted.delete(emitted.keys().next().value!);
  window.dispatchEvent(
    new CustomEvent<SuggestionMetric>('cma:suggestion-metric', { detail: metric }),
  );
  return true;
}

export function confirmSuggestionSend(
  set: SuggestedReplySet,
  itemId: string,
  mode: SuggestionSendMode,
  receipt?: MessageAcceptance,
) {
  if (!receipt || !set.items.some((item) => item.id === itemId)) return;
  confirm(scope(set), itemId, mode, receipt);
}

function confirm(
  identityScope: Scope,
  itemId: string,
  mode: SuggestionSendMode,
  receipt: MessageAcceptance,
) {
  const identity = 'commandId' in receipt ? receipt.commandId : receipt.runId;
  const key = JSON.stringify([
    identityScope.viewerId,
    identityScope.agentId,
    identityScope.setId,
    itemId,
    identity,
  ]);
  const metric = { ...identityScope, kind: 'send_confirmed' as const, itemId, mode, receipt };
  if (!emit(`send:${key}`, metric)) return;
  pending.set(key, { metric, at: Date.now() });
  if (pending.size > limit) pending.delete(pending.keys().next().value!);
}

export function rememberSuggestionRequest(
  set: SuggestedReplySet,
  itemId: string,
  mode: SuggestionSendMode,
  requestKey: string,
) {
  if (!requestKey || !set.items.some((item) => item.id === itemId)) return;
  attempts.set(JSON.stringify([set.viewerId, set.agentId, requestKey]), {
    scope: scope(set),
    itemId,
    mode,
    requestKey,
    at: Date.now(),
  });
  if (attempts.size > limit) attempts.delete(attempts.keys().next().value!);
}

export function useSuggestionExposure(set: SuggestedReplySet, enabled: boolean) {
  const element = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!enabled || !element.current || typeof IntersectionObserver === 'undefined') return;
    let intersecting = false;
    const expose = () => {
      if (document.visibilityState !== 'visible' || !intersecting) return;
      emit(`exposure:${JSON.stringify([set.viewerId, set.agentId, set.id])}`, {
        ...scope(set),
        kind: 'exposure',
      });
    };
    const observer = new IntersectionObserver(
      (entries) => {
        intersecting = entries.some(
          (entry) => entry.isIntersecting && entry.intersectionRatio >= 0.5,
        );
        expose();
      },
      { threshold: 0.5 },
    );
    observer.observe(element.current);
    document.addEventListener('visibilitychange', expose);
    return () => {
      observer.disconnect();
      document.removeEventListener('visibilitychange', expose);
    };
  }, [set, enabled]);
  return element;
}

export function useSuggestionOutcomes(
  conversation: ConversationState | undefined,
  viewerId: string,
  agentId: string,
  retained?: ReturnType<NonNullable<AgentController['readSuggestionRequest']>>,
) {
  useEffect(() => {
    for (const [key, attempt] of attempts) {
      if (Date.now() - attempt.at > lifetime) {
        attempts.delete(key);
        continue;
      }
      if (
        attempt.scope.viewerId !== viewerId ||
        attempt.scope.agentId !== agentId ||
        retained?.key !== attempt.requestKey ||
        retained.state !== 'accepted' ||
        !retained.receipt
      )
        continue;
      confirm(attempt.scope, attempt.itemId, attempt.mode, retained.receipt);
      attempts.delete(key);
    }
    for (const [key, entry] of pending) {
      if (Date.now() - entry.at > lifetime) {
        pending.delete(key);
        continue;
      }
      const { metric } = entry;
      if (!conversation || metric.viewerId !== viewerId || metric.agentId !== agentId) continue;
      const receipt = metric.receipt;
      const turn =
        'runId' in receipt
          ? receipt.runId
          : (conversation.messages.find((message) => message.commandId === receipt.commandId)
              ?.runId ??
            (conversation.responseBinding?.commandId === receipt.commandId &&
            conversation.responseBinding.responseId === receipt.responseId
              ? String(conversation.responseBinding.turnSequence)
              : undefined));
      const outcome = turn === undefined ? undefined : conversation.turnOutcomes?.[turn];
      if (!outcome) continue;
      emit(`outcome:${key}`, { ...metric, kind: 'task_outcome', outcome });
      pending.delete(key);
    }
  }, [conversation, viewerId, agentId, retained]);
}
