import { ProtocolError } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../client';
import { feedbackReasons, type FeedbackDetails } from '../feedbackTypes';

export class FeedbackReasonsError extends Error {
  constructor(readonly status: number) {
    super(
      status === 409
        ? 'Feedback changed. Reload feedback before saving this draft.'
        : [401, 403, 404].includes(status)
          ? 'Feedback is unavailable for this account.'
          : 'Reasons could not be confirmed. Retry the same reasons.',
    );
  }
}

async function request(
  agentId: string,
  messageId: string,
  suffix: string,
  init: RequestInit,
): Promise<FeedbackDetails> {
  const response = await fetch(
    `/api/v1/agents/${encodeURIComponent(agentId)}/messages/${encodeURIComponent(messageId)}/feedback${suffix}`,
    { credentials: 'same-origin', cache: 'no-store', ...init },
  );
  if (!response.ok) throw new FeedbackReasonsError(response.status);
  const value = (await response.json()) as FeedbackDetails;
  if (
    !value ||
    value.agent_id !== agentId ||
    value.message_id !== messageId ||
    typeof value.viewer_id !== 'string' ||
    !['up', 'down', null].includes(value.vote) ||
    !Number.isSafeInteger(value.version) ||
    value.version < 0 ||
    !Array.isArray(value.reasons) ||
    !value.reasons.every((reason) => feedbackReasons.includes(reason)) ||
    new Set(value.reasons).size !== value.reasons.length ||
    typeof value.comment !== 'string' ||
    Array.from(value.comment).length > 500 ||
    (value.updated_at !== null &&
      (typeof value.updated_at !== 'string' || !Number.isFinite(Date.parse(value.updated_at)))) ||
    (value.request_id !== null && typeof value.request_id !== 'string') ||
    (value.vote !== 'down' && (value.reasons.length !== 0 || value.comment !== ''))
  )
    throw new ProtocolError('Agent Manager returned invalid feedback reasons.');
  return value;
}

export function feedbackReasonsClient(): Pick<
  CmaWebClient,
  'readFeedbackDetails' | 'setFeedbackReasons'
> {
  return {
    readFeedbackDetails: (agent, message, options) =>
      request(agent, message, '', { signal: options?.signal }),
    setFeedbackReasons: async (agent, message, body, options) => {
      const result = await request(agent, message, '/reasons', {
        method: 'PUT',
        signal: options?.signal,
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      if (
        result.vote !== 'down' ||
        result.request_id !== body.request_id ||
        result.version !== body.expected_version + 1 ||
        result.comment !== body.comment ||
        JSON.stringify(result.reasons) !== JSON.stringify(body.reasons)
      )
        throw new ProtocolError('Agent Manager returned different feedback reasons.');
      return result;
    },
  };
}
