import { beforeEach, expect, it, vi } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { conversationFromTranscript } from './conversation';
import type { AgentResourceState } from './controllerTypes';
import { readPreparation } from './preparationRead';
import { harness, json, page } from './transport/controller.fixture';
import { IdentityChangedError } from './transport/identity';

beforeEach(() => sessionStorage.clear());

async function preparing() {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const state: { current: AgentResourceState } = {
    current: {
      status: 'ready',
      detail: { ...AGENT_DETAIL, status: 'starting' },
      workspace: workspaceFixture(),
      conversation: conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
      v2: session,
    },
  };
  const select = vi.spyOn(h.client.agentTransport!, 'openForLoad');
  const publish = vi.fn();
  const read = () =>
    readPreparation(h.client, 'agt_1', new AbortController().signal, state, publish);
  return { ...h, session, state, select, publish, read };
}

it('refreshes startup through the selected transport with fresh account checks', async () => {
  const h = await preparing();
  h.currentUser.mockClear();
  const pending = h.session.journal.retain('Retain the pending message');
  const result = await h.read();
  expect(result.detail.status).toBe('ready');
  expect(result.v2?.journal.read()).toEqual(pending);
  expect(result.conversation.agentIncarnation).toBe('a'.repeat(64));
  expect(h.select).not.toHaveBeenCalled();
  expect(h.currentUser).toHaveBeenCalledTimes(3);
  expect(h.snapshot).toHaveBeenCalledOnce();
  expect(h.post).not.toHaveBeenCalled();
});

it('rejects an account change before reading the retained transport history', async () => {
  const h = await preparing();
  h.currentUser.mockResolvedValueOnce('alice').mockResolvedValue('bob');
  await expect(h.read()).rejects.toBeInstanceOf(IdentityChangedError);
  expect(h.snapshot).not.toHaveBeenCalled();
  expect(h.publish).not.toHaveBeenCalled();
  expect(() => h.session.journal.read()).toThrow(IdentityChangedError);
});

it('rejects history read through the retained transport if the account changes during it', async () => {
  const h = await preparing();
  h.snapshot.mockImplementationOnce(async () => {
    h.currentUser.mockResolvedValue('bob');
    return json(page([], 0));
  });
  await expect(h.read()).rejects.toBeInstanceOf(IdentityChangedError);
  expect(h.snapshot).toHaveBeenCalledOnce();
  expect(h.publish).not.toHaveBeenCalled();
  expect(h.select).not.toHaveBeenCalled();
});
