import type { ResolvedConfig } from '../config.js';
import { eventStream } from '../streaming.js';
import { frames } from './frames.js';
import type { AgentConversationEvent, AgentStreamOptions } from './types.js';

/** The deadline covers admission and response headers, not an active event body. */
export function connection(
  config: ResolvedConfig,
  path: string,
  threadId: string,
  options: AgentStreamOptions,
  body?: unknown,
  extraHeaders?: Readonly<Record<string, string>>,
): AsyncGenerator<AgentConversationEvent> {
  return eventStream(
    config,
    path,
    options,
    (response, signal) => frames(response, signal, threadId),
    'AG-UI',
    body,
    extraHeaders,
  );
}
