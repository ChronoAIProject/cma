import { describe, expect, it } from 'vitest';
import { ProtocolError, TransportError, ValidationError } from '../src/index.js';
import {
  agent,
  response,
  observation,
  receipt,
  harness,
  json,
  set,
  intent as target,
} from './queued-cancellation-v2-fixtures.js';
const intent = { ...target, text: 'Changed text\n<attachment>edited contents</attachment>' };
function page() {
  return {
    object: 'cma.agent.queued_inputs',
    agent_id: agent,
    next_after: null,
    data: [
      { response: observation().response, command_revision: 1, text: 'Original', editable: true },
    ],
  };
}
function accepted() {
  const original = receipt();
  return {
    object: 'cma.response.queued_input',
    edit: original.cancellation,
    response: observation().response,
    target: original.target,
    command_revision: 2,
    text: intent.text,
  };
}
describe('editable authoritative queued input', () => {
  it('lists the ordered server queue without creating work', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(json(page()));
    expect(await t.api.listQueuedInputs(agent)).toEqual(page());
    expect(String(t.fetch.mock.calls[0]![0])).toBe(
      `https://cma.example/gateway/api/v2/agents/${agent}/queued-inputs`,
    );
    expect(t.fetch.mock.calls[0]![1]!.method).toBe('GET');
  });
  it('captures exact text and revision and recovers uncertain edit without automatic retry', async () => {
    const t = harness();
    t.fetch.mockRejectedValueOnce(new Error('lost reply')).mockResolvedValueOnce(json(accepted()));
    await expect(
      t.api.editQueuedInput(agent, response, intent, { idempotencyKey: 'saved-edit' }),
    ).rejects.toBeInstanceOf(TransportError);
    expect(t.fetch).toHaveBeenCalledTimes(1);
    expect(
      await t.api.editQueuedInput(agent, response, intent, { idempotencyKey: 'saved-edit' }),
    ).toEqual(accepted());
    expect(t.fetch.mock.calls[0]![1]!.body).toBe(t.fetch.mock.calls[1]![1]!.body);
    expect(JSON.parse(String(t.fetch.mock.calls[0]![1]!.body))).toEqual(intent);
    expect(new Headers(t.fetch.mock.calls[0]![1]!.headers).get('idempotency-key')).toBe(
      'saved-edit',
    );
  });
  it('rejects response identity, revision and changed payload mismatches', async () => {
    for (const [path, value] of [
      ['target.source_command_id', 'foreign'],
      ['command_revision', 3],
      ['text', 'different'],
      ['response.receipt.agent_id', 'agt_foreign'],
    ]) {
      const t = harness();
      const body = accepted();
      set(body, String(path), value);
      t.fetch.mockResolvedValueOnce(json(body));
      await expect(
        t.api.editQueuedInput(agent, response, intent, { idempotencyKey: 'saved-edit' }),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
  });
  it('rejects invalid content and unknown authority fields before network IO', async () => {
    for (const body of [
      { ...intent, text: '' },
      { ...intent, text: 'x'.repeat(262145) },
      { ...intent, actor: 'forged' },
      { ...intent, expected_command_revision: 0 },
    ]) {
      const t = harness();
      await expect(
        t.api.editQueuedInput(agent, response, body, { idempotencyKey: 'saved-edit' }),
      ).rejects.toBeInstanceOf(ValidationError);
      expect(t.fetch).not.toHaveBeenCalled();
    }
  });
  it('rejects cross-agent, duplicate and out-of-order queue entries', async () => {
    const invalid = page();
    invalid.data.push(structuredClone(invalid.data[0]!));
    for (const body of [invalid, { ...page(), agent_id: 'agt_foreign' }]) {
      const t = harness();
      t.fetch.mockResolvedValueOnce(json(body));
      await expect(t.api.listQueuedInputs(agent)).rejects.toBeInstanceOf(ProtocolError);
    }
  });
});

it('loads all 64 maximum escaped-text inputs through pages below the global 8 MiB cap', async () => {
  const t = harness();
  const text = '\u0001'.repeat(256 * 1024);
  for (let start = 0; start < 64; start += 4) {
    const data = Array.from({ length: 4 }, (_, offset) => {
      const entry = structuredClone(page().data[0]!);
      entry.text = text;
      const order = start + offset + 1;
      set(entry, 'response.receipt.command_id', `cmd_${order}`);
      set(entry, 'response.receipt.response_id', `resp_${order}`);
      set(entry, 'response.receipt.agent_order', order);
      return entry;
    });
    const body = { ...page(), data, next_after: start + 4 < 64 ? start + 4 : null };
    expect(new TextEncoder().encode(JSON.stringify(body)).byteLength).toBeLessThan(8 * 1024 * 1024);
    t.fetch.mockResolvedValueOnce(json(body));
  }
  const result = await t.api.listQueuedInputs(agent);
  expect(result.data).toHaveLength(64);
  expect(result.data.every((entry) => entry.text === text)).toBe(true);
  expect(t.fetch).toHaveBeenCalledTimes(16);
  expect(String(t.fetch.mock.calls[15]![0])).toContain('after=60');
});

it('refuses forged or nonadvancing page cursors instead of returning a partial queue', async () => {
  for (const body of [
    { ...page(), next_after: 7 },
    { ...page(), data: [], next_after: 1 },
  ]) {
    const t = harness();
    t.fetch.mockResolvedValueOnce(json(body));
    await expect(t.api.listQueuedInputs(agent)).rejects.toBeInstanceOf(ProtocolError);
  }
  const t = harness();
  const first = { ...page(), next_after: 1 };
  t.fetch.mockResolvedValueOnce(json(first)).mockResolvedValueOnce(json(page()));
  await expect(t.api.listQueuedInputs(agent)).rejects.toBeInstanceOf(ProtocolError);
  expect(t.fetch).toHaveBeenCalledTimes(2);
});
