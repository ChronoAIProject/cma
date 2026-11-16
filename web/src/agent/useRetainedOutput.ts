import { useEffect, useMemo, useRef, useState } from 'react';
import {
  ApiError,
  ProtocolError,
  type RetainedOutput,
  type RetainedOutputPage,
} from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import type { ConversationState, ConversationMessage } from './model';

export interface RetainedOutputAccess {
  readonly client: CmaWebClient;
  readonly agentId: string;
  readonly viewerId: string;
  readonly authority?: object;
}
interface Reading {
  readonly output?: RetainedOutput;
  readonly content: string;
  readonly next?: string;
  readonly loading: boolean;
  readonly error?: string;
}

function append(previous: Reading, page: RetainedOutputPage, agentId: string): Reading {
  const output = page.output;
  const offset = new TextEncoder().encode(previous.content).length;
  if (output === null) {
    if (offset || page.content || page.next || page.offset)
      throw new ProtocolError('Retained output disappeared during retrieval');
    return { content: '', loading: false };
  }
  const end = offset + new TextEncoder().encode(page.content).length;
  if (
    output.agent_id !== agentId ||
    !output.partial ||
    (previous.output !== undefined && JSON.stringify(previous.output) !== JSON.stringify(output)) ||
    page.offset !== offset ||
    end > output.byte_length ||
    output.byte_length > 1024 * 1024 ||
    page.content.length === 0 ||
    new TextEncoder().encode(page.content).length > 512 * 1024 ||
    (page.next !== null) !== end < output.byte_length ||
    page.next === previous.next
  )
    throw new ProtocolError('Retained output chunks do not identify one complete prefix');
  return {
    output,
    content: previous.content + page.content,
    next: page.next ?? undefined,
    loading: page.next !== null,
  };
}

/** One bounded retrieval per history change, scoped to the current viewer and authority. */
export function useRetainedOutput(
  access: RetainedOutputAccess | undefined,
  conversation: ConversationState,
) {
  const { client, agentId, viewerId, authority } = access ?? {};
  const scope = useMemo(
    () => ({ client, agentId, viewerId, authority }),
    [client, agentId, viewerId, authority],
  );
  const failures = Object.entries(conversation.turnOutcomes ?? {})
    .filter(([, outcome]) => outcome === 'failed')
    .map(([run]) => run)
    .sort()
    .join(',');
  const [revision, setRevision] = useState(0);
  const retry = useMemo(() => () => setRevision((value) => value + 1), []);
  const saved = useRef<{ scope: object; value: Reading } | undefined>(undefined);
  const [state, setState] = useState<{ scope: object; value: Reading }>();
  useEffect(() => {
    const read = client?.readRetainedOutput;
    if (!read || !agentId || !viewerId) return;
    const request = new AbortController();
    let current =
      saved.current?.scope === scope ? saved.current.value : { content: '', loading: true };
    // A verified completed immutable prefix never needs another content read.
    if (current.output && !current.next && !current.error) return;
    const publish = () => {
      if (request.signal.aborted) return;
      saved.current = { scope, value: current };
      setState(saved.current);
    };
    current = { ...current, loading: true, error: undefined };
    publish();
    void (async () => {
      try {
        for (let pages = 0; pages < 33; pages += 1) {
          const page = await read(agentId, { cursor: current.next, signal: request.signal });
          if (request.signal.aborted) return;
          current = append(current, page, agentId);
          publish();
          if (!current.next) return;
        }
        throw new ProtocolError('Retained output exceeded its page bound');
      } catch (error) {
        if (request.signal.aborted) return;
        if (error instanceof ApiError && [401, 403, 404, 409].includes(error.status)) {
          current = { content: '', loading: false };
        } else {
          current = {
            ...current,
            loading: false,
            error: 'Retained partial output could not be fully loaded.',
          };
        }
        publish();
      }
    })();
    return () => request.abort();
  }, [client, agentId, viewerId, scope, failures, revision]);
  const value = state?.scope === scope ? state.value : undefined;
  const message = useMemo<ConversationMessage | undefined>(
    () =>
      value?.output
        ? {
            id: value.output.id,
            runId: String(value.output.turn_sequence),
            role: 'assistant',
            content: value.content,
            status: 'error',
            at: value.output.failed_at,
            order: value.output.failure_sequence,
            finalAnswer: false,
            retainedPartial: { loading: value.loading, error: value.error, retry },
          }
        : undefined,
    [value, retry],
  );
  const display = useMemo(() => {
    if (!message) return conversation;
    const messages = conversation.messages.filter(
      (item) =>
        !(
          item.liveOnly &&
          item.role === 'assistant' &&
          item.runId === message.runId &&
          !value?.loading &&
          !value?.error &&
          message.content.startsWith(item.content)
        ),
    );
    return { ...conversation, messages: [...messages, message] };
  }, [conversation, message, value]);
  return { conversation: display, error: !message && failures ? value?.error : undefined, retry };
}
