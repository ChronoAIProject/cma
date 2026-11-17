import { vi } from 'vitest';
import { createClient } from '../index.js';
import { loadCorpus } from './public-contract/context.js';

const cases = loadCorpus().filter((item) => item.id.startsWith('response-controls-v2-'));
export const agent = 'agt_fixture';
export const response = 'resp_control_source';
export const route = `/api/v2/agents/${agent}/responses/${response}/controls`;
export function fixture(id: string): Record<string, unknown> {
  return structuredClone(
    cases.find((item) => item.id === `response-controls-v2-${id}`)!.response.body,
  ) as Record<string, unknown>;
}
export const intent = {
  turn_sequence: 4,
  expected_agent_revision: 4,
  command: { type: 'cancel' },
} as const;
export function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}
export function harness() {
  const fetch = vi.fn<typeof globalThis.fetch>();
  return {
    fetch,
    api: createClient({ baseUrl: 'https://cma.example/gateway', fetch }).responsesV2,
  };
}
