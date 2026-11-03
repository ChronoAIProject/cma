import type { MessageVote } from '../api/client';

const fallback = new Map<string, Record<string, MessageVote>>();

function key(agentId: string, viewerId: string) {
  return `cma:message-feedback:pending:${JSON.stringify([viewerId, agentId])}`;
}

export function pendingFeedback(agentId: string, viewerId: string): Record<string, MessageVote> {
  const scope = key(agentId, viewerId);
  const retained = fallback.get(scope);
  if (retained) return retained;
  try {
    const value: unknown = JSON.parse(sessionStorage.getItem(scope) ?? '{}');
    if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
    return Object.fromEntries(
      Object.entries(value).filter(
        ([messageId, vote]) =>
          /^event:[1-9]\d*$/.test(messageId) && (vote === null || vote === 'up' || vote === 'down'),
      ),
    );
  } catch {
    return fallback.get(scope) ?? {};
  }
}

export function retainFeedback(
  agentId: string,
  viewerId: string,
  messageId: string,
  vote: MessageVote | undefined,
) {
  const scope = key(agentId, viewerId);
  const entries = { ...pendingFeedback(agentId, viewerId) };
  if (vote === undefined) delete entries[messageId];
  else entries[messageId] = vote;
  try {
    if (Object.keys(entries).length === 0) sessionStorage.removeItem(scope);
    else sessionStorage.setItem(scope, JSON.stringify(entries));
    fallback.delete(scope);
  } catch {
    fallback.set(scope, entries);
  }
}
