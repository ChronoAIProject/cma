import { beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { loadAgent } from './loadAgent';
import { loadV2Agent } from './transport/load';
import { harness } from './transport/controller.fixture';
import type { CmaWebClient } from '../api/client';

function missing(status = 404): NonNullable<CmaWebClient['agentHistory']> {
  return {
    state: vi.fn().mockRejectedValue(new ApiError(status, 'Endpoint unavailable')),
    history: vi.fn(),
  };
}
beforeEach(() => sessionStorage.clear());

it('keeps the selected v2 transport and journal when a predecessor lacks bounded reads', async () => {
  const h = harness();
  const selected = await h.client.agentTransport!.openForLoad('agt_1', {
    signal: new AbortController().signal,
  });
  if (!selected) throw new Error('Expected v2 fixture');
  const choose = vi.spyOn(h.client.agentTransport!, 'openForLoad').mockResolvedValue(selected);
  const before = selected.journal.read();
  const ready = await loadAgent(
    { ...h.client, agentHistory: missing() },
    'agt_1',
    new AbortController().signal,
  );
  expect(choose).toHaveBeenCalledTimes(1);
  expect(ready.v2).toBe(selected);
  expect(selected.journal.read()).toEqual(before);
  expect(h.snapshot).toHaveBeenCalledTimes(1);
  expect(h.currentUser.mock.calls.length).toBeGreaterThan(2);
  expect(h.post).not.toHaveBeenCalled();
  expect(h.client.runAgent).not.toHaveBeenCalled();
});

it('uses the same selected session for a direct v2 loader fallback', async () => {
  const h = harness();
  const selected = await h.client.agentTransport!.open('agt_1');
  if (!selected) throw new Error('Expected v2 fixture');
  const ready = await loadV2Agent(
    { ...h.client, agentHistory: missing() },
    'agt_1',
    new AbortController().signal,
    selected,
  );
  expect(ready.v2).toBe(selected);
  expect(h.snapshot).toHaveBeenCalledTimes(1);
  expect(h.post).not.toHaveBeenCalled();
});

it('does not publish a late bounded attempt after selecting the legacy fallback', async () => {
  let resolve!: (value: typeof AGENT_DETAIL) => void;
  const stale = new Promise<typeof AGENT_DETAIL>((yes) => {
    resolve = yes;
  });
  const readAgent = vi.fn().mockReturnValueOnce(stale).mockResolvedValue(AGENT_DETAIL);
  const publish = vi.fn();
  const ready = await loadAgent(
    agentClient({ agentHistory: missing(), readAgent }),
    'agt_1',
    new AbortController().signal,
    false,
    undefined,
    undefined,
    publish,
  );
  expect(ready.historyLoading).toBe(false);
  const count = publish.mock.calls.length;
  resolve(AGENT_DETAIL);
  await new Promise((yes) => setTimeout(yes, 0));
  expect(publish).toHaveBeenCalledTimes(count);
});

it('does not treat authorization or service failures as missing endpoints', async () => {
  for (const status of [403, 409, 503]) {
    const transcript = vi.fn();
    await expect(
      loadAgent(
        agentClient({ agentHistory: missing(status), readAgentTranscript: transcript }),
        'agt_1',
        new AbortController().signal,
      ),
    ).rejects.toBeInstanceOf(ApiError);
    expect(transcript).not.toHaveBeenCalled();
  }
});
