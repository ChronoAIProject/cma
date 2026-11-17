import { response as legacy, agentId } from './responses-fixtures.js';
export { harness, agentId, request } from './responses-fixtures.js';
export const responseId = 'resp_opaque';
export const response = {
  ...legacy,
  id: responseId,
  status: 'queued',
  conversation: { id: agentId },
  completed_at: null,
};
export const part = { type: 'output_text', text: 'Hello 世界', annotations: [], logprobs: [] };
export const item = {
  type: 'message',
  id: 'msg_committed',
  status: 'completed',
  role: 'assistant',
  content: [part],
};
export const completed = { ...response, status: 'completed', completed_at: 2, output: [item] };
export function events(): Record<string, unknown>[] {
  return [
    { type: 'response.created', response },
    { type: 'response.in_progress', response: { ...response, status: 'in_progress' } },
    {
      type: 'response.output_item.added',
      output_index: 0,
      item: { ...item, status: 'in_progress', content: [] },
    },
    {
      type: 'response.content_part.added',
      output_index: 0,
      content_index: 0,
      item_id: item.id,
      part: { ...part, text: '' },
    },
    {
      type: 'response.output_text.delta',
      output_index: 0,
      content_index: 0,
      item_id: item.id,
      delta: part.text,
      logprobs: [],
    },
    {
      type: 'response.output_text.done',
      output_index: 0,
      content_index: 0,
      item_id: item.id,
      text: part.text,
      logprobs: [],
    },
    {
      type: 'response.content_part.done',
      output_index: 0,
      content_index: 0,
      item_id: item.id,
      part,
    },
    { type: 'response.output_item.done', output_index: 0, item },
    { type: 'response.completed', response: completed },
  ].map((event, sequence_number) => ({ ...event, sequence_number }));
}
export function wire(values = events()) {
  return values
    .map(
      (event) =>
        `${event.type === 'error' ? '' : `id: ${String(event.sequence_number)}\n`}event: ${String(event.type)}\ndata: ${JSON.stringify(event)}\n\n`,
    )
    .join('');
}
export function sse(body: BodyInit = wire(), override: Record<string, string> = {}) {
  return new Response(body, {
    headers: {
      'content-type': 'text/event-stream',
      'x-cma-response-id': responseId,
      'x-cma-command-id': 'cmd_opaque',
      ...override,
    },
  });
}
