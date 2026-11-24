import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { DeletionNotice, DeletionProgress } from './DeletionNotice';
import type { DeletionRequest } from './deletion';
import { useState } from 'react';

function Notice({
  requests,
  disabled,
  onViewStatus,
}: {
  requests: readonly DeletionRequest[];
  disabled: boolean;
  onViewStatus: (request: DeletionRequest) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <DeletionNotice
        requests={requests}
        disabled={disabled}
        onDismiss={() => {}}
        onViewStatus={() => setOpen(true)}
      />
      {open && !disabled && requests.length ? (
        <DeletionProgress
          requests={requests}
          onClose={() => setOpen(false)}
          onViewStatus={(request) => {
            setOpen(false);
            onViewStatus(request);
          }}
        />
      ) : null}
    </>
  );
}

const requests: DeletionRequest[] = [
  { action: { kind: 'delete_sandbox', sandboxId: 'environment' }, key: 'first', accepted: true },
  { action: { kind: 'delete_agent', agentId: 'task' }, key: 'second', accepted: false },
  { action: { kind: 'remove_workspace', workspaceId: 'workspace' }, key: 'third', accepted: true },
];

it('keeps every mixed and unconfirmed request reachable with its original identity', () => {
  const select = vi.fn();
  const view = render(<Notice requests={requests} disabled={false} onViewStatus={select} />);
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
  view.rerender(<Notice requests={requests.slice(1)} disabled={false} onViewStatus={select} />);
  expect(screen.getByRole('status')).toHaveTextContent('2 deletion requests · 1 unconfirmed');
  fireEvent.click(screen.getByRole('button', { name: 'View status' }));
  view.rerender(<Notice requests={[]} disabled={false} onViewStatus={select} />);
  expect(screen.queryByRole('region')).not.toBeInTheDocument();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(select).toHaveBeenCalledTimes(3);
});

it('keeps status unavailable while another action owns the dialog', () => {
  render(<Notice requests={requests} disabled onViewStatus={vi.fn()} />);
  const open = screen.getByRole('button', { name: 'View status' });
  expect(open).toBeDisabled();
  fireEvent.click(open);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});
