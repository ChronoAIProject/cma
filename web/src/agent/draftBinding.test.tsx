import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { agentClient } from '../test/agent';
import { useAgentDraft } from './draft';
import { acceptDraftCommand, agentDraftKey, readAgentDraft } from './draftStorage';
import { agentTransportJournal } from './transport/journal';
import { recoverAgentInput, submitAgentInput } from './transport/submit';
import { accepted, completedInputReceipt } from './transport/controller.fixture';
import { bindDraftCommand } from './draftDelivery';

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());
const text = 'Keep this exact draft';
const key = agentDraftKey('owner', 'agt_1');

function setup() {
  const client = agentClient();
  const hook = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
  act(() => hook.result.current.setContent(text));
  const journal = agentTransportJournal(sessionStorage, 'owner', 'agt_1');
  journal.select(true);
  return { hook, journal };
}

it.each(['read', 'write'] as const)(
  'refuses a new input before retaining or dispatching when draft binding cannot %s',
  async (failure) => {
    const { hook, journal } = setup();
    const before = readAgentDraft('owner', 'agt_1');
    const submission = hook.result.current.beginSubmission!(text);
    const get = Storage.prototype.getItem;
    const set = Storage.prototype.setItem;
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(function (this: Storage, name) {
      if (failure === 'read' && name === key) throw new DOMException('Denied', 'SecurityError');
      return get.call(this, name);
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
      this: Storage,
      name,
      value,
    ) {
      if (failure === 'write' && name === key && JSON.parse(value).commandKey)
        throw new DOMException('Full', 'QuotaExceededError');
      set.call(this, name, value);
    });
    const fetch = vi.fn<typeof globalThis.fetch>(async () => accepted());
    let error: unknown;
    await act(async () => {
      try {
        await submitAgentInput(createClient({ fetch }), journal, text, { onAccepted() {} });
      } catch (caught) {
        error = caught;
      }
    });
    submission.settled();
    expect.soft(fetch).not.toHaveBeenCalled();
    expect.soft(journal.read()).toBeNull();
    expect.soft(error).toMatchObject({ name: 'DraftBindingError' });
    expect.soft(hook.result.current.content).toBe(text);
    expect.soft(hook.result.current.error).toMatch(/Copy your text/);
    vi.restoreAllMocks();
    expect(readAgentDraft('owner', 'agt_1')).toEqual(before);
  },
);

it('synchronizes a successful binding and an old storage retry without losing the request key', () => {
  const { hook, journal } = setup();
  const retry = hook.result.current.retryStorage!;
  const set = Storage.prototype.setItem;
  const fault = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
    this: Storage,
    name,
    value,
  ) {
    if (name === key && JSON.parse(value).commandKey)
      throw new DOMException('Full', 'QuotaExceededError');
    set.call(this, name, value);
  });
  const first = hook.result.current.beginSubmission!(text);
  act(() => {
    bindDraftCommand('owner', 'agt_1', text, 'failed-binding', sessionStorage);
  });
  first.settled();
  expect(hook.result.current.error).toMatch(/Copy your text/);
  expect(journal.read()).toBeNull();
  fault.mockRestore();
  const second = hook.result.current.beginSubmission!(text);
  act(() => journal.retain(text));
  second.settled();
  const retained = journal.read()!;
  expect(readAgentDraft('owner', 'agt_1').commandKey).toBe(retained.key);
  expect.soft(hook.result.current.error).toBeUndefined();
  act(retry);
  expect.soft(readAgentDraft('owner', 'agt_1').commandKey).toBe(retained.key);
  act(() => journal.accepted(retained.key, { responseId: 'resp_1', commandId: 'cmd_1' }));
  expect(hook.result.current.content).toBe('');
});

it('does not mistake an older saved revision after a failed edit for a superseded submission', () => {
  const { hook, journal } = setup();
  const original = readAgentDraft('owner', 'agt_1');
  const set = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new DOMException('Full', 'QuotaExceededError');
  });
  act(() => hook.result.current.setContent('The current unsaved revision'));
  set.mockRestore();
  const submission = hook.result.current.beginSubmission!('The current unsaved revision');
  act(() =>
    expect(() => journal.retain('The current unsaved revision')).toThrow(
      'not sent because its draft',
    ),
  );
  submission.settled();
  expect(journal.read()).toBeNull();
  expect(readAgentDraft('owner', 'agt_1')).toEqual(original);
  expect(hook.result.current.content).toBe('The current unsaved revision');
  act(() => hook.result.current.retryStorage!());
  const retry = hook.result.current.beginSubmission!('The current unsaved revision');
  act(() => journal.retain('The current unsaved revision'));
  retry.settled();
  expect(readAgentDraft('owner', 'agt_1').commandKey).toBe(journal.read()?.key);
});

