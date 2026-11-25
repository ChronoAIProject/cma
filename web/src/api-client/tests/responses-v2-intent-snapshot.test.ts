// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { createClient, ValidationError } from '../index.js';
import { agentId, completed, request, responseId, sse } from './responses-v2-fixtures.js';

const alias = `/api/v2/agents/${agentId}/responses`;

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

it.each([
  ['create', 'request ID', '/api/v2/responses'],
  ['create', 'credentials', '/api/v2/responses'],
  ['createForAgent', 'request ID', alias],
  ['createForAgent', 'credentials', alias],
] as const)('captures %s input before awaiting %s', async (method, callback, route) => {
  const entered = deferred<void>();
  const release = deferred<string>();
  const delayed = () => {
    entered.resolve();
    return release.promise;
  };
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(sse());
  const { responsesV2 } = createClient({
    fetch,
    requestIdProvider: callback === 'request ID' ? delayed : () => 'request-original',
    credential: callback === 'credentials' ? delayed : undefined,
  });
  const part = { type: 'input_text' as const, text: 'Read the report' };
  const body = {
    ...request,
    conversation: { id: agentId },
    input: [{ role: 'user', content: [part] }] as const,
    metadata: { caption: 'original' },
  };
  const original = JSON.stringify(body);
  const onReceipt = vi.fn();
  const options = { onReceipt, onEvent: vi.fn(), idempotencyKey: 'intent-original' };
  const result =
    method === 'create'
      ? responsesV2.create(body, options)
      : responsesV2.createForAgent(agentId, body, options);

  await entered.promise;
  expect(fetch).not.toHaveBeenCalled();
  body.conversation.id = 'agt_other';
  part.text = 'Changed after submit';
  body.metadata.caption = 'changed';
  options.idempotencyKey = 'intent-changed';
  release.resolve(callback === 'request ID' ? 'request-original' : 'synthetic');

  await expect(result).resolves.toEqual(completed);
  expect(fetch).toHaveBeenCalledOnce();
  const [url, init] = fetch.mock.calls[0]!;
  expect(String(url)).toBe(route);
  expect(init?.method).toBe('POST');
  expect(init?.body).toBe(original);
  expect(new Headers(init?.headers).get('Idempotency-Key')).toBe('intent-original');
  expect(new Headers(init?.headers).get('X-Request-Id')).toBe('request-original');
  expect(onReceipt).toHaveBeenCalledExactlyOnceWith({ responseId, commandId: 'cmd_opaque' });
});

it.each([
  ['an unsupported undefined field', { ...request, future_field: undefined }, /unsupported/],
  ['a mismatched Agent alias', { ...request, conversation: 'agt_other' }, /match the Agent alias/],
  [
    'a conversation that serializes to another Agent',
    {
      ...request,
      conversation: Object.defineProperty({ id: agentId }, 'toJSON', {
        value: () => ({ id: 'agt_other' }),
      }),
    },
    /match the Agent alias/,
  ],
] as const)('rejects %s before requesting credentials or sending', async (_case, body, pattern) => {
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(sse());
  const requestIdProvider = vi.fn(() => 'request-original');
  const credential = vi.fn(() => 'synthetic');
  const { responsesV2 } = createClient({ fetch, requestIdProvider, credential });

  const result = responsesV2.createForAgent(agentId, body, { onEvent: vi.fn() });
  await expect(result).rejects.toBeInstanceOf(ValidationError);
  await expect(result).rejects.toThrow(pattern);
  expect(requestIdProvider).not.toHaveBeenCalled();
  expect(credential).not.toHaveBeenCalled();
  expect(fetch).not.toHaveBeenCalled();
});
