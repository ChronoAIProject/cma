import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { useResourceActions } from './ResourceActions';
import { agentClient } from './test/agent';
import { downloadAgentHistory } from './agent/historyExport';
import type { CmaWebClient } from './api/client';
import type { AgentEvent, AgentTranscript } from './api/types';

vi.mock('./agent/historyExport', () => ({ downloadAgentHistory: vi.fn() }));
const transcript: AgentTranscript = { agentId: 'agt_1', throughSeq: 3, turns: [] };
const event = (seq: number, kind: AgentEvent['kind']): AgentEvent => ({
  seq,
  kind,
  appendedAt: '2026-10-12T00:00:00Z',
});
beforeEach(() => vi.clearAllMocks());

it('exports fresh retained data even when raw runtime history is unavailable, without changing resources', async () => {
  const readAgentTranscript = vi.fn().mockResolvedValue(transcript);
  const exportAgentHistory = vi.fn().mockRejectedValue(new Error('Runtime unavailable'));
  const start = event(2, { type: 'tool_call_started', call_id: 'tool', title: 'Fixture tool' });
  const output = event(3, {
    type: 'tool_output_reported',
    call_id: 'tool',
    output: { text: 'Fixture result', truncated: false },
  });
  const readAgentEvents = vi
    .fn<CmaWebClient['readAgentEvents']>()
    .mockResolvedValueOnce({
      items: [event(1, { type: 'turn_started' }), start],
      next: 2,
    })
    .mockResolvedValueOnce({
      items: [output, event(4, { type: 'tool_call_updated', call_id: 'later' })],
      next: 4,
    });
  const client = agentClient({
    readAgentTranscript,
    readAgentEvents,
    exportAgentHistory,
    stopAgent: vi.fn(),
    reviveSandbox: vi.fn(),
  });
  const changed = vi.fn();
  const view = renderHook(() => useResourceActions(client, changed));
  await act(async () =>
    view.result.current.select({ kind: 'export_retained_conversation', agentId: 'agt_1' }),
  );
  expect(readAgentTranscript).toHaveBeenCalledWith(
    'agt_1',
    expect.objectContaining({
      signal: expect.any(AbortSignal),
      maxPages: 256,
      maxBytes: 16 * 1024 * 1024,
    }),
  );
  expect(readAgentEvents.mock.calls.map(([, options]) => options?.cursor)).toEqual([0, 2]);
  expect(readAgentEvents).toHaveBeenCalledWith(
    'agt_1',
    expect.objectContaining({ signal: expect.any(AbortSignal), cache: 'no-store' }),
  );
  expect(downloadAgentHistory).toHaveBeenCalledExactlyOnceWith('agt_1', {
    transcript,
    toolEvents: [start, output],
  });
  expect(exportAgentHistory).not.toHaveBeenCalled();
  expect(changed).not.toHaveBeenCalled();
  expect(client.stopAgent).not.toHaveBeenCalled();
  expect(client.reviveSandbox).not.toHaveBeenCalled();
});

it('reports an incomplete event read without downloading partial history', async () => {
  const client = agentClient({
    readAgentTranscript: async () => transcript,
    readAgentEvents: async () => ({ items: [event(1, { type: 'turn_started' })] }),
  });
  const view = renderHook(() => useResourceActions(client, vi.fn()));
  await act(async () =>
    view.result.current.select({ kind: 'export_retained_conversation', agentId: 'agt_1' }),
  );
  expect(downloadAgentHistory).not.toHaveBeenCalled();
  expect(view.result.current.error).toMatch(/saved sequence/);
});

it.each(['identity', 'revocation', 'client', 'unmount'])(
  'discards buffered export after %s changes, even if the transport ignores abort',
  async (change) => {
    let resolve!: (value: AgentTranscript) => void;
    const readAgentTranscript = vi.fn<CmaWebClient['readAgentTranscript']>(
      () =>
        new Promise<AgentTranscript>((done) => {
          resolve = done;
        }),
    );
    const original = agentClient({ readAgentTranscript });
    const view = renderHook(
      ({ viewerId, enabled, client }) =>
        useResourceActions(client, vi.fn(), undefined, viewerId, undefined, enabled),
      {
        initialProps: { viewerId: 'first', enabled: true, client: original },
      },
    );
    act(() =>
      view.result.current.select({ kind: 'export_retained_conversation', agentId: 'agt_1' }),
    );
    await waitFor(() => expect(readAgentTranscript).toHaveBeenCalledOnce());
    if (change === 'unmount') view.unmount();
    else
      view.rerender({
        viewerId: change === 'identity' ? 'second' : 'first',
        enabled: change !== 'revocation',
        client: change === 'client' ? agentClient() : original,
      });
    await act(async () => resolve(transcript));
    expect(downloadAgentHistory).not.toHaveBeenCalled();
    const options = readAgentTranscript.mock.calls[0]?.[1] as { signal: AbortSignal };
    expect(options.signal.aborted).toBe(true);
  },
);

it('does not download a failed or foreign transcript', async () => {
  for (const value of [new Error('Access revoked'), { ...transcript, agentId: 'foreign' }]) {
    const readAgentTranscript = vi.fn().mockImplementation(async () => {
      if (value instanceof Error) throw value;
      return value;
    });
    const client = agentClient({ readAgentTranscript });
    const view = renderHook(() => useResourceActions(client, vi.fn()));
    await act(async () =>
      view.result.current.select({ kind: 'export_retained_conversation', agentId: 'agt_1' }),
    );
    expect(downloadAgentHistory).not.toHaveBeenCalled();
    expect(view.result.current.error).toBeDefined();
    view.unmount();
  }
});