it.each([text, 'A later draft'])(
  'never binds or clears a later revision even when its text is %s',
  (next) => {
    const { hook, journal } = setup();
    const submission = hook.result.current.beginSubmission!(text);
    act(() => hook.result.current.setContent(next));
    const newer = readAgentDraft('owner', 'agt_1');
    act(() => journal.retain(text));
    submission.settled();
    act(() => journal.accepted(journal.read()!.key, { responseId: 'resp_1', commandId: 'cmd_1' }));
    expect(readAgentDraft('owner', 'agt_1')).toEqual(newer);
    expect(hook.result.current.content).toBe(next);
  },
);

it.each(['agent', 'client', 'unmount'] as const)(
  'clears the original draft when its send is accepted after changing %s',
  (change) => {
    const client = agentClient();
    const hook = renderHook(({ agent, selected }) => useAgentDraft(selected, 'owner', agent), {
      initialProps: { agent: 'agt_1', selected: client },
    });
    act(() => hook.result.current.setContent(text));
    const submission = hook.result.current.beginSubmission!(text);
    if (change === 'unmount') hook.unmount();
    else
      hook.rerender({
        agent: change === 'agent' ? 'agt_2' : 'agt_1',
        selected: change === 'client' ? agentClient() : client,
      });
    if (change === 'agent') act(() => hook.result.current.setContent('Other Agent draft'));
    act(() => {
      expect(bindDraftCommand('owner', 'agt_1', text, 'old-request', sessionStorage)).toBe('bound');
    });
    if (change === 'agent') {
      hook.rerender({ agent: 'agt_1', selected: client });
      act(() => hook.result.current.retryStorage!());
      expect(readAgentDraft('owner', 'agt_1').commandKey).toBe('old-request');
    }
    act(() => acceptDraftCommand('owner', 'agt_1', 'old-request', sessionStorage));
    submission.settled();
    expect(readAgentDraft('owner', 'agt_1').content).toBe('');
    if (change !== 'unmount') expect(hook.result.current.content).toBe('');
    if (change === 'agent')
      expect(readAgentDraft('owner', 'agt_2').content).toBe('Other Agent draft');
  },
);

it('does not bind a submission after the owner changes', () => {
  const client = agentClient();
  const hook = renderHook(({ owner }) => useAgentDraft(client, owner, 'agt_1'), {
    initialProps: { owner: 'owner' },
  });
  act(() => hook.result.current.setContent(text));
  const submission = hook.result.current.beginSubmission!(text);
  hook.rerender({ owner: 'someone-else' });
  act(() => hook.result.current.setContent('Other owner draft'));
  const before = { ...sessionStorage };
  expect(bindDraftCommand('owner', 'agt_1', text, 'old-request', sessionStorage)).toBe(
    'superseded',
  );
  submission.settled();
  expect({ ...sessionStorage }).toEqual(before);
});

it('keeps a retry queued immediately after binding on the same request key', () => {
  const { hook, journal } = setup();
  const retry = hook.result.current.retryStorage!;
  const submission = hook.result.current.beginSubmission!(text);
  act(() => {
    journal.retain(text);
    retry();
  });
  submission.settled();
  expect(readAgentDraft('owner', 'agt_1').commandKey).toBe(journal.read()?.key);
});

it('does not dispatch a bound draft if pending journal persistence fails; explicit retry uses a new key', async () => {
  const { hook, journal } = setup();
  const fetch = vi.fn<typeof globalThis.fetch>(async () => accepted());
  const sdk = createClient({ fetch });
  const set = Storage.prototype.setItem;
  const fault = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
    this: Storage,
    name,
    value,
  ) {
    if (name.startsWith('cma:agent-transport:v1:') && JSON.parse(value).input?.state === 'pending')
      throw new DOMException('Pending unavailable', 'QuotaExceededError');
    set.call(this, name, value);
  });
  const first = hook.result.current.beginSubmission!(text);
  act(() =>
    expect(() => submitAgentInput(sdk, journal, text, { onAccepted() {} })).toThrow(
      'Pending unavailable',
    ),
  );
  first.settled();
  const unsentKey = readAgentDraft('owner', 'agt_1').commandKey;
  expect(unsentKey).toBeTruthy();
  expect(journal.read()).toBeNull();
  expect(fetch).not.toHaveBeenCalled();
  expect(hook.result.current.content).toBe(text);
  fault.mockRestore();
  const second = hook.result.current.beginSubmission!(text);
  await act(async () => submitAgentInput(sdk, journal, text, { onAccepted: second.accepted }));
  second.settled();
  expect(fetch).toHaveBeenCalledOnce();
  expect(journal.read()?.key).not.toBe(unsentKey);
  expect(hook.result.current.content).toBe('');
});

