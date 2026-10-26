import { vi } from 'vitest';
import { createClient } from '../src/index.js';

export const agent = 'agt_fixture';
export const incarnation = 'a'.repeat(64);
export const cursor = (sequence: number) => `ag1.${incarnation}.${sequence}`;
export const pageCursor = (through: number, after: number) =>
  `ags1.${incarnation}.${through}.${after}`;
export const eligibility = () => ({
  object: 'cma.ui.transport',
  schema_version: 1,
  enabled: false,
  selection: 'new_tabs',
  input: 'responses_v2',
  snapshot_schema: 1,
  event_schema: 1,
  control_schema: 1,
  live_fragments: true,
  snapshot: '/api/v2/agents/{agent_id}/events/snapshot',
  events: '/api/v2/agents/{agent_id}/events',
  controls: '/api/v2/agents/{agent_id}/responses/{response_id}/controls',
  command_status: '/api/v2/commands/{command_id}',
});
export const item = (sequence: number, kind: unknown = null) => ({
  cursor: cursor(sequence),
  sequence,
  appended_at: '2026-09-16T00:00:00Z',
  kind,
  response_binding: null as unknown,
});
export const page = (through = 2, after = 0, count = through - after) => ({
  object: 'cma.agent.event_page',
  schema_version: 1,
  agent_id: agent,
  incarnation,
  through_cursor: cursor(through),
  items: Array.from({ length: count }, (_, index) => item(after + index + 1)),
  next_page_cursor: after + count < through ? pageCursor(through, after + count) : null,
});
export const durable = (sequence: number, kind: unknown = null) => ({
  ...item(sequence, kind),
  agent_id: agent,
  incarnation,
});
export const live = () => ({
  agent_id: agent,
  incarnation,
  turn_sequence: 1,
  after_sequence: 2,
  message_id: 'msg_1',
  kind: 'reasoning_summary_delta',
  delta: 'ephemeral only',
});
export const gap = () => ({
  agent_id: agent,
  incarnation,
  turn_sequence: 1,
  frames: null,
  reason: 'reconnected',
});
export const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { 'content-type': 'application/json' } });
export const frame = (event: string, data: unknown, id?: string) =>
  `${id === undefined ? '' : `id: ${id}\n`}event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
export function sse(body: string | Uint8Array, split?: number) {
  const bytes = typeof body === 'string' ? new TextEncoder().encode(body) : body;
  return new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        if (split)
          for (let offset = 0; offset < bytes.length; offset += split)
            controller.enqueue(bytes.slice(offset, offset + split));
        else controller.enqueue(bytes);
        controller.close();
      },
    }),
    { headers: { 'content-type': 'text/event-stream' } },
  );
}
export function harness() {
  const fetch = vi.fn<typeof globalThis.fetch>();
  const client = createClient({
    baseUrl: 'https://cma.example/gateway',
    fetch,
    credential: async () => 'secret',
    requestIdProvider: () => 'request-agent',
  });
  return { fetch, api: client.agentEventsV2 };
}
export const kinds = [
  { type: 'created', workspace_id: 'wrk', worktree_id: 'wkt' },
  { type: 'conversation_available' },
  { type: 'runtime_changed' },
  { type: 'work_completed', step: 'publish_ready' },
  { type: 'work_completed', step: 'install_skill' },
  { type: 'skill_installation_changed', skill_id: 'skill-1', status: 'installed' },
  {
    type: 'work_failed',
    step: 'open_conversation',
    failure: { class: 'runtime', detail: 'unavailable' },
  },
  { type: 'turn_started', turn_sequence: 1 },
  { type: 'turn_metadata', turn_sequence: 1, metadata: { model: 'test' } },
  { type: 'message', role: 'assistant', content: 'kept' },
  { type: 'turn_completed', turn_sequence: 1 },
  { type: 'turn_failed', turn_sequence: 1, failure: { class: 'runtime', detail: 'safe' } },
  { type: 'turn_cancelled', turn_sequence: 1 },
  { type: 'tool_started', call_id: 'call', title: 'Working', kind: 'read', status: 'pending' },
  { type: 'tool_updated', call_id: 'call', status: 'completed', title: null },
  { type: 'approval_requested', approval_id: 'approval', action: null, detail: null },
  { type: 'approval_resolved', approval_id: 'approval', decision: 'allow' },
  {
    type: 'delivery_changed',
    operation_sequence: 1,
    operation: 'send_message',
    state: 'write_confirmed',
  },
  { type: 'usage', input_tokens: 1, output_tokens: 2, context_used: 3, context_window: null },
  { type: 'mode_changed', mode: 'code' },
  { type: 'plan_set', entries: [{ title: 'Check', status: 'completed' }] },
  { type: 'config_options_set', options: [{ id: 'mode', name: 'Mode', kind: 'select' }] },
  { type: 'available_commands_set', commands: [{ name: 'review', description: 'Review' }] },
  { type: 'history_gap', frames: 2 },
  { type: 'authentication_required' },
  { type: 'status_changed', from: 'working', to: 'ready' },
  { type: 'first_message_changed', state: 'completed' },
  { type: 'title_set', title: 'Title', source: 'runtime' },
  { type: 'stopped', reason: 'requested' },
  { type: 'deleted' },
];
