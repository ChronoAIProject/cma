import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { useAgentDraft } from '../agent/draft';
import { agentDraftKey, readAgentDraft, saveAgentDraft } from '../agent/draftStorage';
import { retainPreparationDraftOwner } from './preparationDraftIdentity';
import {
  readPreparationDraft,
  savePreparationDraft,
  preparationDraftKey,
} from './preparationDraftStorage';
import { transferPreparationDraft } from './preparationDraftHandoff';

const owner = 'owner';
const scope = { owner, intentKey: 'original-request' };
const binding = {
  operationId: 'op_1',
  agentId: 'agt_1',
  sandboxId: 'sbx_1',
  workspaceId: 'wsp_1',
  worktreeId: 'wkt_1',
};
const text = 'Keep this exact\n准备草稿 🧭';
beforeEach(() => {
  sessionStorage.clear();
  retainPreparationDraftOwner(owner);
});
afterEach(() => vi.restoreAllMocks());
function setup() {
  const source = savePreparationDraft(scope, undefined, text, binding);
  const client = agentClient();
  const hook = renderHook(
    ({ client, viewerId, agentId }) => useAgentDraft(client, viewerId, agentId),
    {
      initialProps: { client, viewerId: owner, agentId: 'agt_1' },
    },
  );
  const transfer = () =>
    transferPreparationDraft(source, owner, binding, hook.result.current, () => true);
  return { source, client, hook, transfer };
}

it('copies through the mounted destination owner and removes only the verified source revision', () => {
  const { hook, transfer } = setup();
  let result: string | undefined;
  act(() => {
    result = transfer();
  });
  expect(result).toBe('transferred');
  expect(hook.result.current.content).toBe(text);
  expect(readAgentDraft(owner, 'agt_1').content).toBe(text);
  expect(readPreparationDraft(scope)).toBeUndefined();
});

it.each(['stored', 'memory-only', 'same-event', 'equal-text'] as const)(
  'preserves a %s destination and the original source',
  (kind) => {
    const { hook, transfer, source } = setup();
    if (kind === 'memory-only')
      vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
        throw new Error('quota');
      });
    let result: string | undefined;
    const value = kind === 'equal-text' ? text : 'Independent Agent draft';
    act(() => {
      hook.result.current.setContent(value);
      if (kind === 'same-event') result = transfer();
    });
    if (kind !== 'same-event')
      act(() => {
        result = transfer();
      });
    expect(result).toBe('retained');
    expect(hook.result.current.content).toBe(value);
    expect(readPreparationDraft(scope)).toEqual(source);
  },
);

it('refuses an independently stored command-bound draft even when mounted memory was empty', () => {
  const { source, transfer } = setup();
  saveAgentDraft(owner, 'agt_1', {
    revision: 'original',
    content: 'Original pending body',
    attachments: [],
    commandKey: 'original-key',
  });
  act(() => expect(transfer()).toBe('retained'));
  expect(readAgentDraft(owner, 'agt_1')).toMatchObject({
    content: 'Original pending body',
    commandKey: 'original-key',
  });
  expect(readPreparationDraft(scope)).toEqual(source);
});

it('does not transfer while the existing draft has a submission in flight', () => {
  const { hook, source, transfer } = setup();
  const submission = hook.result.current.beginSubmission!('Pending input');
  act(() => expect(transfer()).toBe('retained'));
  expect(readPreparationDraft(scope)).toEqual(source);
  submission.settled();
  act(() => expect(transfer()).toBe('transferred'));
});

it.each(['owner', 'client', 'agent', 'unmount'] as const)(
  'rejects a captured destination after %s changes',
  (change) => {
    const { hook, source, client } = setup();
    const destination = hook.result.current;
    if (change === 'unmount') hook.unmount();
    else
      hook.rerender({
        client: change === 'client' ? agentClient() : client,
        viewerId: change === 'owner' ? 'other' : owner,
        agentId: change === 'agent' ? 'other' : 'agt_1',
      });
    act(() =>
      expect(transferPreparationDraft(source, owner, binding, destination, () => true)).toBe(
        'retained',
      ),
    );
    expect(readPreparationDraft(scope)).toEqual(source);
    expect(readAgentDraft(owner, 'agt_1').content).toBe('');
  },
);

