// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { createClient } from '../client.js';
import {
  ApiError,
  ProtocolError,
  RequestAborted,
  TimeoutError,
  ValidationError,
} from '../errors.js';
import { launchFixture } from './launch-fixtures.js';

const subject = 'owner-甲';
const proof = `v1.${Buffer.from(subject).toString('base64url')}`;
const options = { expectedSubject: subject, idempotencyKey: 'saved-command' };
const body = { choice: 'resume' as const, start_if_ready: true, placement_id: 'placement' };
function setup(status = 201, value: unknown = launchFixture, verified: string | null = proof) {
  const fetch = vi.fn<typeof globalThis.fetch>(
    async () =>
      new Response(JSON.stringify(value), {
        status,
        headers: {
          'content-type': 'application/json',
          ...(verified === null ? {} : { 'X-CMA-Verified-Subject': verified }),
        },
      }),
  );
  const client = createClient({ baseUrl: 'https://cma.example.test', fetch });
  return {
    fetch,
    run: (overrides = {}) =>
      client.launches.createOrResumeBound('public/example', body, { ...options, ...overrides }),
  };
}

it.each([201, 202])(
  'binds HTTP %s to the intended subject without changing body bytes',
  async (status) => {
    const test = setup(status);
    expect(await test.run()).toEqual(launchFixture);
    expect(test.fetch).toHaveBeenCalledOnce();
    const [url, init] = test.fetch.mock.calls[0]!;
    expect(url).toBe('https://cma.example.test/api/v2/triggers/public/public%2Fexample/launches');
    expect(init?.body).toBe(JSON.stringify(body));
    expect(init?.credentials).toBe('include');
    const headers = new Headers(init?.headers);
    expect(headers.get('X-CMA-Expected-Subject')).toBe(proof);
    expect(headers.get('Idempotency-Key')).toBe(options.idempotencyKey);
  },
);

it.each([null, 'v1.b3RoZXI', `${proof}=`, `${proof}, ${proof}`, 'v2.invalid', ''])(
  'rejects unbound success (%s)',
  async (verified) => {
    const test = setup(201, launchFixture, verified);
    await expect(test.run()).rejects.toBeInstanceOf(ProtocolError);
    expect(test.fetch).toHaveBeenCalledOnce();
  },
);

it.each([
  [404, 'not_found'],
  [405, 'method_not_allowed'],
  [401, 'unauthorized'],
  [409, 'sandbox_credential_setup_required'],
  [409, 'trigger_repeat_choice_required'],
  [429, 'visitor_launch_limit_reached'],
  [422, 'idempotency_key_reused'],
])('does not settle an unbound %s %s or fall back to v1', async (status, code) => {
  const test = setup(Number(status), { code, title: 'Synthetic refusal' }, null);
  await expect(test.run()).rejects.toBeInstanceOf(ProtocolError);
  expect(test.fetch).toHaveBeenCalledOnce();
});

it('reports an unbound subject mismatch without interpreting it as credential refusal', async () => {
  const test = setup(
    409,
    { code: 'command_subject_mismatch', title: 'Use the original account' },
    null,
  );
  await expect(test.run()).rejects.toMatchObject({ status: 409, code: 'command_subject_mismatch' });
  expect(test.fetch).toHaveBeenCalledOnce();
});

it.each([
  'sandbox_credential_setup_required',
  'trigger_repeat_choice_required',
  'idempotency_recovery_required',
])('retains a verified %s domain error', async (code) => {
  const test = setup(409, { code, title: 'Synthetic refusal' });
  await expect(test.run()).rejects.toBeInstanceOf(ApiError);
  expect(test.fetch).toHaveBeenCalledOnce();
});

it.each(['', '\ud800', 'a'.repeat(3070)])(
  'rejects invalid or oversized subject before transport',
  async (expectedSubject) => {
    const test = setup();
    await expect(test.run({ expectedSubject })).rejects.toBeInstanceOf(ValidationError);
    expect(test.fetch).not.toHaveBeenCalled();
  },
);

it('accepts the largest canonical subject below the 4096-byte header bound', async () => {
  const expectedSubject = 'a'.repeat(3069);
  const verified = `v1.${Buffer.from(expectedSubject).toString('base64url')}`;
  expect(verified.length).toBe(4095);
  const test = setup(201, launchFixture, verified);
  await expect(test.run({ expectedSubject })).resolves.toEqual(launchFixture);
});

it('does not send an aborted command', async () => {
  const test = setup();
  await expect(test.run({ signal: AbortSignal.abort() })).rejects.toBeInstanceOf(RequestAborted);
  expect(test.fetch).not.toHaveBeenCalled();
});

it('keeps body consumption inside the original deadline', async () => {
  const stream = new ReadableStream<Uint8Array>({
    start: (controller) => controller.enqueue(new TextEncoder().encode('{')),
  });
  const fetch = vi.fn<typeof globalThis.fetch>(
    async () => new Response(stream, { status: 201, headers: { 'X-CMA-Verified-Subject': proof } }),
  );
  const client = createClient({ baseUrl: 'https://cma.example.test', fetch });
  await expect(
    client.launches.createOrResumeBound('public', body, { ...options, timeoutMs: 20 }),
  ).rejects.toBeInstanceOf(TimeoutError);
  expect(fetch).toHaveBeenCalledOnce();
});

it('enforces the existing response byte limit', async () => {
  const test = setup(201, { oversized: 'x'.repeat(8 * 1024 * 1024) });
  await expect(test.run()).rejects.toBeInstanceOf(ProtocolError);
});
