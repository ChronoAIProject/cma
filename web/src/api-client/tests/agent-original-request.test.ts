// @vitest-environment node
import { expect, it } from 'vitest';
import { ProtocolError } from '../index.js';
import { createRestWebClient } from '../../api/rest';
import { transport } from './resource-fixtures.js';

const original = {
  status: 'ready',
  agent_id: 'retained-agent',
  original_request: {
    text: '  比较三个方案\nKeep [literal] bytes.\r\n🦊  ',
    source: 'admitted_first_task',
    turn_sequence: null,
  },
};

it('reads original admitted text unchanged even before dispatch, without history scans or writes', async () => {
  const test = transport();
  test.answer(original);
  const result = await createRestWebClient(test.client).readAgentOriginalRequest!(
    original.agent_id,
  );
  expect(result).toEqual({
    kind: 'ready',
    agentId: original.agent_id,
    source: 'admitted-first-task',
    text: original.original_request.text,
    turnSequence: null,
  });
  expect(test.calls).toHaveLength(1);
  expect(test.calls[0]!.url).toBe(
    'https://cma.example.test/api/v1/agents/retained-agent/original-request',
  );
  expect(test.calls[0]!.init).toMatchObject({ method: 'GET', cache: 'no-store' });
  expect(test.calls[0]!.init.body).toBeUndefined();
});

it('preserves explicit unavailability instead of substituting a title, reply or first history page', async () => {
  const test = transport();
  test.answer({ status: 'unavailable', agent_id: original.agent_id, original_request: null });
  expect(
    await createRestWebClient(test.client).readAgentOriginalRequest!(original.agent_id),
  ).toEqual({ kind: 'unavailable', agentId: original.agent_id });
  expect(test.calls).toHaveLength(1);
});

it.each([
  { agent_id: 'other-agent' },
  { status: 'ready', original_request: null },
  { status: 'unavailable' },
  { original_request: { ...original.original_request, source: 'setup' } },
  { original_request: { ...original.original_request, turn_sequence: -1 } },
  { original_request: { ...original.original_request, turn_sequence: 0 } },
  { original_request: { ...original.original_request, text: null } },
])('rejects inconsistent original-request evidence %j', async (change) => {
  const test = transport();
  test.answer({ ...original, ...change });
  await expect(test.client.agents.originalRequest(original.agent_id)).rejects.toBeInstanceOf(
    ProtocolError,
  );
});

it('enforces the 256 KiB UTF-8 boundary without truncating the admitted request', async () => {
  const test = transport();
  const text = '🦊'.repeat(64 * 1024);
  test.answer({ ...original, original_request: { ...original.original_request, text } });
  const result = await test.client.agents.originalRequest(original.agent_id);
  expect(result.original_request?.text).toBe(text);
  test.answer({
    ...original,
    original_request: { ...original.original_request, text: text + 'a' },
  });
  await expect(test.client.agents.originalRequest(original.agent_id)).rejects.toBeInstanceOf(
    ProtocolError,
  );
});

it.each([401, 403, 404, 503])('retains HTTP %i as a read failure', async (status) => {
  const test = transport();
  test.answer({ detail: 'Synthetic refusal' }, status);
  await expect(test.client.agents.originalRequest(original.agent_id)).rejects.toMatchObject({
    status,
  });
  expect(test.calls).toHaveLength(1);
});
