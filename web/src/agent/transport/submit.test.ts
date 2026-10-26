import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, createClient } from '@chronoai/cma-sdk';
import { agentTransportJournal } from './journal';
import { recoverAgentInput, submitAgentInput } from './submit';

const receipt = { responseId: 'resp_retained', commandId: 'cmd_retained' };
beforeEach(() => sessionStorage.clear());
afterEach(() => vi.useRealTimers());

function acceptedResponse(cancel = vi.fn()) {
  return new Response(new ReadableStream({ cancel }), {
    headers: {
      'content-type': 'text/event-stream',
      'x-cma-response-id': receipt.responseId,
      'x-cma-command-id': receipt.commandId,
    },
  });
}

it('retains acceptance and returns before the Agent turn finishes', async () => {
  const cancel = vi.fn();
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(acceptedResponse(cancel));
  const sdk = createClient({ fetch });
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  const onAccepted = vi.fn(() => expect(journal.read()?.receipt).toEqual(receipt));
  await submitAgentInput(sdk, journal, 'start work', { onAccepted });
  expect(onAccepted).toHaveBeenCalledOnce();
  expect(fetch).toHaveBeenCalledOnce();
  expect(fetch.mock.calls[0]?.[0]).toBe('/api/v2/agents/agent-one/responses');
  expect(JSON.parse(String(fetch.mock.calls[0]?.[1]?.body))).toEqual({
    input: 'start work',
    stream: true,
  });
  expect(cancel).toHaveBeenCalledOnce();
});

it('retries a lost acknowledgement only on explicit recovery with the exact saved request', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockRejectedValueOnce(new TypeError('connection lost'))
    .mockResolvedValueOnce(acceptedResponse());
  const sdk = createClient({ fetch });
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  const onAccepted = vi.fn();
  await expect(submitAgentInput(sdk, journal, 'original\ntext', { onAccepted })).rejects.toThrow();
  expect(fetch).toHaveBeenCalledOnce();
  expect(onAccepted).not.toHaveBeenCalled();
  const reloaded = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  expect(reloaded.select(false)).toBe('responses_v2');
  await recoverAgentInput(sdk, reloaded, { onAccepted });
  const [first, recovered] = fetch.mock.calls;
  expect(recovered?.[0]).toBe(first?.[0]);
  expect(recovered?.[1]?.body).toBe(first?.[1]?.body);
  expect(new Headers(recovered?.[1]?.headers).get('idempotency-key')).toBe(
    new Headers(first?.[1]?.headers).get('idempotency-key'),
  );
  expect(onAccepted).toHaveBeenCalledOnce();
  await recoverAgentInput(sdk, reloaded, { onAccepted });
  expect(fetch).toHaveBeenCalledTimes(2);
});

it('keeps an accepted receipt when a UI callback fails and recovers without a POST', async () => {
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(acceptedResponse());
  const sdk = createClient({ fetch });
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  await expect(
    submitAgentInput(sdk, journal, 'hello', {
      onAccepted() {
        throw new Error('view unmounted');
      },
    }),
  ).rejects.toThrow();
  expect(journal.read()?.state).toBe('accepted');
  const onAccepted = vi.fn();
  await recoverAgentInput(sdk, journal, { onAccepted });
  expect(onAccepted).toHaveBeenCalledWith(receipt);
  expect(fetch).toHaveBeenCalledOnce();
});

it('does not infer admission refusal from a post-admission HTTP failure', async () => {
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(
    new Response(
      JSON.stringify({
        type: 'about:blank',
        title: 'Unauthorized',
        status: 401,
        code: 'unauthorized',
        detail: 'Sign in',
      }),
      { status: 401, headers: { 'content-type': 'application/problem+json' } },
    ),
  );
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  await expect(
    submitAgentInput(createClient({ fetch }), journal, 'hello', { onAccepted() {} }),
  ).rejects.toThrow();
  expect(journal.read()?.state).toBe('pending');
  expect(fetch).toHaveBeenCalledOnce();
});

it('does not send or retain a command after navigation already aborted it', () => {
  const fetch = vi.fn<typeof globalThis.fetch>();
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  const controller = new AbortController();
  controller.abort();
  expect(() =>
    submitAgentInput(createClient({ fetch }), journal, 'hello', {
      signal: controller.signal,
      onAccepted() {},
    }),
  ).toThrow();
  expect(journal.read()).toBeNull();
  expect(fetch).not.toHaveBeenCalled();
});

it('settles explicit invalid-input refusal so an edited draft can become a new intent', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          error: {
            type: 'invalid_request_error',
            code: 'invalid_response_input',
            message: 'Invalid input',
            param: 'input',
          },
        }),
        { status: 422, headers: { 'content-type': 'application/json' } },
      ),
    )
    .mockResolvedValueOnce(acceptedResponse());
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  const sdk = createClient({ fetch });
  const onAccepted = vi.fn();
  await expect(submitAgentInput(sdk, journal, 'Rejected input', { onAccepted })).rejects.toThrow();
  expect(journal.read()).toBeNull();
  expect(onAccepted).not.toHaveBeenCalled();
  await submitAgentInput(sdk, journal, 'Corrected input', { onAccepted });
  expect(onAccepted).toHaveBeenCalledOnce();
  const first = fetch.mock.calls[0]![1]!;
  const second = fetch.mock.calls[1]![1]!;
  expect(new Headers(second.headers).get('idempotency-key')).not.toBe(
    new Headers(first.headers).get('idempotency-key'),
  );
});

