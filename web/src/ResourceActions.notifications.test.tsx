import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useResourceActions } from './ResourceActions';
import { AGENT_DETAIL, agentClient } from './test/agent';
import type { CmaWebClient } from './api/client';
import { useDeletionNoticeReconciliation } from './useDeletionNoticeReconciliation';
import { persistDeletion, restoreDeletions } from './deletionRecovery';
import { sandboxFixture } from './test/hierarchy';
import { downloadAgentHistory } from './agent/historyExport';

vi.mock('./agent/historyExport', () => ({ downloadAgentHistory: vi.fn() }));

it.each([false, true])(
  'never reports a resource change for history export (failure: %s)',
  async (fails) => {
    const changed = vi.fn();
    const exportAgentHistory = vi.fn();
    if (fails) exportAgentHistory.mockRejectedValue(new Error('Export unavailable'));
    else exportAgentHistory.mockResolvedValue(new Blob(['downloaded history']));
    const client = agentClient({ exportAgentHistory });
    const view = renderHook(() => useResourceActions(client, changed));
    await act(async () => view.result.current.select({ kind: 'export_history', agentId: 'agt_1' }));
    expect(changed).not.toHaveBeenCalled();
    expect(exportAgentHistory).toHaveBeenCalledExactlyOnceWith('agt_1', {
      signal: expect.any(AbortSignal),
    });
    if (fails) expect(view.result.current.error).toBe('Export unavailable');
    else expect(downloadAgentHistory).toHaveBeenCalledWith('agt_1', expect.any(Blob));
  },
);

it('delivers a delayed acceptance to the current view callback after navigation', async () => {
  let accept!: () => void;
  const pauseSandbox = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        accept = resolve;
      }),
  );
  const client = agentClient({ pauseSandbox });
  const previous = vi.fn();
  const current = vi.fn();
  const view = renderHook(({ changed }) => useResourceActions(client, changed), {
    initialProps: { changed: previous },
  });
  act(() => view.result.current.select({ kind: 'pause_sandbox', sandboxId: 'sbx_1' }));
  view.rerender({ changed: current });
  await act(async () => accept());
  expect(previous).not.toHaveBeenCalled();
  expect(current).toHaveBeenCalledExactlyOnceWith({ kind: 'pause_sandbox', sandboxId: 'sbx_1' });
});

afterEach(() => vi.restoreAllMocks());

function DeletionNotices({
  source,
  kind,
}: {
  readonly source: CmaWebClient;
  readonly kind: 'Agent' | 'Sandbox';
}) {
  const observed = useDeletionNoticeReconciliation(source, 'owner', 0, true);
  const actions = useResourceActions(
    observed.client,
    () => {},
    undefined,
    'owner',
    undefined,
    true,
    undefined,
    observed.settlements,
  );
  return (
    <>
      <button
        onClick={() =>
          actions.select(
            kind === 'Agent'
              ? { kind: 'delete_agent', agentId: 'second' }
              : { kind: 'delete_sandbox', sandboxId: 'second' },
          )
        }
      >
        Delete second
      </button>
      <button
        onClick={() =>
          void (kind === 'Agent'
            ? observed.client.readAgent('first')
            : observed.client.readSandbox('first'))
        }
      >
        Read first terminal
      </button>
      <output aria-label="Settlements">{observed.revision}</output>
      <output aria-label="Deleting environments">
        {[...actions.deletingSandboxIds].join(',')}
      </output>
      <button onClick={actions.viewDeletionStatus} disabled={!actions.viewDeletionStatus}>
        Open pending request
      </button>
      {actions.notice}
      {actions.overlay}
    </>
  );
}

