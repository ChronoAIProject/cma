import { beforeEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { agentTransportJournal } from './journal';
import { recoverAgentInput, submitAgentInput } from './submit';

const receipt = { responseId: 'resp_retained', commandId: 'cmd_retained' };
beforeEach(() => sessionStorage.clear());

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