function admissionConflict(code = 'agent_command_admission_conflict') {
  return new Response(
    JSON.stringify({
      error: {
        type: 'invalid_request_error',
        code,
        message: 'Retry the same request.',
        param: null,
      },
    }),
    {
      status: 409,
      headers: { 'content-type': 'application/json' },
    },
  );
}

it('automatically recovers definite startup contention with one key, body and acknowledgement', async () => {
  vi.useFakeTimers();
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(admissionConflict())
    .mockResolvedValueOnce(acceptedResponse());
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  const authorize = vi.fn(async () => undefined);
  const onAccepted = vi.fn();
  const submission = submitAgentInput(createClient({ fetch }), journal, 'first\nmessage', {
    authorize,
    onAccepted,
  });
  await vi.runAllTimersAsync();
  await submission;
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(authorize).toHaveBeenCalledTimes(2);
  expect(onAccepted).toHaveBeenCalledOnce();
  expect(journal.read()?.receipt).toEqual(receipt);
  expect(fetch.mock.calls[1]?.[1]?.body).toBe(fetch.mock.calls[0]?.[1]?.body);
  expect(new Headers(fetch.mock.calls[1]?.[1]?.headers).get('idempotency-key')).toBe(
    new Headers(fetch.mock.calls[0]?.[1]?.headers).get('idempotency-key'),
  );
});

it('bounds contention retries and preserves the unresolved original intent', async () => {
  vi.useFakeTimers();
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockImplementation(async () => admissionConflict());
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  const onAccepted = vi.fn();
  const result = submitAgentInput(createClient({ fetch }), journal, 'retained', {
    onAccepted,
  }).catch((error: unknown) => error);
  await vi.runAllTimersAsync();
  expect(await result).toBeInstanceOf(ApiError);
  expect(fetch).toHaveBeenCalledTimes(4);
  expect(onAccepted).not.toHaveBeenCalled();
  expect(journal.read()).toMatchObject({ input: 'retained', state: 'pending' });
  expect(
    new Set(fetch.mock.calls.map(([, init]) => new Headers(init?.headers).get('idempotency-key')))
      .size,
  ).toBe(1);
});

it('stops a contention retry when navigation aborts or the account changes', async () => {
  vi.useFakeTimers();
  for (const change of ['navigation', 'account'] as const) {
    sessionStorage.clear();
    const controller = new AbortController();
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockImplementation(async () => admissionConflict());
    const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
    journal.select(true);
    const result = submitAgentInput(createClient({ fetch }), journal, 'retained', {
      signal: controller.signal,
      async authorize() {
        throw new Error('account changed');
      },
      onAccepted() {
        throw new Error('unexpected acceptance');
      },
    }).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(0);
    if (change === 'navigation') controller.abort();
    await vi.runAllTimersAsync();
    expect(await result).toBeInstanceOf(Error);
    expect(fetch).toHaveBeenCalledOnce();
  }
});

it('does not automatically retry a different conflict or a failure after a receipt', async () => {
  for (const response of [admissionConflict('agent_command_key_conflict'), acceptedResponse()]) {
    sessionStorage.clear();
    const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(response);
    const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
    journal.select(true);
    await expect(
      submitAgentInput(createClient({ fetch }), journal, 'retained', {
        authorize: async () => {
          throw new ApiError(409, 'Observation changed', 'agent_command_admission_conflict');
        },
        onAccepted() {},
      }),
    ).rejects.toThrow();
    expect(fetch).toHaveBeenCalledOnce();
    expect(journal.read()?.state).toBe('pending');
  }
});

it('does not retry an intent whose account epoch or retained state changed during backoff', async () => {
  vi.useFakeTimers();
  for (const change of ['invalidated', 'accepted', 'replaced'] as const) {
    sessionStorage.clear();
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockImplementation(async () => admissionConflict());
    const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
    journal.select(true);
    const onAccepted = vi.fn();
    const result = submitAgentInput(createClient({ fetch }), journal, 'original', {
      authorize: async () => undefined,
      onAccepted,
    }).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(0);
    const retained = journal.read()!;
    if (change === 'invalidated') journal.invalidate();
    else if (change === 'accepted') journal.accepted(retained.key, receipt);
    else {
      journal.refused(retained.key);
      journal.retain('replacement');
    }
    await vi.runAllTimersAsync();
    expect(await result).toBeInstanceOf(Error);
    expect(fetch).toHaveBeenCalledOnce();
    expect(onAccepted).not.toHaveBeenCalled();
    if (change === 'accepted') expect(journal.read()?.receipt).toEqual(receipt);
    if (change === 'replaced') expect(journal.read()?.input).toBe('replacement');
  }
});
