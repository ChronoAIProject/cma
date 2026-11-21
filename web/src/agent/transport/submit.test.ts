import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, createClient } from '@cma/client';
import { commandStatus } from './controls.fixture';
import { withRequestRecovery } from './requestRecovery';
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

function retainedReceipt() {
  const status = commandStatus(receipt.responseId, 1, true);
  return Response.json({
    ...status,
    receipt: { ...status.receipt, agent_id: 'agent-one', command_id: receipt.commandId },
  });
}

function duplicateTabStorage() {
  const retained = new Map<string, string>(Object.entries(sessionStorage));
  return {
    get length() {
      return retained.size;
    },
    key: (index: number) => [...retained.keys()][index] ?? null,
    getItem: (key: string) => retained.get(key) ?? null,
    setItem: (key: string, value: string) => {
      retained.set(key, value);
    },
    removeItem: (key: string) => {
      retained.delete(key);
    },
  };
}

it('reconciles one lost acknowledgement from a reloaded page and a second tab', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockRejectedValueOnce(new TypeError('connection lost'))
    .mockImplementation(async () => retainedReceipt());
  const sdk = createClient({ fetch });
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  const onAccepted = vi.fn();
  await expect(submitAgentInput(sdk, journal, 'original\ntext', { onAccepted })).rejects.toThrow();
  expect(fetch).toHaveBeenCalledOnce();
  expect(onAccepted).not.toHaveBeenCalled();
  const reloaded = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  const otherTab = agentTransportJournal(duplicateTabStorage(), 'alice', 'agent-one');
  expect(reloaded.select(false)).toBe('responses_v2');
  const onOtherTab = vi.fn();
  await Promise.all([
    recoverAgentInput(sdk, reloaded, { onAccepted }),
    recoverAgentInput(sdk, otherTab, { onAccepted: onOtherTab }),
  ]);
  const [first, ...lookups] = fetch.mock.calls;
  expect(lookups).toHaveLength(2);
  for (const recovered of lookups) {
    expect(recovered[0]).toBe(`${first?.[0]}/receipt`);
    expect(recovered[1]?.body).toBe(first?.[1]?.body);
    expect(new Headers(recovered[1]?.headers).get('idempotency-key')).toBe(
      new Headers(first?.[1]?.headers).get('idempotency-key'),
    );
  }
  expect(onAccepted).toHaveBeenCalledExactlyOnceWith(receipt);
  expect(onOtherTab).toHaveBeenCalledExactlyOnceWith(receipt);
  await recoverAgentInput(sdk, reloaded, { onAccepted });
  expect(fetch).toHaveBeenCalledTimes(3);
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

it('checks a receipt after an ambiguous transport error without submitting again', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockRejectedValueOnce(new TypeError('Connection lost'))
    .mockResolvedValueOnce(retainedReceipt());
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  const authorize = vi.fn(async () => undefined);
  const onAccepted = vi.fn();
  await withRequestRecovery(undefined, { enabled: true, delay: () => 0 }, (recovery) =>
    submitAgentInput(createClient({ fetch }), journal, 'first\nmessage', {
      authorize,
      onAccepted,
      recovery,
      signal: recovery.signal,
    }),
  );
  expect(fetch.mock.calls.map(([url]) => url)).toEqual([
    '/api/v2/agents/agent-one/responses',
    '/api/v2/agents/agent-one/responses/receipt',
  ]);
  expect(authorize).toHaveBeenCalledTimes(2);
  expect(onAccepted).toHaveBeenCalledExactlyOnceWith(receipt);
  expect(journal.read()?.receipt).toEqual(receipt);
  expect(fetch.mock.calls[1]?.[1]?.body).toBe(fetch.mock.calls[0]?.[1]?.body);
  expect(new Headers(fetch.mock.calls[1]?.[1]?.headers).get('idempotency-key')).toBe(
    new Headers(fetch.mock.calls[0]?.[1]?.headers).get('idempotency-key'),
  );
});

it('retains an unresolved intent across a conflict and repeated receipt misses', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(admissionConflict())
    .mockImplementation(async () => Response.json(null));
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  const sdk = createClient({ fetch });
  const onAccepted = vi.fn();
  await expect(submitAgentInput(sdk, journal, 'retained', { onAccepted })).rejects.toBeInstanceOf(
    ApiError,
  );
  expect(await recoverAgentInput(sdk, journal, { onAccepted })).toBe(false);
  expect(await recoverAgentInput(sdk, journal, { onAccepted })).toBe(false);
  expect(fetch.mock.calls.map(([url]) => url)).toEqual([
    '/api/v2/agents/agent-one/responses',
    '/api/v2/agents/agent-one/responses/receipt',
    '/api/v2/agents/agent-one/responses/receipt',
  ]);
  expect(onAccepted).not.toHaveBeenCalled();
  expect(journal.read()).toMatchObject({ input: 'retained', state: 'pending' });
  expect(
    new Set(fetch.mock.calls.map(([, init]) => new Headers(init?.headers).get('idempotency-key')))
      .size,
  ).toBe(1);
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

it.each(['invalidated', 'accepted', 'replaced'] as const)(
  'revalidates the original intent when the journal is %s while a receipt is in flight',
  async (change) => {
    const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
    journal.select(true);
    const retained = journal.retain('original');
    let finish!: (value: Response) => void;
    const fetch = vi.fn<typeof globalThis.fetch>(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const onAccepted = vi.fn();
    const result = recoverAgentInput(createClient({ fetch }), journal, { onAccepted });
    const outcome = result.catch((error: unknown) => error);
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledOnce());
    if (change === 'invalidated') journal.invalidate();
    else if (change === 'accepted') journal.accepted(retained.key, receipt);
    else {
      journal.refused(retained.key);
      journal.retain('replacement');
    }
    finish(retainedReceipt());
    if (change === 'accepted') {
      expect(await outcome).toBe(true);
      expect(onAccepted).toHaveBeenCalledExactlyOnceWith(receipt);
    } else {
      expect(await outcome).toBeInstanceOf(Error);
      expect(onAccepted).not.toHaveBeenCalled();
    }
    if (change === 'replaced') expect(journal.read()?.input).toBe('replacement');
  },
);
