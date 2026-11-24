import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { RuntimeAttentionDisclosure } from './RuntimeAttentionDisclosure';

const failedSandboxes = [
  { sandboxId: 'sbx_first', name: 'First environment', agentIds: ['agt_one', 'agt_two'] },
  { sandboxId: 'sbx_second', name: 'Second environment', agentIds: [] },
];

it('discloses the exact counted environments and navigates without retrying an update', async () => {
  const user = userEvent.setup();
  const onSelect = vi.fn();
  const { container } = render(
    <RuntimeAttentionDisclosure
      attention={{ freshness: 'fresh', failedSandboxes }}
      selectedSandboxId="sbx_second"
      onSelect={onSelect}
    />,
  );
  expect(screen.getByRole('button', { name: 'First environment' })).toBeVisible();
  expect(screen.getByText('2 updates failed · Fix')).toBeVisible();
  expect(container.querySelector('.character-avatar')).toHaveAttribute('data-motion', 'still');
  expect(screen.getByRole('button', { name: 'Second environment' })).toHaveAttribute(
    'aria-current',
    'page',
  );
  expect(screen.getByText('sbx_first')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'First environment' }));
  expect(onSelect).toHaveBeenCalledExactlyOnceWith('sbx_first');
  expect(screen.queryByText(/history.*safe/i)).not.toBeInTheDocument();
});

it('marks stale failures and removes rows when authority is unknown', () => {
  const view = render(
    <RuntimeAttentionDisclosure
      attention={{ freshness: 'stale', failedSandboxes }}
      onSelect={vi.fn()}
    />,
  );
  expect(screen.getByText('2 updates failed (last known) · Fix')).toBeInTheDocument();
  view.rerender(
    <RuntimeAttentionDisclosure
      attention={{ freshness: 'unknown', failedSandboxes: [] }}
      onSelect={vi.fn()}
    />,
  );
  expect(view.container).toBeEmptyDOMElement();
});

it.each([
  [{ selectedAgentId: 'agt_one' }, { selectedAgentId: 'agt_two' }],
  [{ selectedSandboxId: 'sbx_first' }, { selectedSandboxId: 'sbx_second' }],
])('reopens a collapsed disclosure when selection changes from %j to %j', async (first, next) => {
  const user = userEvent.setup();
  const onSelect = vi.fn();
  const attention = { freshness: 'fresh' as const, failedSandboxes };
  const view = render(
    <RuntimeAttentionDisclosure attention={attention} onSelect={onSelect} {...first} />,
  );
  const summary = screen.getByText('2 updates failed · Fix');
  const details = summary.closest('details')!;
  expect(details.open).toBe(true);

  await user.click(summary);
  expect(details.open).toBe(false);
  view.rerender(
    <RuntimeAttentionDisclosure attention={{ ...attention }} onSelect={onSelect} {...first} />,
  );
  expect(details.open).toBe(false);

  view.rerender(<RuntimeAttentionDisclosure attention={attention} onSelect={onSelect} {...next} />);
  expect(details.open).toBe(true);
  expect(onSelect).not.toHaveBeenCalled();
});
