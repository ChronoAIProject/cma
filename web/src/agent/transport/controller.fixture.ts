import { vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { agentClient } from '../../test/agent';
import { createAgentTransport } from './client';
import { commandStatus, controlReceipt, observation } from './controls.fixture';
import { queuedObservation, queuedReceipt } from './queued.fixture';

export const incarnation = 'a'.repeat(64);
export const cursor = (seq: number) => `ag1.${incarnation}.${seq}`;
export const binding = { response_id: 'resp_1', command_id: 'cmd_1', turn_sequence: 1 };
export const at = '2026-09-16T00:00:00Z';
export const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { 'content-type': 'application/json' },
  });
export function page(items: unknown[], through: number, after?: number) {
  return {
    object: 'cma.agent.event_page',
    schema_version: 1,
    agent_id: 'agt_1',
    incarnation,
    through_cursor: cursor(through),
    items,
    next_page_cursor: after === undefined ? null : `ags1.${incarnation}.${through}.${after}`,
  };
}
export function item(sequence: number, kind: unknown, response_binding: unknown = null) {
  return { cursor: cursor(sequence), sequence, appended_at: at, kind, response_binding };
}
export function accepted() {
  return new Response(new ReadableStream(), {
    headers: {
      'content-type': 'text/event-stream',
      'x-cma-response-id': 'resp_1',
      'x-cma-command-id': 'cmd_1',
    },
  });
}
export function queuedPage(text = 'Cancel this waiting message', revision = 7) {
  return {
    object: 'cma.agent.queued_inputs',
    agent_id: 'agt_1',
    data: [
      {
        response: queuedObservation('resp_1', revision).response,
        command_revision: revision,
        text,
        editable: true,
      },
    ],
  };
}
export function harness() {
  let stream: ReadableStreamDefaultController<Uint8Array> | undefined;
  const post = vi.fn<(init?: RequestInit) => Promise<Response>>().mockResolvedValue(accepted());
  const snapshot = vi
    .fn<(url: string) => Promise<Response>>()
    .mockImplementation(async () => json(page([], 0)));
  const cancel = vi.fn();
  const currentUser = vi.fn(async () => 'alice' as string | undefined);
  const listQueued = vi.fn(async () =>
    json({ object: 'cma.agent.queued_inputs', agent_id: 'agt_1', data: [] }),
  );
  const observeQueued = vi.fn(async (responseId: string) =>
    json(queuedObservation(responseId, 7, false)),
  );
  const cancelQueued = vi.fn(async (responseId: string, init?: RequestInit) =>
    json(queuedReceipt(responseId, JSON.parse(String(init?.body)))),
  );
  const observe = vi.fn(async (responseId: string) => json(observation(responseId)));
  const control = vi.fn(async (responseId: string, init?: RequestInit) =>
    json(controlReceipt(responseId, JSON.parse(String(init?.body))), 202),
  );
  const command = vi.fn(async () =>
    json(
      commandStatus(
        null,
        Number(JSON.parse(String(control.mock.lastCall?.[1]?.body)).turn_sequence),
        true,
      ),
    ),
  );
  const eligibility = vi.fn(async () =>
    json({
      object: 'cma.ui.transport',
      schema_version: 1,
      enabled: true,
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
    }),
  );
  const fetch = vi.fn<typeof globalThis.fetch>(async (input, init) => {
    const url = String(input);
    if (url === '/api/v2/ui/transport') return eligibility();
    if (url === '/api/v2/agents/agt_1/queued-inputs') return listQueued();
    if (url === '/api/v2/commands/cmd_control') return command();
    const queued = /^\/api\/v2\/agents\/agt_1\/responses\/([^/]+)\/queued-cancellation$/.exec(url);
    if (queued)
      return init?.method === 'POST' ? cancelQueued(queued[1]!, init) : observeQueued(queued[1]!);
    const target = /^\/api\/v2\/agents\/agt_1\/responses\/([^/]+)\/controls$/.exec(url);
    if (target) return init?.method === 'POST' ? control(target[1]!, init) : observe(target[1]!);
    if (url.startsWith('/api/v2/agents/agt_1/events/snapshot')) return snapshot(url);
    if (url.startsWith('/api/v2/agents/agt_1/events?'))
      return new Response(
        new ReadableStream({
          start(controller) {
            stream = controller;
          },
          cancel,
        }),
        { headers: { 'content-type': 'text/event-stream' } },
      );
    if (url === '/api/v2/agents/agt_1/responses') return post(init);
    throw new Error(`Unexpected request ${url}`);
  });
  const sdk = createClient({ fetch });
  const client = agentClient({
    agentTransport: createAgentTransport(sdk, { storage: sessionStorage, currentUser }),
    runAgent: vi.fn(),
    interruptAgent: vi.fn(),
    replayAgent: vi.fn(),
    readAgentTranscript: vi.fn(),
    readAgentEvents: vi.fn(),
  });
  const emit = (type: string, data: unknown, id?: string) => {
    if (!stream) throw new Error('No Agent stream');
    stream.enqueue(
      new TextEncoder().encode(
        `${id === undefined ? '' : `id: ${id}\n`}event: ${type}\ndata: ${JSON.stringify(data)}\n\n`,
      ),
    );
  };
  return {
    client,
    sdk,
    fetch,
    post,
    snapshot,
    cancel,
    currentUser,
    listQueued,
    observeQueued,
    cancelQueued,
    observe,
    control,
    command,
    eligibility,
    end() {
      stream?.close();
      stream = undefined;
    },
    fail(code = 'agent_read_unavailable') {
      emit('cma.agent.error', { code, message: 'Temporarily unavailable.' });
    },
    gap(reason: 'lagged' | 'reconnected' = 'lagged', turnSequence: number | null = 1) {
      emit('cma.agent.live_gap', {
        agent_id: 'agt_1',
        incarnation,
        turn_sequence: turnSequence,
        frames: reason === 'reconnected' ? null : 1,
        reason,
      });
    },
    get streaming() {
      return stream !== undefined;
    },
    durable(
      sequence: number,
      kind: unknown,
      response_binding: unknown = null,
      messageCommandId?: string,
    ) {
      emit(
        'cma.agent.event',
        {
          ...item(sequence, kind, response_binding),
          agent_id: 'agt_1',
          incarnation,
          ...(messageCommandId ? { message_command_id: messageCommandId } : {}),
        },
        cursor(sequence),
      );
    },
    live(kind: 'text_delta' | 'reasoning_summary_delta', delta: string, after: number) {
      emit('cma.agent.live', {
        agent_id: 'agt_1',
        incarnation,
        turn_sequence: 1,
        after_sequence: after,
        message_id: kind === 'text_delta' ? 'live_text' : 'live_reasoning',
        kind,
        delta,
      });
    },
  };
}
