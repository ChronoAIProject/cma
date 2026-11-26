import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { CmaWebClient } from '../api/client';
import { useNavigationOpeningPreparation } from './useOpeningPreparation';

afterEach(() => vi.restoreAllMocks());

function client(readAccessible: ReturnType<typeof vi.fn>, prepare: ReturnType<typeof vi.fn>) {
  const detail = {
    id: 'agt_1',
    workspace: { id: 'wks_1' },
    sandbox: { id: 'sbx_1' },
  };
  return {
    resourceViews: { agents: { readAccessible } },
    agentTransport: { prepareNavigation: prepare },
    readAgent: vi.fn().mockResolvedValue(detail),
    readWorkspace: vi.fn().mockResolvedValue({ id: 'wks_1', sandboxId: 'sbx_1' }),
    readAgentTranscript: vi.fn().mockResolvedValue({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
    readAgentUsage: vi.fn().mockResolvedValue({ agentId: 'agt_1', throughSeq: 0 }),
  } as unknown as CmaWebClient;
}

it('classifies a personal row before retaining the opening transport', async () => {
  const readAccessible = vi.fn().mockResolvedValue({
    id: 'agt_1',
    workspace: { id: 'wks_1' },
    sandbox: { id: 'sbx_1' },
  });
  const release = vi.fn();
  const prepare = vi.fn().mockReturnValue({
    release,
    handoff: vi.fn(),
    authorizeRead: vi.fn().mockResolvedValue({ owner: 'alice', assertCurrent: vi.fn() }),
  });
  const preparedClient = client(readAccessible, prepare);
  const view = renderHook(() => useNavigationOpeningPreparation(preparedClient, 'alice'));

  act(() => view.result.current('agt_1', true));
  await waitFor(() => expect(prepare).toHaveBeenCalledWith('agt_1', 'alice'));
  await waitFor(() => expect(preparedClient.readAgent).toHaveBeenCalledOnce());
  expect(preparedClient.readWorkspace).toHaveBeenCalledWith(
    'wks_1',
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(preparedClient.readAgentTranscript).toHaveBeenCalledOnce();
  expect(preparedClient.readAgentUsage).toHaveBeenCalledOnce();
  expect(readAccessible).toHaveBeenCalledWith(
    'agt_1',
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );

  act(() => view.result.current('agt_1', false));
  await waitFor(() => expect(release).toHaveBeenCalledOnce());
  expect(prepare.mock.results[0]?.value.authorizeRead).toHaveBeenCalledOnce();
  view.unmount();
});

it('does not prepare shared Agents and aborts a cancelled classification', async () => {
  const sharedRead = vi.fn().mockResolvedValue({ id: 'agt_shared', access: 'shared' });
  const prepare = vi.fn();
  const shared = renderHook(() =>
    useNavigationOpeningPreparation(client(sharedRead, prepare), 'alice'),
  );
  act(() => shared.result.current('agt_shared', true));
  await waitFor(() => expect(sharedRead).toHaveBeenCalledOnce());
  expect(prepare).not.toHaveBeenCalled();
  shared.unmount();

  let signal!: AbortSignal;
  const pendingRead = vi.fn((_id: string, options: { signal: AbortSignal }) => {
    signal = options.signal;
    return new Promise<never>(() => undefined);
  });
  const pending = renderHook(() =>
    useNavigationOpeningPreparation(client(pendingRead, prepare), 'alice'),
  );
  act(() => pending.result.current('agt_pending', true));
  await waitFor(() => expect(pendingRead).toHaveBeenCalledOnce());
  pending.unmount();
  expect(signal.aborted).toBe(true);
});

it('does not prepare runtime, usage or events for a retired navigation row', async () => {
  const readAccessible = vi.fn().mockResolvedValue({
    id: 'agt_1',
    workspace: { id: 'wks_1' },
    sandbox: { id: 'sbx_1', status: 'retired' },
  });
  const prepare = vi.fn();
  const source = client(readAccessible, prepare);
  const view = renderHook(() => useNavigationOpeningPreparation(source, 'alice'));
  act(() => view.result.current('agt_1', true));
  await act(async () => {});
  expect(readAccessible).toHaveBeenCalledOnce();
  expect(prepare).not.toHaveBeenCalled();
  expect(source.readAgent).not.toHaveBeenCalled();
  expect(source.readWorkspace).not.toHaveBeenCalled();
  expect(source.readAgentTranscript).not.toHaveBeenCalled();
  expect(source.readAgentUsage).not.toHaveBeenCalled();
});
