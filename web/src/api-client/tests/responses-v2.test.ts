// @vitest-environment node
import { describe, expect, it, vi } from 'vitest';
import {
  ApiError,
  ProtocolError,
  TransportError,
  ValidationError,
  errorFromResponse,
  type TaskAcceptanceIntent,
  type V2ResponseEvent,
} from '../index.js';
import {
  response,
  responseId,
  completed,
  events,
  wire,
  sse,
  harness,
  agentId,
  request,
  item,
} from './responses-v2-fixtures.js';

describe('Responses v2 durable streams', () => {
  const declaration = {
    version: 1,
    action: {
      type: 'declare',
      requirements: [
        { id: 'plan', required: true, evidence: { kind: 'plan_step', title: 'Write report' } },
        {
          id: 'output',
          required: true,
          evidence: {
            kind: 'deliverable',
            resource: { kind: 'live_file', path: 'out/report.csv' },
          },
        },
      ],
    },
  } as const satisfies TaskAcceptanceIntent;
  const retry = {
    version: 1,
    action: { type: 'retry', task_id: 'cmd_original', after_turn_sequence: 3 },
  } as const satisfies TaskAcceptanceIntent;

  it.each([declaration, retry])(
    'sends the exact versioned task action through the SDK',
    async (task) => {
      const t = harness();
      t.fetch.mockResolvedValueOnce(sse());
      const body = { input: 'Write the report', stream: true, task_acceptance: task } as const;
      const options = { onEvent: t.onEvent, idempotencyKey: 'contract-key' };
      await expect(t.client.responsesV2.createForAgent(agentId, body, options)).resolves.toEqual(
        completed,
      );
      expect(t.fetch).toHaveBeenCalledOnce();
      const [url, init] = t.fetch.mock.calls[0]!;
      expect(String(url)).toBe(`https://cma.example/api/v2/agents/${agentId}/responses`);
      expect(new Headers(init?.headers).get('idempotency-key')).toBe('contract-key');
      expect(JSON.parse(String(init?.body))).toEqual(body);
    },
  );

  it.each([
    { task_acceptance: { ...declaration, version: 2 } },
    { task_acceptance: { ...declaration, action: { ...declaration.action, requirements: [] } } },
    { task_acceptance: { ...retry, action: { ...retry.action, after_turn_sequence: 0 } } },
    { task_acceptance: { ...retry, action: { ...retry.action, future_field: true } } },
    { task_acceptance: declaration, future_field: true },
  ])('rejects invalid task requests before calling the transport', async (fields) => {
    const t = harness();
    const body = {
      input: 'Write the report',
      stream: true,
      ...fields,
    } as unknown as typeof request;
    await expect(
      t.client.responsesV2.createForAgent(agentId, body, { onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(ValidationError);
    expect(t.fetch).not.toHaveBeenCalled();
  });

  it('retains the receipt before any frame and sends the same stable key through both aliases', async () => {
    const t = harness();
    const order: string[] = [];
    const options = {
      idempotencyKey: 'retained-key',
      onReceipt: (receipt: unknown) => {
        expect(receipt).toEqual({ responseId, commandId: 'cmd_opaque' });
        order.push('receipt');
      },
      onEvent: (event: V2ResponseEvent) => {
        order.push(event.type);
      },
    };
    t.fetch.mockResolvedValueOnce(sse()).mockResolvedValueOnce(sse());
    expect(
      await t.client.responsesV2.create({ ...request, conversation: { id: agentId } }, options),
    ).toEqual(completed);
    expect(order).toEqual(['receipt', ...events().map((event) => event.type)]);
    expect(
      await t.client.responsesV2.createForAgent(agentId, { input: 'Hello', stream: true }, options),
    ).toEqual(completed);
    expect(t.fetch.mock.calls.map(([url]) => String(url))).toEqual([
      'https://cma.example/api/v2/responses',
      `https://cma.example/api/v2/agents/${agentId}/responses`,
    ]);
    for (const [, init] of t.fetch.mock.calls)
      expect(new Headers(init!.headers).get('idempotency-key')).toBe('retained-key');
  });
  it.each(Array.from({ length: 9 }, (_, index) => index))(
    'replays after durable cursor %s, including mid-message and completed EOF',
    async (startingAfter) => {
      const t = harness();
      t.fetch.mockResolvedValueOnce(sse(wire(events().slice(startingAfter + 1))));
      await t.client.responsesV2.replay(responseId, { startingAfter, onEvent: t.onEvent });
      expect(t.onEvent.mock.calls.map(([event]) => event)).toEqual(
        events().slice(startingAfter + 1),
      );
      expect(String(t.fetch.mock.calls[0]![0])).toContain(
        `starting_after=${String(startingAfter)}`,
      );
    },
  );
  it('decodes every UTF-8 byte and split CRLF without changing sequence or text', async () => {
    const t = harness();
    const bytes = new TextEncoder().encode(wire().replaceAll('\n', '\r\n'));
    t.fetch.mockResolvedValueOnce(
      sse(
        new ReadableStream({
          start(controller) {
            for (const byte of bytes) controller.enqueue(new Uint8Array([byte]));
            controller.close();
          },
        }),
      ),
    );
    expect(await t.client.responsesV2.create(request, { onEvent: t.onEvent })).toEqual(completed);
  });
  it('recovers premature EOF from the acknowledged cursor with one POST', async () => {
    const t = harness();
    const receipt = vi.fn();
    const recovery = vi.fn();
    t.fetch
      .mockResolvedValueOnce(sse(wire(events().slice(0, 5)), { 'x-request-id': 'post-read' }))
      .mockResolvedValueOnce(sse(wire(events().slice(5))));
    await expect(
      t.client.responsesV2.create(request, {
        idempotencyKey: 'retained-key',
        onReceipt: receipt,
        onRecovery: recovery,
        onEvent: t.onEvent,
      }),
    ).resolves.toEqual(completed);
    expect(receipt).toHaveBeenCalledExactlyOnceWith({ responseId, commandId: 'cmd_opaque' });
    expect(recovery).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({
        initiatingRequestId: 'post-read',
        interruptedRequestId: 'post-read',
        startingAfter: 4,
        attempt: 1,
        reason: 'premature_eof',
      }),
    );
    expect(t.fetch.mock.calls.map(([, init]) => init!.method)).toEqual(['POST', 'GET']);
    expect(String(t.fetch.mock.calls[1]![0])).toContain(
      `/responses/${responseId}/events?starting_after=4`,
    );
    expect(new Headers(t.fetch.mock.calls[1]![1]!.headers).has('idempotency-key')).toBe(false);
    expect(t.onEvent.mock.calls.map(([event]) => event.sequence_number)).toEqual([
      0, 1, 2, 3, 4, 5, 6, 7, 8,
    ]);
  });
  it('resumes after an in-band observation error without acknowledging it as durable', async () => {
    const t = harness();
    const error = {
      type: 'error',
      sequence_number: 1,
      code: 'response_observation_unavailable',
      message: 'Reconnect',
      param: null,
    };
    const recovery = vi.fn();
    t.fetch
      .mockResolvedValueOnce(
        sse(wire([events()[0]!, error]), { 'x-request-id': 'observation-read' }),
      )
      .mockResolvedValueOnce(sse(wire(events().slice(1))));
    await expect(
      t.client.responsesV2.create(request, { onEvent: t.onEvent, onRecovery: recovery }),
    ).resolves.toEqual(completed);
    expect(t.onEvent.mock.calls.map(([event]) => event)).toEqual(events());
    expect(recovery).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({
        reason: 'observation_error',
        startingAfter: 0,
        initiatingRequestId: 'observation-read',
      }),
    );
    expect(t.fetch.mock.calls.map(([, init]) => init!.method)).toEqual(['POST', 'GET']);
  });
  it('preserves explicit replay connection errors', async () => {
    const t = harness();
    const error = {
      type: 'error',
      sequence_number: 1,
      code: 'response_observation_unavailable',
      message: 'Reconnect',
      param: null,
    };
    t.fetch.mockResolvedValueOnce(sse(wire([error])));
    await expect(
      t.client.responsesV2.replay(responseId, { startingAfter: 0, onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(TransportError);
    expect(t.onEvent.mock.calls.map(([event]) => event.type)).toEqual(['error']);
    expect(t.fetch).toHaveBeenCalledOnce();
  });
  it.each([
    [0, { response: { ...response, id: 'resp_foreign' } }],
    [0, { response: { ...response, conversation: { id: 'agt_foreign' } } }],
    [0, { response: { ...response, status: 'in_progress' } }],
    [0, { response: { ...response, output: [item] } }],
    [1, { type: 'response.completed', response: { ...completed, output: [] } }],
    [2, { output_index: 2 }],
    [3, { item_id: 'different' }],
    [4, { content_index: 1 }],
    [5, { text: 'changed' }],
    [6, { part: { type: 'output_text', text: 'changed', annotations: [], logprobs: [] } }],
    [7, { item: { ...item, status: 'in_progress' } }],
    [8, { response: { ...completed, output: [] } }],
  ] as const)('rejects inconsistent event %s', async (index, override) => {
    const t = harness();
    const frames = events();
    frames[index] = { ...frames[index], ...override };
    t.fetch.mockResolvedValueOnce(sse(wire(frames)));
    await expect(
      t.client.responsesV2.create(request, { onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
  it.each([
    (body: string) => body.replace('id: 0', 'id: 7'),
    (body: string) => body.replace('id: 0\n', ''),
    (body: string) => body.replace('event: response.created', 'event: response.failed'),
    (body: string) => body + wire([events()[8]!]),
  ])('rejects malformed durable SSE envelopes', async (mutate) => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(sse(mutate(wire())));
    await expect(
      t.client.responsesV2.create(request, { onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
  it('refuses missing receipts and forged replay identity', async () => {
    const t = harness();
    t.fetch
      .mockResolvedValueOnce(sse(wire(), { 'x-cma-command-id': '' }))
      .mockResolvedValueOnce(sse());
    await expect(
      t.client.responsesV2.create(request, { onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(ProtocolError);
    await expect(
      t.client.responsesV2.replay('resp_other', { onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
  it('does not resubmit after receipt or consumer failure', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(sse()).mockResolvedValueOnce(sse());
    const fail = () => {
      throw new Error('consumer stopped');
    };
    await expect(
      t.client.responsesV2.create(request, { onEvent: t.onEvent, onReceipt: fail }),
    ).rejects.toBeInstanceOf(ProtocolError);
    await expect(t.client.responsesV2.create(request, { onEvent: fail })).rejects.toBeInstanceOf(
      ProtocolError,
    );
    expect(t.fetch).toHaveBeenCalledTimes(2);
  });
  it('validates Unicode metadata and adjacent text parts without mutating the request', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(sse());
    const body = {
      ...request,
      metadata: { ['界'.repeat(64)]: '🙂'.repeat(512) },
      input: [
        {
          role: 'user',
          content: [
            { type: 'input_text', text: 'Hello ' },
            { type: 'input_text', text: '世界' },
          ],
        },
      ],
    } as const;
    await t.client.responsesV2.create(body, { onEvent: t.onEvent });
    expect(JSON.parse(String(t.fetch.mock.calls[0]![1]!.body))).toEqual(body);
  });
  it.each(['', 'x'.repeat(257), 'line\nbreak'])(
    'validates idempotency key %j before sending',
    async (idempotencyKey) => {
      const t = harness();
      await expect(
        t.client.responsesV2.create(request, { idempotencyKey, onEvent: t.onEvent }),
      ).rejects.toBeInstanceOf(ValidationError);
      expect(t.fetch).not.toHaveBeenCalled();
    },
  );
  it.each([-1, 1.5, Number.MAX_SAFE_INTEGER])(
    'refuses invalid replay cursor %s before HTTP',
    async (startingAfter) => {
      const t = harness();
      await expect(
        t.client.responsesV2.replay(responseId, { startingAfter, onEvent: t.onEvent }),
      ).rejects.toBeInstanceOf(ValidationError);
      expect(t.fetch).not.toHaveBeenCalled();
    },
  );
  it('preserves nested HTTP errors, parameters, correlation and Retry-After', async () => {
    const error = await errorFromResponse(
      new Response(
        JSON.stringify({
          error: {
            type: 'server_error',
            code: 'response_unavailable',
            message: 'Retry the same key',
            param: null,
          },
        }),
        { status: 503, headers: { 'retry-after': '1', 'x-request-id': 'req_v2' } },
      ),
    );
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({
      code: 'response_unavailable',
      status: 503,
      retryAfterMs: 1000,
      requestId: 'req_v2',
      extensions: { type: 'server_error', param: null },
    });
  });
});

it('preserves legacy REST error extensions even when they are named error', async () => {
  const error = await errorFromResponse(
    new Response(
      JSON.stringify({
        status: 409,
        code: 'legacy_conflict',
        detail: 'Legacy detail',
        error: { diagnostic: 'retained' },
      }),
      { status: 409 },
    ),
  );
  expect(error).toMatchObject({
    code: 'legacy_conflict',
    extensions: { error: { diagnostic: 'retained' } },
  });
});
