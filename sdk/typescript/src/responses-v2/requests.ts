import { v2EventContext } from './event-context.js';
import { answerContext } from '../answer-context.js';
import { ProtocolError } from '../errors.js';
import { dictionary, record, text } from '../schema.js';
import type { CreateV2AgentResponseRequest } from './types.js';

export function conversationId(value: unknown): string {
  if (typeof value === 'string' && value.trim()) return value;
  const source = record(value);
  if (Object.keys(source).length !== 1 || !text(source.id).trim())
    throw new ProtocolError('conversation must identify an existing Agent');
  return text(source.id);
}
export function v2Request(value: unknown, agent?: string): CreateV2AgentResponseRequest {
  const body = record(value);
  if (
    Object.keys(body).some(
      (key) =>
        ![
          'conversation',
          'input',
          'stream',
          'metadata',
          'model',
          'store',
          'previous_response_id',
          'event_context',
          'answer_context',
        ].includes(key),
    )
  )
    throw new ProtocolError('Responses field is unsupported');
  if (body.conversation !== undefined) {
    const id = conversationId(body.conversation);
    if (agent !== undefined && id !== agent && id.startsWith('agt_') && agent.startsWith('agt_'))
      throw new ProtocolError('conversation must match the Agent alias');
  } else if (agent === undefined) throw new ProtocolError('conversation is required');
  if (body.stream !== true || (body.store !== undefined && body.store !== true))
    throw new ProtocolError('Responses require streaming and durable storage');
  if (body.previous_response_id !== undefined && body.previous_response_id !== null)
    throw new ProtocolError('previous_response_id is unsupported; use conversation each turn');
  if (body.model !== undefined && !text(body.model).trim())
    throw new ProtocolError('model must be nonempty');
  let content = body.input;
  if (typeof content !== 'string') {
    if (!Array.isArray(content) || content.length !== 1)
      throw new ProtocolError('expected one user message');
    const message = record(content[0]);
    if (
      message.role !== 'user' ||
      (message.type !== undefined && message.type !== 'message') ||
      Object.keys(message).some((key) => !['role', 'type', 'content'].includes(key))
    )
      throw new ProtocolError('expected one user message');
    content = message.content;
    if (typeof content !== 'string') {
      if (!Array.isArray(content) || !content.length)
        throw new ProtocolError('expected input_text parts');
      content = content
        .map((part: unknown) => {
          const item = record(part);
          if (
            item.type !== 'input_text' ||
            Object.keys(item).some((key) => !['type', 'text'].includes(key))
          )
            throw new ProtocolError('expected input_text');
          return text(item.text);
        })
        .join('');
    }
  }
  if (!text(content).length) throw new ProtocolError('input must not be empty');
  if (body.event_context !== undefined) v2EventContext(body.event_context);
  if (body.answer_context !== undefined) answerContext(body.answer_context);
  if (body.metadata !== undefined && body.metadata !== null) {
    const entries = Object.entries(dictionary(text)(body.metadata));
    if (
      entries.length > 16 ||
      entries.some(([key, item]) => [...key].length > 64 || [...item].length > 512)
    )
      throw new ProtocolError('metadata exceeds its boundary');
  }
  return value as CreateV2AgentResponseRequest;
}
