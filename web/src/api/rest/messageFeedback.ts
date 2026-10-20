import { ProtocolError } from '@chronoai/cma-sdk';
import type { CmaWebClient, MessageFeedback } from '../client';

function feedback(value: unknown): value is MessageFeedback {
  return Boolean(
    value &&
    typeof value === 'object' &&
    'message_id' in value &&
    typeof value.message_id === 'string' &&
    /^event:[1-9]\d*$/.test(value.message_id) &&
    'vote' in value &&
    (value.vote === null || value.vote === 'up' || value.vote === 'down'),
  );
}

async function request(path: string, init: RequestInit): Promise<unknown> {
  const response = await fetch(path, { credentials: 'same-origin', cache: 'no-store', ...init });
  if (!response.ok)
    throw new Error(
      response.status === 401 || response.status === 403
        ? 'Message feedback is unavailable for this account.'
        : 'Message feedback could not be confirmed. Retry to check or save your vote.',
    );
  return response.json();
}

export function messageFeedbackClient(): Pick<
  CmaWebClient,
  'readMessageFeedback' | 'setMessageFeedback'
> {
  return {
    readMessageFeedback: async (agentId, options) => {
      const result: MessageFeedback[] = [];
      const seen = new Set<string>();
      let after = 0;
      for (;;) {
        const value = await request(
          `/api/v1/agents/${encodeURIComponent(agentId)}/message-feedback?limit=200${after ? `&after=${String(after)}` : ''}`,
          { signal: options?.signal },
        );
        if (
          !value ||
          typeof value !== 'object' ||
          !('items' in value) ||
          !Array.isArray(value.items) ||
          value.items.length > 200 ||
          !value.items.every((item: unknown) => feedback(item) && item.vote !== null) ||
          !('next' in value) ||
          (value.next !== null &&
            (typeof value.next !== 'number' ||
              !Number.isSafeInteger(value.next) ||
              value.next <= after))
        )
          throw new ProtocolError('Agent Manager returned invalid message feedback.');
        for (const item of value.items as MessageFeedback[]) {
          if (seen.has(item.message_id))
            throw new ProtocolError('Agent Manager returned duplicate message feedback.');
          seen.add(item.message_id);
          result.push(item);
        }
        if (value.next === null) return result;
        after = value.next as number;
      }
    },
    setMessageFeedback: async (agentId, messageId, vote, options) => {
      const value = await request(
        `/api/v1/agents/${encodeURIComponent(agentId)}/messages/${encodeURIComponent(messageId)}/feedback`,
        {
          method: 'PUT',
          signal: options?.signal,
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ vote }),
        },
      );
      if (!feedback(value) || value.message_id !== messageId || value.vote !== vote)
        throw new ProtocolError('Agent Manager returned different message feedback.');
      return value;
    },
  };
}