for (const kind of ['Agent', 'Sandbox'] as const)
  it.each(['healthy', 'write-failure', 'unknown', 'remove-failure'] as const)(
    `retains unrelated ${kind} responsibility when another request settles: %s`,
    async (failure) => {
      const storageFails = failure === 'write-failure' || failure === 'unknown';
      persistDeletion('owner', {
        key: 'synthetic-first-accepted',
        action:
          kind === 'Agent'
            ? { kind: 'delete_agent', agentId: 'first' }
            : { kind: 'delete_sandbox', sandboxId: 'first' },
        accepted: true,
      });
      if (storageFails)
        vi.spyOn(localStorage, 'setItem').mockImplementation(() => {
          throw new DOMException('Synthetic storage full', 'QuotaExceededError');
        });
      const command = vi.fn().mockResolvedValue(undefined);
      if (failure === 'unknown') command.mockRejectedValueOnce(new Error('Response lost'));
      const source = agentClient({
        deleteAgent: command,
        deleteSandbox: command,
        readAgent: async (id) => ({
          ...AGENT_DETAIL,
          id,
          status: id === 'first' ? 'deleted' : 'stopped',
          worktree: {
            ...AGENT_DETAIL.worktree,
            agentId: id,
            status: id === 'first' ? 'removed' : 'ready',
          },
        }),
        readSandbox: async (id) => ({
          ...sandboxFixture(id === 'first' ? 'retired' : 'active'),
          id,
          workspaces: [],
        }),
      });
      render(<DeletionNotices source={source} kind={kind} />);
      await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Delete second' })));
      fireEvent.click(screen.getByRole('checkbox'));
      await act(async () =>
        fireEvent.click(
          screen.getByRole('button', {
            name: kind === 'Agent' ? 'Delete Agent' : 'Request deletion',
          }),
        ),
      );
      expect(command).toHaveBeenCalledExactlyOnceWith('second', {
        idempotencyKey: expect.any(String),
      });
      const original = command.mock.calls[0]![1].idempotencyKey;
      if (failure === 'unknown') {
        expect(await screen.findByText('Response lost')).toBeVisible();
        await act(async () => fireEvent.click(screen.getByText('Close', { selector: 'button' })));
      }
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
      expect(screen.getAllByRole('region', { name: 'Deletion request' })).toHaveLength(1);
      expect(screen.getByRole('region', { name: 'Deletion request' })).toHaveTextContent(
        failure === 'unknown'
          ? '2 deletion requests · 1 unconfirmed'
          : `Deleting 2 ${kind === 'Sandbox' ? 'environments' : 'Agents'}`,
      );
      expect(restoreDeletions('owner')).toHaveLength(storageFails ? 1 : 2);
      if (failure === 'remove-failure')
        vi.spyOn(localStorage, 'removeItem').mockImplementation(() => {
          throw new DOMException('Synthetic removal blocked', 'SecurityError');
        });
      await act(async () =>
        fireEvent.click(screen.getByRole('button', { name: 'Read first terminal' })),
      );
      await waitFor(() => expect(screen.getByLabelText('Settlements')).toHaveTextContent('1'));
      expect(screen.queryAllByRole('region', { name: 'Deletion request' })).toHaveLength(1);
      if (kind === 'Sandbox')
        expect(screen.getByLabelText('Deleting environments').textContent).toBe(
          failure === 'unknown' ? '' : 'second',
        );
      await act(async () =>
        fireEvent.click(screen.getByRole('button', { name: 'Open pending request' })),
      );
      const progress = screen.getByRole('dialog', { name: 'Deletion progress' });
      expect(within(progress).getAllByRole('listitem')).toHaveLength(1);
      const row = within(progress).getByRole('listitem');
      expect(within(row).getByRole('code')).toHaveTextContent('second');
      await act(async () => fireEvent.click(within(row).getByRole('button')));
      const status = await screen.findByRole('dialog', { name: `Deleting ${kind}` });
      expect(within(status).getByText(original)).toBeVisible();
      expect(command).toHaveBeenCalledTimes(1);
      if (failure === 'unknown') {
        expect(within(status).getByText('Request unconfirmed')).toBeVisible();
        await act(async () =>
          fireEvent.click(within(status).getByRole('button', { name: 'Check request' })),
        );
        expect(command.mock.calls[1]).toEqual(command.mock.calls[0]);
      } else {
        await act(async () =>
          fireEvent.click(within(status).getByText('Close', { selector: 'button' })),
        );
      }
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
      expect(screen.queryAllByRole('region', { name: 'Deletion request' })).toHaveLength(1);
    },
  );

it('resets the open aggregate when the viewer changes without reading or submitting requests', async () => {
  for (const viewer of ['first-owner', 'second-owner'])
    persistDeletion(viewer, {
      key: viewer,
      accepted: true,
      action: { kind: 'delete_agent', agentId: viewer },
    });
  const source = agentClient({ readAgent: vi.fn(), deleteAgent: vi.fn() });
  function Notice({ viewer }: { readonly viewer: string }) {
    return useResourceActions(source, vi.fn(), undefined, viewer).notice;
  }
  const view = render(<Notice viewer="first-owner" />);
  fireEvent.click(screen.getByRole('button', { name: 'View status' }));
  expect(screen.getByRole('dialog')).toHaveTextContent('first-owner');
  view.rerender(<Notice viewer="second-owner" />);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'View status' }));
  expect(screen.getByRole('dialog')).toHaveTextContent('second-owner');
  expect(screen.getByRole('dialog')).not.toHaveTextContent('first-owner');
  expect(source.readAgent).not.toHaveBeenCalled();
  expect(source.deleteAgent).not.toHaveBeenCalled();
});

it('withdraws the aggregate status list while current access is uncertain', () => {
  persistDeletion('owner', {
    key: 'synthetic-access-gated-request',
    accepted: true,
    action: { kind: 'delete_agent', agentId: 'task' },
  });
  const source = agentClient({ readAgent: vi.fn(), deleteAgent: vi.fn() });
  function Notice({ enabled }: { readonly enabled: boolean }) {
    return useResourceActions(source, vi.fn(), undefined, 'owner', undefined, enabled).notice;
  }
  const view = render(<Notice enabled />);
  fireEvent.click(screen.getByRole('button', { name: 'View status' }));
  expect(screen.getByRole('dialog', { name: 'Deletion progress' })).toBeVisible();
  view.rerender(<Notice enabled={false} />);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  const open = screen.getByRole('button', { name: 'View status' });
  expect(open).toBeDisabled();
  fireEvent.click(open);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(source.readAgent).not.toHaveBeenCalled();
  expect(source.deleteAgent).not.toHaveBeenCalled();
});
