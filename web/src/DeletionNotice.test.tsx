import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { useState } from 'react';
import { DeletionNotice, DeletionProgress } from './DeletionNotice';
import type { DeletionRequest } from './deletion';

const requests: DeletionRequest[] = [
  { action: { kind: 'delete_sandbox', sandboxId: 'environment' }, key: 'first', accepted: true },
  { action: { kind: 'delete_agent', agentId: 'task' }, key: 'second', accepted: false },
  { action: { kind: 'remove_workspace', workspaceId: 'workspace' }, key: 'third', accepted: true },
];

it('keeps every mixed and unconfirmed request reachable with its original identity', () => {
  const select = vi.fn();
  function Notices({ pending }: { readonly pending: readonly DeletionRequest[] }) {
    const [open, setOpen] = useState(false);
    return (
      <>
        <DeletionNotice
          requests={pending}
          disabled={false}
          onViewStatus={() => setOpen(true)}
          onDismiss={vi.fn()}
        />
        {open && pending.length > 0 ? (
          <DeletionProgress
            requests={pending}
            onClose={() => setOpen(false)}
            onViewStatus={(request) => {
              select(request);
              setOpen(false);
            }}
          />
        ) : null}
      </>
    );
  }
  const view = render(<Notices pending={requests} />);
  expect(screen.getByRole('status')).toHaveTextContent('3 deletion requests · 1 unconfirmed');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  for (const [index, request] of requests.entries()) {
    fireEvent.click(screen.getByRole('button', { name: 'View status' }));
    const progress = screen.getByRole('dialog', { name: 'Deletion progress' });
    expect(within(progress).getAllByRole('listitem')).toHaveLength(3);
    expect(within(progress).getByText('Agent delete request is not confirmed.')).toBeVisible();
    const row = within(progress).getAllByRole('listitem')[index]!;
    expect(within(row).getByRole('code')).toHaveTextContent(
      ['environment', 'task', 'workspace'][index]!,
    );
    fireEvent.click(within(row).getByRole('button'));
    expect(select).toHaveBeenLastCalledWith(request);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  }
  view.rerender(<Notices pending={requests.slice(1)} />);
  expect(screen.getByRole('status')).toHaveTextContent('2 deletion requests · 1 unconfirmed');
  fireEvent.click(screen.getByRole('button', { name: 'View status' }));
  view.rerender(<Notices pending={[]} />);
  expect(screen.queryByRole('region')).not.toBeInTheDocument();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(select).toHaveBeenCalledTimes(3);
});

it('keeps status unavailable while another action owns the dialog', () => {
  const openStatus = vi.fn();
  render(
    <DeletionNotice requests={requests} disabled onViewStatus={openStatus} onDismiss={vi.fn()} />,
  );
  const open = screen.getByRole('button', { name: 'View status' });
  expect(open).toBeDisabled();
  fireEvent.click(open);
  expect(openStatus).not.toHaveBeenCalled();
});
