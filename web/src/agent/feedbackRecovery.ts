import type { MessageVote } from '../api/client';
import type { FeedbackReason, FeedbackReasonRequest } from '../api/feedbackTypes';

export interface ReasonDraft {
  readonly reasons: readonly FeedbackReason[];
  readonly comment: string;
  readonly pending?: FeedbackReasonRequest;
}
const drafts = new Map<string, Record<string, ReasonDraft>>();
let draftViewer: string | undefined;

export function feedbackAuthority(viewer: string | undefined, enabled: boolean) {
  if (draftViewer !== viewer || !enabled) drafts.clear();
  draftViewer = viewer;
}

export function reasonDraft(agent: string, viewer: string, message: string) {
  return drafts.get(key(agent, viewer))?.[message];
}

export function retainReasonDraft(
  agent: string,
  viewer: string,
  message: string,
  draft: ReasonDraft | undefined,
) {
  const scope = key(agent, viewer);
  const entries = { ...drafts.get(scope) };
  if (draft) entries[message] = draft;
  else delete entries[message];
  if (Object.keys(entries).length) drafts.set(scope, entries);
  else drafts.delete(scope);
}

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
