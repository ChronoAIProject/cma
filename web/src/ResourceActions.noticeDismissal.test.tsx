import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { useResourceActions } from './ResourceActions';
import { persistDeletion, restoreDeletions } from './deletionRecovery';
import { agentClient } from './test/agent';
import { sandboxFixture } from './test/hierarchy';
import type { CmaWebClient } from './api/client';

function Actions({
  client,
  view = '/agents/current',
}: {
  readonly client: CmaWebClient;
  readonly view?: string;
}) {
  const actions = useResourceActions(
    client,
    vi.fn(),
    undefined,
    'owner',
    undefined,
    true,
    undefined,
    undefined,
    { view, environment: false },
  );
  return (
    <>
      <button onClick={() => actions.select({ kind: 'delete_sandbox', sandboxId: 'second' })}>
        Delete second
      </button>
      <button onClick={actions.viewDeletionStatus}>Deletion progress</button>
      <aside>{actions.environmentNotice}</aside>
      <main>{actions.notice}</main>
      {actions.overlay}
    </>
  );
}

it('reveals a dismissed notice only after a new confirmed deletion and never on retry', async () => {
  persistDeletion('owner', {
    key: 'old-request',
    accepted: true,
    action: { kind: 'delete_sandbox', sandboxId: 'first' },
  });
  const deleteSandbox = vi
    .fn()
    .mockRejectedValueOnce(new Error('Response lost'))
    .mockResolvedValue(undefined);
  const client = agentClient({
    deleteSandbox,
    readSandbox: async (id) => ({ ...sandboxFixture(), id }),
  });
  const view = render(<Actions client={client} />);
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss deletion notice' }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Delete second' })));
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Keep environment' }));
  expect(deleteSandbox).not.toHaveBeenCalled();
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Delete second' })));
  fireEvent.click(screen.getByRole('checkbox'));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Request deletion' })));
  fireEvent.click(screen.getByText('Close', { selector: 'button' }));
  const notice = screen.getByRole('region', { name: 'Deletion request' });
  expect(notice.closest('main')).not.toBeNull();
  expect(notice).toHaveTextContent('2 deletion requests · 1 unconfirmed');
  const original = restoreDeletions('owner').find(
    (request) => request.action.kind === 'delete_sandbox' && request.action.sandboxId === 'second',
  );
  expect(original).toBeDefined();
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss deletion notice' }));
  fireEvent.click(screen.getByRole('button', { name: 'Deletion progress' }));
  const list = screen.getByRole('dialog', { name: 'Deletion progress' });
  await act(async () =>
    fireEvent.click(within(list).getByRole('button', { name: 'View Sandbox deletion status (2)' })),
  );
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Check request' })));
  expect(deleteSandbox).toHaveBeenCalledTimes(2);
  expect(deleteSandbox.mock.calls[1]).toEqual(deleteSandbox.mock.calls[0]);
  expect(restoreDeletions('owner')).toContainEqual({ ...original, accepted: true });
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  view.rerender(<Actions client={client} view="/" />);
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
});

it('does not announce a delayed deletion in a different view', async () => {
  let accept!: () => void;
  const deleteSandbox = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        accept = resolve;
      }),
  );
  const client = agentClient({
    deleteSandbox,
    readSandbox: async (id) => ({ ...sandboxFixture(), id }),
  });
  const view = render(<Actions client={client} />);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Delete second' })));
  fireEvent.click(screen.getByRole('checkbox'));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Request deletion' })));
  view.rerender(<Actions client={client} view="/" />);
  await act(async () => accept());
  expect(
    within(screen.getByRole('main')).queryByRole('region', { name: 'Deletion request' }),
  ).not.toBeInTheDocument();
  expect(screen.getByRole('region', { name: 'Deletion request' }).closest('aside')).not.toBeNull();
  expect(deleteSandbox).toHaveBeenCalledTimes(1);
  expect(restoreDeletions('owner')).toHaveLength(1);
});
