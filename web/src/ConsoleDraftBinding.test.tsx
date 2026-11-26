import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import './test/connectedAccounts';
import { Console } from './Console';
import { CmaClientProvider } from './api/context';
import { AGENT_DETAIL } from './test/agent';
import { sandboxFixture } from './test/hierarchy';
import { WIRE_AGENT } from './test/rest';
import { taskExperience } from './test/taskExperience';
import { connectionClient, type ConnectionReport } from './connections/client';
import { deferred } from './agent/transport/queuedReconciliation.fixture';
import { harness, completedInputReceipt } from './agent/transport/controller.fixture';
import { agentTransportJournal } from './agent/transport/journal';
import { agentDraftKey, readAgentDraft } from './agent/draftStorage';

const text = 'Keep my message and attachment';
const owner = 'owner';
const agent = 'agt_1';
const draftKey = agentDraftKey(owner, agent);
const flush = () => act(() => vi.advanceTimersByTimeAsync(0));
const field = () => screen.getByRole('textbox', { name: 'Message' });
const send = () => fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
const retry = () => fireEvent.click(screen.getByRole('button', { name: 'Retry draft storage' }));

beforeEach(() => {
  vi.useFakeTimers();
  sessionStorage.clear();
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
  sessionStorage.clear();
});

async function setup(paused = false) {
  const h = harness();
  h.currentUser.mockResolvedValue(owner);
  if (paused) vi.spyOn(h.client, 'readSandbox').mockResolvedValue(sandboxFixture('paused'));
  const resourceViews = createClient({
    fetch: async () =>
      Response.json({
        ...WIRE_AGENT,
        id: agent,
        workspace: { ...WIRE_AGENT.workspace, id: AGENT_DETAIL.workspace.id },
        sandbox: { ...WIRE_AGENT.sandbox, id: AGENT_DETAIL.sandbox.id },
      }),
  });
  const upload = vi.fn(async () => ({
    id: `att_${'a'.repeat(64)}`,
    agentId: agent,
    name: 'notes.txt',
    size: 5,
    path: `/home/attachments/${agent}/att_${'a'.repeat(64)}/notes.txt`,
    mediaType: 'text/plain',
    sha256: 'b'.repeat(64),
  }));
  const client = {
    ...h.client,
    resourceViews,
    taskExperience: taskExperience(owner),
    uploadAgentAttachment: upload,
  };
  const show = () => (
    <CmaClientProvider client={client}>
      <Console
        user={{ id: owner, handle: owner, visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={{ kind: 'agent', agentId: agent }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>
  );
  const journal = agentTransportJournal(sessionStorage, owner, agent);
  const view = render(show());
  await flush();
  expect(h.streaming).toBe(true);
  fireEvent.change(field(), { target: { value: text } });
  fireEvent.drop(field(), {
    dataTransfer: { files: [new File(['notes'], 'notes.txt', { type: 'text/plain' })] },
  });
  await flush();
  expect(screen.getByText('Ready', { exact: true })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  return { h, upload, journal, view, show };
}

it.each(['read', 'write'] as const)(
  'keeps a real Console draft and uploaded file when binding cannot %s; only explicit retry sends',
  async (failure) => {
    const f = await setup();
    const saved = readAgentDraft(owner, agent);
    const get = Storage.prototype.getItem;
    const set = Storage.prototype.setItem;
    const read = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(function (
      this: Storage,
      key,
    ) {
      if (failure === 'read' && key === draftKey)
        throw new DOMException('Unavailable', 'SecurityError');
      return get.call(this, key);
    });
    const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
      this: Storage,
      key,
      value,
    ) {
      if (failure === 'write' && key === draftKey && JSON.parse(value).commandKey)
        throw new DOMException('Unavailable', 'QuotaExceededError');
      set.call(this, key, value);
    });
    send();
    await flush();
    expect(f.h.post).not.toHaveBeenCalled();
    expect(f.h.inputReceipt).not.toHaveBeenCalled();
    expect(f.journal.read()).toBeNull();
    expect(field()).toHaveValue(text);
    expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('notes.txt');
    expect(screen.getByText('Not sent', { exact: true })).toBeInTheDocument();
    expect(screen.queryByText('Awaiting confirmation')).not.toBeInTheDocument();
    expect(
      screen.getByText(/message was not sent because its draft could not be saved/),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry draft storage' })).toBeEnabled();
    read.mockRestore();
    write.mockRestore();
    expect(readAgentDraft(owner, agent)).toEqual(saved);
    retry();
    await flush();
    expect(f.h.post).not.toHaveBeenCalled();
    expect(f.h.inputReceipt).not.toHaveBeenCalled();
    expect(f.upload).toHaveBeenCalledOnce();
    send();
    await flush();
    expect(f.h.post).toHaveBeenCalledOnce();
    expect(f.journal.read()?.state).toBe('accepted');
    expect(field()).toHaveValue('');
    expect(screen.queryByRole('list', { name: 'Attachments' })).not.toBeInTheDocument();
    expect(sessionStorage.getItem(draftKey)).toBeNull();
    expect(screen.queryByText('Not sent', { exact: true })).not.toBeInTheDocument();
    const row = screen
      .getAllByRole('article', { name: owner })
      .find((item) => within(item).queryByText(text));
    expect(within(row!).getByText('Sent', { exact: true })).toBeInTheDocument();
    expect(f.upload).toHaveBeenCalledOnce();
  },
);

it.each(['retry first', 'send directly', 'remount'] as const)(
  'preserves a later successful binding after %s and clears it through the original receipt',
  async (recovery) => {
    const f = await setup();
    const set = Storage.prototype.setItem;
    const fault = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
      this: Storage,
      key,
      value,
    ) {
      if (key === draftKey && JSON.parse(value).commandKey)
        throw new DOMException('Full', 'QuotaExceededError');
      set.call(this, key, value);
    });
    send();
    await flush();
    expect(f.h.post).not.toHaveBeenCalled();
    expect(f.journal.read()).toBeNull();
    expect(screen.getByRole('button', { name: 'Retry draft storage' })).toBeInTheDocument();
    fault.mockRestore();
    if (recovery === 'retry first') retry();
    f.h.post.mockRejectedValueOnce(new TypeError('Response lost'));
    send();
    await flush();
    expect(f.h.post).toHaveBeenCalledOnce();
    const pending = f.journal.read()!;
    expect(pending.state).toBe('pending');
    expect(readAgentDraft(owner, agent).commandKey).toBe(pending.key);
    expect(screen.queryByRole('button', { name: 'Retry draft storage' })).not.toBeInTheDocument();
    if (recovery === 'remount') {
      f.view.unmount();
      render(f.show());
      await flush();
      expect(field()).toHaveValue(text);
      expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
    }
    f.h.inputReceipt.mockResolvedValue(completedInputReceipt());
    fireEvent.click(screen.getByRole('button', { name: 'Recover pending request' }));
    await flush();
    expect(f.journal.read()).toMatchObject({ key: pending.key, state: 'accepted' });
    expect(field()).toHaveValue('');
    expect(screen.queryByRole('list', { name: 'Attachments' })).not.toBeInTheDocument();
    expect(sessionStorage.getItem(draftKey)).toBeNull();
    expect(f.h.post).toHaveBeenCalledOnce();
    expect(f.upload).toHaveBeenCalledOnce();
    for (const [lookup] of f.h.inputReceipt.mock.calls) {
      expect(lookup!.body).toBe(f.h.post.mock.calls[0]![0]!.body);
      expect(new Headers(lookup!.headers).get('idempotency-key')).toBe(pending.key);
    }
  },
);

it.each([text, 'My next message'])(
  'preserves the new %s revision typed while the original send waits for connections',
  async (newText) => {
    const f = await setup(true);
    fireEvent(window, new Event('blur'));
    const report = await connectionClient.check({ type: 'agent', id: agent });
    const ready = deferred<ConnectionReport>();
    vi.mocked(connectionClient.check).mockReturnValueOnce(ready.promise);
    send();
    await flush();
    expect(f.h.post).not.toHaveBeenCalled();
    expect(screen.getByText('Sending…')).toBeInTheDocument();
    fireEvent.change(field(), { target: { value: newText } });
    const newer = readAgentDraft(owner, agent);
    await act(async () => ready.resolve(report));
    await flush();
    expect(f.h.post).toHaveBeenCalledOnce();
    expect(f.journal.read()?.state).toBe('accepted');
    expect(field()).toHaveValue(newText);
    expect(readAgentDraft(owner, agent)).toEqual(newer);
    expect(newer.commandKey).toBeUndefined();
    expect(f.upload).toHaveBeenCalledOnce();
  },
);

it('does not classify a post-dispatch journal failure as not sent and recovers its exact request', async () => {
  const f = await setup();
  const set = Storage.prototype.setItem;
  const fault = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
    this: Storage,
    key,
    value,
  ) {
    if (key.startsWith('cma:agent-transport:v1:') && JSON.parse(value).input?.state === 'accepted')
      throw new DOMException('Accepted unavailable', 'QuotaExceededError');
    set.call(this, key, value);
  });
  send();
  await flush();
  const pending = f.journal.read()!;
  expect(pending.state).toBe('pending');
  expect(f.h.post).toHaveBeenCalledOnce();
  expect(screen.queryByText('Not sent', { exact: true })).not.toBeInTheDocument();
  expect(field()).toHaveValue(text);
  expect(readAgentDraft(owner, agent).commandKey).toBe(pending.key);
  fault.mockRestore();
  f.h.inputReceipt.mockResolvedValue(completedInputReceipt());
  fireEvent.click(screen.getByRole('button', { name: 'Recover pending request' }));
  await flush();
  expect(f.journal.read()).toMatchObject({ key: pending.key, state: 'accepted' });
  expect(field()).toHaveValue('');
  expect(f.h.post).toHaveBeenCalledOnce();
  expect(f.upload).toHaveBeenCalledOnce();
  for (const [lookup] of f.h.inputReceipt.mock.calls) {
    expect(lookup!.body).toBe(f.h.post.mock.calls[0]![0]!.body);
    expect(new Headers(lookup!.headers).get('idempotency-key')).toBe(pending.key);
  }
});
