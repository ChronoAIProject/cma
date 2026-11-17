import { ProtocolError } from '../errors.js';
import { boolean, dictionary, object, optional, nullable, record, text } from '../schema.js';
import type { CreateResponseRequest } from './types.js';

const requestFields = object({
  conversation: text,
  input: (value: unknown) => value,
  stream: boolean,
  metadata: optional(nullable(dictionary(text))),
  previous_response_id: optional(nullable(text)),
  model: optional(text),
  store: optional(boolean),
});

export function createResponseRequest(value: unknown): CreateResponseRequest {
  const source = record(value);
  const body = requestFields(value);
  for (const field of Object.keys(source)) {
    if (
      ![
        'conversation',
        'input',
        'stream',
        'metadata',
        'previous_response_id',
        'model',
        'store',
      ].includes(field)
    )
      throw new ProtocolError(`Responses field is unsupported: ${field}`);
  }
  if (!body.conversation.trim())
    throw new ProtocolError('conversation must identify an existing Agent');
  if (!body.stream || body.store === false)
    throw new ProtocolError('Responses require stream: true and durable storage');
  if (typeof body.input === 'string') requireText(body.input);
  else {
    const input = one(body.input);
    if (
      input.role !== 'user' ||
      (input.type !== undefined && input.type !== 'message') ||
      Object.keys(input).some((key) => !['role', 'type', 'content'].includes(key))
    )
      throw new ProtocolError('Responses input must be exactly one user message');
    if (typeof input.content === 'string') requireText(input.content);
    else {
      const content = one(input.content);
      if (
        content.type !== 'input_text' ||
        Object.keys(content).some((key) => !['type', 'text'].includes(key))
      )
        throw new ProtocolError('Responses content must be input_text');
      requireText(content.text);
    }
  }
  const metadata = Object.entries(body.metadata ?? {});
  if (
    metadata.length > 16 ||
    metadata.some(([key, item]) => [...key].length > 64 || [...item].length > 512)
  )
    throw new ProtocolError('Responses metadata exceeds its count or character boundary');
  return value as CreateResponseRequest;
}

function one(value: unknown) {
  if (!Array.isArray(value) || value.length !== 1)
    throw new ProtocolError('expected exactly one text message or content part');
  return record(value[0]);
}

function requireText(value: unknown) {
  if (!text(value).length) throw new ProtocolError('Responses input text cannot be empty');
}
