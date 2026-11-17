// @vitest-environment node
import { describe, expect, it, vi } from 'vitest';
import {
  ApiError,
  ProtocolError,
  RequestAborted,
  TransportError,
  ValidationError,
  commandIsTerminal,
  type V2ResponseControlRequest,
} from '../index.js';
import {
  agent,
  response,
  route,
  fixture,
  intent,
  json,
  harness,
} from './response-controls-v2-fixtures.js';

describe('response-targeted controls', () => {
  it('observes active, unsupported and queued targets without claiming operation permission', async () => {
    const t = harness();
    for (const id of ['owner-observe', 'unsupported-observe', 'queued-observe']) {
      const expected = fixture(id);
      t.fetch.mockResolvedValueOnce(json(expected));
      expect(await t.api.observeControls(agent, response)).toEqual(expected);
    }
    for (const [url, options] of t.fetch.mock.calls) {
      expect(String(url)).toBe(`https://cma.example/gateway${route}`);
      expect(options!.method).toBe('GET');
      expect(options!.body).toBeUndefined();
    }
  });
  it('submits every control with its original revision, turn and retained key', async () => {
    const t = harness();
    const commands = [
      intent.command,
      { type: 'steer', text: '  Review\nthis first  ' },
      ...(['allow', 'deny', 'abort'] as const).map((decision) => ({
        type: 'resolve_approval',
        approval_id: 'approval-contract',
        decision,
      })),
    ] as const;
    for (const command of commands) {
      const body = { ...intent, command } as V2ResponseControlRequest;
      t.fetch.mockResolvedValueOnce(json(fixture('cancel'), 202));
      await t.api.submitControl(agent, response, body, { idempotencyKey: 'retained key' });
      const [url, options] = t.fetch.mock.calls.at(-1)!;
      expect(String(url)).toBe(`https://cma.example/gateway${route}`);
      expect(JSON.parse(String(options!.body))).toEqual(body);
      expect(new Headers(options!.headers).get('idempotency-key')).toBe('retained key');
    }
  });
  it('preserves an uncertain admission and explicitly retries the identical old intent', async () => {
    const t = harness();
    t.fetch
      .mockRejectedValueOnce(new Error('lost reply'))
      .mockResolvedValueOnce(json(fixture('cancel-retry'), 202));
    const options = { idempotencyKey: 'persisted-before-send' };
    await expect(t.api.submitControl(agent, response, intent, options)).rejects.toBeInstanceOf(
      TransportError,
    );
    expect(t.fetch).toHaveBeenCalledOnce();
    await t.api.submitControl(agent, response, intent, options);
    expect(t.fetch.mock.calls[0]![1]!.body).toBe(t.fetch.mock.calls[1]![1]!.body);
    expect(new Headers(t.fetch.mock.calls[1]![1]!.headers).get('idempotency-key')).toBe(
      options.idempotencyKey,
    );
  });
  it('captures mutable JavaScript input before awaiting the response', async () => {
    const t = harness();
    let release!: (value: Response) => void;
    t.fetch.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    );
    const body = { ...intent, command: { type: 'steer' as const, text: 'original' } };
    const pending = t.api.submitControl(agent, response, body, { idempotencyKey: 'key' });
    body.expected_agent_revision = 4;
    body.command.text = 'changed after call';
    await vi.waitFor(() => expect(t.fetch).toHaveBeenCalledOnce());
    release(json(fixture('cancel'), 202));
    await pending;
    expect(JSON.parse(String(t.fetch.mock.calls[0]![1]!.body)).command.text).toBe('original');
  });
  it('polls uncertain and completed control delivery without fabricating target cancellation', async () => {
    const t = harness();
    const accepted = fixture('cancel') as { control: Record<string, unknown> };
    for (const delivery of [
      'dispatch_fenced',
      'recovery_required',
      'runtime_lost',
      'write_confirmed',
      'execution_observed',
      'pre_write_failed',
    ]) {
      const command = { ...accepted.control, status: 'in_progress', turn_sequence: 4, delivery };
      t.fetch.mockResolvedValueOnce(json(command));
      const status = await t.api.readCommand('cmd_cancel_contract');
      expect(status.delivery).toBe(delivery);
      expect(commandIsTerminal(status)).toBe(false);
    }
    const completed = {
      ...accepted.control,
      status: 'completed',
      turn_sequence: 4,
      delivery: 'write_confirmed',
    };
    t.fetch
      .mockResolvedValueOnce(json(completed))
      .mockResolvedValueOnce(json(fixture('owner-observe')));
    expect(commandIsTerminal(await t.api.readCommand('cmd_cancel_contract'))).toBe(true);
    expect((await t.api.observeControls(agent, response)).response.status).toBe('in_progress');
    expect(t.fetch.mock.calls.every(([, options]) => options!.method === 'GET')).toBe(true);
  });
  it.each([
    'cancel-conflict',
    'stale-target',
    'wrong-approval',
    'capability-refusal',
    'foreign-submit',
    'anonymous-submit',
  ])('preserves typed refusal %s without retrying', async (id) => {
    const t = harness();
    const body = fixture(id);
    const code = (body.error as { code: string }).code;
    t.fetch.mockResolvedValueOnce(
      json(
        body,
        id === 'capability-refusal'
          ? 422
          : id === 'foreign-submit'
            ? 404
            : id === 'anonymous-submit'
              ? 401
              : 409,
      ),
    );
    await expect(
      t.api.submitControl(agent, response, intent, { idempotencyKey: 'old-key' }),
    ).rejects.toMatchObject({ code });
    expect(t.fetch).toHaveBeenCalledOnce();
  });
  it.each(['', ' ', ' key', 'key ', 'key\n', 'a'.repeat(257), '😀'])(
    'rejects an invalid or normalized key before HTTP: %s',
    async (key) => {
      const t = harness();
      await expect(
        t.api.submitControl(agent, response, intent, { idempotencyKey: key }),
      ).rejects.toBeInstanceOf(ValidationError);
      expect(t.fetch).not.toHaveBeenCalled();
    },
  );
  it('accepts a 256-byte retained key and Unicode text without changing bytes', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(json(fixture('cancel'), 202));
    const body = { ...intent, command: { type: 'steer' as const, text: '界'.repeat(80_000) } };
    await t.api.submitControl(agent, response, body, { idempotencyKey: 'k'.repeat(256) });
    expect(JSON.parse(String(t.fetch.mock.calls[0]![1]!.body))).toEqual(body);
  });
  it.each([
    { ...intent, actor: 'forged' },
    { ...intent, turn_sequence: 0 },
    { ...intent, expected_agent_revision: Number.MAX_SAFE_INTEGER + 1 },
    { ...intent, command: { type: 'cancel', text: 'injected' } },
    { ...intent, command: { type: 'steer', text: '   ' } },
    { ...intent, command: { type: 'steer', text: '界'.repeat(90_000) } },
    { ...intent, command: { type: 'resolve_approval', approval_id: 'bad\n', decision: 'allow' } },
    { ...intent, command: { type: 'resolve_approval', approval_id: 'approval', decision: 'yes' } },
  ])('refuses malformed intent before HTTP', async (body) => {
    const t = harness();
    await expect(
      t.api.submitControl(agent, response, body as never, { idempotencyKey: 'key' }),
    ).rejects.toBeInstanceOf(ValidationError);
    expect(t.fetch).not.toHaveBeenCalled();
  });
  it.each([' ', 'bad\n', 'a'.repeat(257), '..'])(
    'rejects invalid target IDs before HTTP: %s',
    async (id) => {
      const t = harness();
      await expect(t.api.observeControls(id, response)).rejects.toBeInstanceOf(ValidationError);
      await expect(t.api.readCommand(id)).rejects.toBeInstanceOf(ValidationError);
      expect(t.fetch).not.toHaveBeenCalled();
    },
  );
  it('honors local cancellation as observation cancellation without POSTing a control', async () => {
    const t = harness();
    await expect(
      t.api.observeControls(agent, response, { signal: AbortSignal.abort() }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(t.fetch).not.toHaveBeenCalled();
  });
  it('preserves a queue/authority observation failure and Retry-After', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          error: {
            type: 'server_error',
            code: 'response_unavailable',
            message: 'Unavailable',
            param: null,
          },
        }),
        { status: 503, headers: { 'Retry-After': '1' } },
      ),
    );
    await expect(t.api.readCommand('cmd_cancel_contract')).rejects.toBeInstanceOf(ApiError);
    expect(t.fetch).toHaveBeenCalledOnce();
  });
  it('rejects wrong success statuses', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(json(fixture('cancel'), 200));
    await expect(
      t.api.submitControl(agent, response, intent, { idempotencyKey: 'key' }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
});