it('retains the actual dispatched key when accepted persistence fails and recovers only its receipt', async () => {
  const { hook, journal } = setup();
  const fetch = vi.fn<typeof globalThis.fetch>(async (url) =>
    String(url).endsWith('/receipt') ? completedInputReceipt() : accepted(),
  );
  const sdk = createClient({ fetch });
  const set = Storage.prototype.setItem;
  const fault = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
    this: Storage,
    name,
    value,
  ) {
    if (name.startsWith('cma:agent-transport:v1:') && JSON.parse(value).input?.state === 'accepted')
      throw new DOMException('Accepted unavailable', 'QuotaExceededError');
    set.call(this, name, value);
  });
  const first = hook.result.current.beginSubmission!(text);
  await act(async () => {
    await expect(
      submitAgentInput(sdk, journal, text, { onAccepted: first.accepted }),
    ).rejects.toMatchObject({
      name: 'ProtocolError',
      cause: { name: 'QuotaExceededError', message: 'Accepted unavailable' },
    });
  });
  first.settled();
  const pending = journal.read()!;
  expect(pending.state).toBe('pending');
  expect(fetch).toHaveBeenCalledOnce();
  expect(readAgentDraft('owner', 'agt_1').commandKey).toBe(pending.key);
  expect(hook.result.current.content).toBe(text);
  fault.mockRestore();
  await act(async () => {
    expect(await recoverAgentInput(sdk, journal, { onAccepted() {} })).toBe(true);
  });
  expect(fetch.mock.calls.map(([url]) => url)).toEqual([
    '/api/v2/agents/agt_1/responses',
    '/api/v2/agents/agt_1/responses/receipt',
  ]);
  expect(fetch.mock.calls[1]![1]!.body).toBe(fetch.mock.calls[0]![1]!.body);
  expect(new Headers(fetch.mock.calls[1]![1]!.headers).get('idempotency-key')).toBe(pending.key);
  expect(hook.result.current.content).toBe('');
});

it.each([
  ['steer', true],
  ['interrupt', true],
  ['steer', false],
  ['interrupt', false],
] as const)('preserves existing %s retention with binding failure=%s', (mode, unavailable) => {
  const { hook, journal } = setup();
  const submission = hook.result.current.beginSubmission!(text);
  const set = Storage.prototype.setItem;
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage, name, value) {
    if (unavailable && name === key && JSON.parse(value).commandKey)
      throw new DOMException('Full', 'QuotaExceededError');
    set.call(this, name, value);
  });
  act(() => {
    if (mode === 'steer')
      journal.retainControl({
        key: 'control-request',
        responseId: 'resp_1',
        commandId: null,
        body: { turn_sequence: 1, expected_agent_revision: 1, command: { type: 'steer', text } },
      });
    else
      journal.interruption.retain({
        key: 'interrupt-request',
        responseId: 'resp_1',
        receipt: null,
        body: { turn_sequence: 1, expected_agent_revision: 1, text },
      });
  });
  submission.settled();
  if (mode === 'steer') expect(journal.readControl()?.key).toBe('control-request');
  else expect(journal.interruption.read()?.key).toBe('interrupt-request');
  expect(hook.result.current.content).toBe(text);
  if (unavailable) expect(hook.result.current.error).toMatch(/Copy your text/);
  else {
    act(() => hook.result.current.retryStorage!());
    expect(readAgentDraft('owner', 'agt_1').commandKey).toBe(
      mode === 'steer' ? 'control-request' : 'interrupt-request',
    );
    act(() => {
      if (mode === 'steer') journal.acceptedControl('control-request', 'cmd_control');
      else
        journal.interruption.accepted('interrupt-request', {
          controlId: 'cmd_control',
          commandId: 'cmd_next',
          responseId: 'resp_next',
        });
    });
    expect(hook.result.current.content).toBe('');
  }
});
