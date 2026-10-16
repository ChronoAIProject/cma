import { expect, it, vi } from 'vitest';
import {
  ProtocolError,
  TransportError,
  ValidationError,
  type V2ResponseInterruptionRequest,
} from '../src/index.js';
import { loadCorpus } from './public-contract/context.js';
import { agent, response, harness, json } from './response-controls-v2-fixtures.js';

const cases = loadCorpus();
const admitted = () =>
  structuredClone(
    cases.find((item) => item.id === 'interruption-v2-admit')!.response.body,
  ) as Record<string, unknown>;
const intent = {
  turn_sequence: 4,
  expected_agent_revision: 4,
  text: '  New turn\n<attachment>kept</attachment>  ',
};

it('admits the exact interruption and explicitly recovers the same input, target and key after a lost receipt', async () => {
  const t = harness();
  t.fetch
    .mockRejectedValueOnce(new Error('lost reply'))
    .mockResolvedValueOnce(json(admitted(), 202));
  const options = { idempotencyKey: 'retained-original' };
  await expect(t.api.interruptWithInput(agent, response, intent, options)).rejects.toBeInstanceOf(
    TransportError,
  );
  expect(t.fetch).toHaveBeenCalledOnce();
  const result = await t.api.interruptWithInput(agent, response, intent, options);
  expect(result.response.receipt.response_id).not.toBe(response);
  expect(result.response.receipt.command_id).not.toBe(result.control.receipt.command_id);
  for (const [url, request] of t.fetch.mock.calls) {
    expect(url).toBe(
      `https://cma.example/gateway/api/v2/agents/${agent}/responses/${response}/interrupt`,
    );
    expect(request!.method).toBe('POST');
    expect(JSON.parse(String(request!.body))).toEqual(intent);
    expect(new Headers(request!.headers).get('idempotency-key')).toBe(options.idempotencyKey);
  }
  expect(t.fetch.mock.calls[0]![1]!.body).toBe(t.fetch.mock.calls[1]![1]!.body);
});

it('captures mutable input before network or credentials can yield', async () => {
  const t = harness();
  let release!: (response: Response) => void;
  t.fetch.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  );
  const body = { ...intent };
  const pending = t.api.interruptWithInput(agent, response, body, { idempotencyKey: 'retained' });
  body.text = 'changed';
  body.expected_agent_revision = 99;
  await vi.waitFor(() => expect(t.fetch).toHaveBeenCalledOnce());
  release(json(admitted(), 202));
  await pending;
  expect(JSON.parse(String(t.fetch.mock.calls[0]![1]!.body))).toEqual(intent);
});

it.each([
  { ...intent, text: ' ' },
  { ...intent, text: '界'.repeat(90_000) },
  { ...intent, turn_sequence: 0 },
  { ...intent, expected_agent_revision: Number.MAX_SAFE_INTEGER + 1 },
  { ...intent, actor: 'forged' },
])('refuses invalid interruption before HTTP %#', async (body) => {
  const t = harness();
  await expect(
    t.api.interruptWithInput(agent, response, body as V2ResponseInterruptionRequest, {
      idempotencyKey: 'retained',
    }),
  ).rejects.toBeInstanceOf(ValidationError);
  expect(t.fetch).not.toHaveBeenCalled();
});

it.each(['', ' key', 'key ', 'key\n', 'k'.repeat(257)])(
  'refuses a changed or missing key: %s',
  async (key) => {
    const t = harness();
    await expect(
      t.api.interruptWithInput(agent, response, intent, { idempotencyKey: key }),
    ).rejects.toBeInstanceOf(ValidationError);
    expect(t.fetch).not.toHaveBeenCalled();
  },
);

it.each([
  ['target.response_id', 'other'],
  ['target.turn_sequence', 8],
  ['expected_agent_revision', 9],
  ['control.receipt.agent_id', 'other'],
  ['control.receipt.response_id', response],
  ['control.turn_sequence', 8],
  ['response.receipt.agent_id', 'other'],
  ['response.receipt.response_id', null],
  ['response.receipt.response_id', response],
  ['response.receipt.agent_order', 99],
  ['response.failure', 'secret'],
  ['response.extra', 'hidden'],
])('rejects a malformed or unrelated interruption pair: %s', async (path, value) => {
  const body = admitted();
  const parts = String(path).split('.');
  let selected = body;
  for (const part of parts.slice(0, -1)) selected = selected[part] as Record<string, unknown>;
  selected[parts.at(-1)!] = value;
  const t = harness();
  t.fetch.mockResolvedValueOnce(json(body, 202));
  await expect(
    t.api.interruptWithInput(agent, response, intent, { idempotencyKey: 'retained' }),
  ).rejects.toBeInstanceOf(ProtocolError);
});