it.each(['operationId', 'agentId', 'sandboxId', 'workspaceId', 'worktreeId'] as const)(
  'keeps the source if the current %s does not match its accepted binding',
  (field) => {
    const { hook, source } = setup();
    act(() =>
      expect(
        transferPreparationDraft(
          source,
          owner,
          { ...binding, [field]: 'other' },
          hook.result.current,
          () => true,
        ),
      ).toBe('retained'),
    );
    expect(readPreparationDraft(scope)).toEqual(source);
    expect(readAgentDraft(owner, 'agt_1').content).toBe('');
  },
);

it('keeps a source revision changed after the offer without copying stale text', () => {
  const { source, transfer } = setup();
  const newer = savePreparationDraft(scope, source, 'Later source edit');
  act(() => expect(transfer()).toBe('retained'));
  expect(readPreparationDraft(scope)).toEqual(newer);
  expect(readAgentDraft(owner, 'agt_1').content).toBe('');
});

it.each(['unreadable', 'write-failure', 'readback-failure', 'source-removal'] as const)(
  'preserves the source across %s',
  (failure) => {
    const { source, transfer } = setup();
    const key = agentDraftKey(owner, 'agt_1');
    const originalGet = Storage.prototype.getItem;
    const originalSet = Storage.prototype.setItem;
    const originalRemove = Storage.prototype.removeItem;
    let written = false;
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(function (this: Storage, name) {
      if (name === key && (failure === 'unreadable' || (failure === 'readback-failure' && written)))
        throw new Error('storage read failed');
      return originalGet.call(this, name);
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
      this: Storage,
      name,
      value,
    ) {
      if (name === key && failure === 'write-failure') throw new Error('quota');
      originalSet.call(this, name, value);
      if (name === key) written = true;
    });
    vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(function (this: Storage, name) {
      if (name === preparationDraftKey(scope) && failure === 'source-removal')
        throw new Error('removal refused');
      originalRemove.call(this, name);
    });
    act(() => expect(transfer()).toBe('retained'));
    expect(readPreparationDraft(scope)).toEqual(source);
    vi.restoreAllMocks();
    if (failure === 'readback-failure' || failure === 'source-removal')
      expect(readAgentDraft(owner, 'agt_1').content).toBe(text);
  },
);

it('refuses a valid source for another Agent even if the caller supplies that binding', () => {
  const { source, hook } = setup();
  const otherScope = { ...scope, intentKey: 'another-request' };
  const otherBinding = { ...binding, agentId: 'other-agent' };
  const other = savePreparationDraft(otherScope, undefined, 'Other draft', otherBinding);
  act(() =>
    expect(
      transferPreparationDraft(other, owner, otherBinding, hook.result.current, () => true),
    ).toBe('retained'),
  );
  expect(readAgentDraft(owner, 'agt_1').content).toBe('');
  expect(readPreparationDraft(otherScope)).toEqual(other);
  expect(readPreparationDraft(scope)).toEqual(source);
});

it('keeps the source when authorization expires during destination readback', () => {
  const { source, transfer } = setup();
  const originalGet = Storage.prototype.getItem;
  const originalSet = Storage.prototype.setItem;
  let written = false;
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage, key, value) {
    originalSet.call(this, key, value);
    if (key === agentDraftKey(owner, 'agt_1')) written = true;
  });
  vi.spyOn(Storage.prototype, 'getItem').mockImplementation(function (this: Storage, key) {
    if (written && key === preparationDraftKey(scope)) throw new Error('Authority unavailable');
    return originalGet.call(this, key);
  });
  act(() => expect(transfer()).toBe('retained'));
  vi.restoreAllMocks();
  expect(readPreparationDraft(scope)).toEqual(source);
  expect(readAgentDraft(owner, 'agt_1').content).toBe(text);
});
