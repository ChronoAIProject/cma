import { createClient, type ResponseObject } from '../index.js';
import { vi } from 'vitest';

export const agentId = 'agt_responses';
export const responseId = `resp_${agentId}_1`;
export const request = {
  conversation: agentId,
  input: 'Review the project',
  stream: true,
} as const;
export const response: ResponseObject = {
  id: responseId,
  object: 'response',
  created_at: 1,
  status: 'in_progress',
  background: false,
  error: null,
  incomplete_details: null,
  instructions: null,
  max_output_tokens: null,
  model: 'profile-model',
  output: [],
  parallel_tool_calls: false,
  previous_response_id: null,
  reasoning: { effort: null, summary: null },
  store: true,
  temperature: null,
  text: { format: { type: 'text' }, verbosity: 'medium' },
  tool_choice: 'none',
  tools: [],
  top_p: null,
  truncation: 'disabled',
  usage: null,
  metadata: {},
};
export const completed = { ...response, status: 'completed' } as const;
export function frame(type: string, sequence_number: number, body: Record<string, unknown>) {
  return `event: ${type}\ndata: ${JSON.stringify({ type, sequence_number, ...body })}\n\n`;
}
export const opening = frame('response.created', 0, { response });
export const ending = frame('response.completed', 1, { response: completed });
export function sse(body: BodyInit = opening + ending) {
  return new Response(body, { headers: { 'content-type': 'text/event-stream; charset=utf-8' } });
}
export function harness() {
  const fetch = vi.fn<typeof globalThis.fetch>();
  const client = createClient({ baseUrl: 'https://cma.example', fetch });
  const onEvent = vi.fn();
  return { fetch, client, onEvent };
}
