import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import './test/connectedAccounts';
import { taskExperience } from './test/taskExperience';
import { consoleDraftFixture } from './test/consoleDraft.fixture';
import { harness } from './agent/transport/controller.fixture';
import { agentTransportJournal } from './agent/transport/journal';
import { agentDraftKey, readAgentDraft, saveAgentDraft } from './agent/draftStorage';
import type { UploadedAttachment } from './agent/attachments';

const owner = 'owner';
const agent = 'agt_1';
const draftKey = () => agentDraftKey(owner, agent);
const flush = () => act(() => vi.advanceTimersByTimeAsync(0));
const field = () => screen.getByRole('textbox', { name: 'Message' });
const send = () => screen.getByRole('button', { name: 'Send message' });
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

function uploaded(name = 'notes.txt'): UploadedAttachment {
  const id = `att_${'a'.repeat(64)}`;
  return {
    id,
    agentId: agent,
    name,
    path: `/home/attachments/${agent}/${id}/${name}`,
    size: 5,
    mediaType: 'text/plain',
    sha256: 'b'.repeat(64),
  };
}

function fixture() {
  const transport = harness();
  transport.currentUser.mockResolvedValue(owner);
  const test = consoleDraftFixture({
    agentTransport: transport.client.agentTransport,
    taskExperience: taskExperience(owner),
    readAgentTranscript: async () => ({ agentId: agent, throughSeq: 0, turns: [] }),
  });
  test.uploadAttachment.mockResolvedValue(uploaded());
  const journal = agentTransportJournal(sessionStorage, owner, agent);
  const fault = { read: false, remove: false };
  const get = Storage.prototype.getItem;
  const remove = Storage.prototype.removeItem;
  vi.spyOn(Storage.prototype, 'getItem').mockImplementation(function (this: Storage, key) {
    if (this === sessionStorage && key === draftKey() && fault.read)
      throw new DOMException('Draft temporarily unavailable', 'SecurityError');
    return get.call(this, key);
  });
  vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(function (this: Storage, key) {
    if (this === sessionStorage && key === draftKey() && fault.remove)
      throw new DOMException('Draft removal temporarily unavailable', 'SecurityError');
    return remove.call(this, key);
  });
  return { ...test, transport, journal, fault };
}

it('reconciles an accepted draft and its ready attachment immediately after the storage retry', async () => {
  const f = fixture();
  const view = render(f.show());
  await flush();
  expect(f.transport.streaming).toBe(true);
  fireEvent.change(field(), { target: { value: 'Already accepted with an attachment' } });
  fireEvent.drop(field(), {
    dataTransfer: { files: [new File(['notes'], 'notes.txt', { type: 'text/plain' })] },
  });
  await flush();
  expect(screen.getByText('Ready', { exact: true })).toBeInTheDocument();
  expect(send()).toBeEnabled();
  f.fault.remove = true;
  fireEvent.click(send());
  await flush();
  const accepted = f.journal.read();
  expect(accepted?.state).toBe('accepted');
  expect(readAgentDraft(owner, agent).commandKey).toBe(accepted?.key);
  expect(field()).toHaveValue('');
  expect(screen.getByText(/saved draft could not be removed/)).toBeInTheDocument();
  expect(f.transport.post).toHaveBeenCalledOnce();
  expect(f.uploadAttachment).toHaveBeenCalledOnce();
  view.unmount();

  f.fault.remove = false;
  f.fault.read = true;
  render(f.show());
  await flush();
  expect(screen.getByText(/saved Agent draft could not be recovered/)).toBeInTheDocument();
  expect(field()).toHaveValue('');
  expect(send()).toBeDisabled();
  const requests = f.transport.fetch.mock.calls.length;
  const accessReads = f.readAccessible.mock.calls.length;
  f.fault.read = false;
  retry();
  await flush();
  expect(field()).toHaveValue('');
  expect(screen.queryByRole('list', { name: 'Attachments' })).not.toBeInTheDocument();
  expect(send()).toBeDisabled();
  expect(sessionStorage.getItem(draftKey())).toBeNull();
  expect(screen.queryByRole('button', { name: 'Retry draft storage' })).not.toBeInTheDocument();
  expect(f.journal.read()).toEqual(accepted);
  expect(f.transport.post).toHaveBeenCalledOnce();
  expect(f.uploadAttachment).toHaveBeenCalledOnce();
  expect(f.transport.inputReceipt).not.toHaveBeenCalled();
  expect(f.transport.cancelQueued).not.toHaveBeenCalled();
  expect(f.transport.fetch).toHaveBeenCalledTimes(requests);
  expect(f.readAccessible).toHaveBeenCalledTimes(accessReads);
});

it.each(['pending', 'new revision', 'another command'] as const)(
  'preserves the %s draft and its attachment when storage becomes readable',
  async (kind) => {
    const f = fixture();
    f.journal.select(true);
    const input = f.journal.retain('Same text does not identify a request');
    if (kind !== 'pending')
      f.journal.accepted(input.key, { commandId: 'cmd_1', responseId: 'resp_1' });
    saveAgentDraft(owner, agent, {
      revision: 'local-draft-revision',
      content: input.input,
      attachments: [
        {
          key: 'ready-file',
          status: 'ready',
          description: { name: 'notes.txt', size: 5, mediaType: 'text/plain' },
          uploaded: uploaded(),
        },
      ],
      ...(kind === 'new revision'
        ? {}
        : { commandKey: kind === 'pending' ? input.key : 'other-command' }),
    });
    const saved = sessionStorage.getItem(draftKey());
    f.fault.read = true;
    render(f.show());
    await flush();
    expect(screen.getByText(/saved Agent draft could not be recovered/)).toBeInTheDocument();
    expect(send()).toBeDisabled();
    f.fault.read = false;
    retry();
    await flush();
    expect(field()).toHaveValue(input.input);
    expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('notes.txt');
    expect(sessionStorage.getItem(draftKey())).toBe(saved);
    if (kind === 'pending') {
      expect(send()).toBeDisabled();
      expect(f.journal.read()).toEqual(input);
      expect(f.transport.inputReceipt).toHaveBeenCalledOnce();
      const lookup = f.transport.inputReceipt.mock.calls[0]![0]!;
      expect(new Headers(lookup.headers).get('idempotency-key')).toBe(input.key);
      expect(JSON.parse(String(lookup.body))).toMatchObject({ input: input.input });
    } else expect(send()).toBeEnabled();
    expect(f.transport.post).not.toHaveBeenCalled();
    expect(f.uploadAttachment).not.toHaveBeenCalled();
    expect(f.transport.cancelQueued).not.toHaveBeenCalled();
  },
);
