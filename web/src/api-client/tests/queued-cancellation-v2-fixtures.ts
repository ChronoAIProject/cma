import {
  fixture as controlsFixture,
  harness,
  json,
  agent,
  response,
} from './response-controls-v2-fixtures.js';
export { harness, json, agent, response };
export const route = `/api/v2/agents/${agent}/responses/${response}/queued-cancellation`;
export const intent = { source_command_id: 'cmd_control_source', expected_command_revision: 1 };
export function observation(cancellable = true) {
  return {
    object: 'cma.response.queued_cancellation.observation',
    response: structuredClone(controlsFixture('queued-observe').response),
    command_revision: 1,
    cancellable,
  };
}
export function receipt(): Record<string, unknown> {
  const source = controlsFixture('queued-observe').response as Record<string, unknown>;
  return {
    object: 'cma.response.queued_cancellation',
    cancellation: {
      ...structuredClone(source),
      receipt: {
        ...(structuredClone(source.receipt) as Record<string, unknown>),
        command_id: 'cmd_queued_cancellation',
        response_id: null,
        agent_order: 2,
      },
      status: 'completed',
    },
    response: { ...source, status: 'cancelled' },
    target: { response_id: response, ...intent },
  };
}
export function set(body: Record<string, unknown>, path: string, value: unknown) {
  const parts = path.split('.');
  let selected = body;
  for (const part of parts.slice(0, -1)) selected = selected[part] as Record<string, unknown>;
  selected[parts.at(-1)!] = value;
}
