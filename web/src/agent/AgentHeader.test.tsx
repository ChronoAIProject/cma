import { createRef } from 'react';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { AgentHeader } from './AgentHeader';

function draw(overrides: Partial<Parameters<typeof AgentHeader>[0]> = {}) {
  return (
    <AgentHeader
      title="Prove lemma 4.2"
      status="ready"
      breadcrumb={[]}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      {...overrides}
    />
  );
}

it('shows the compact title immediately with no pin or hover strip', () => {
  render(draw());
  expect(screen.getByRole('heading', { level: 1, name: 'Prove lemma 4.2' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Show Agent details' })).toHaveTextContent(
    'Prove lemma 4.2',
  );
  expect(screen.queryByRole('button', { name: /Pin header/ })).not.toBeInTheDocument();
  expect(document.querySelector('.agent-header__strip')).toBeNull();
});

it('keeps the heading focusable for arrival without opening the menu', () => {
  const heading = createRef<HTMLHeadingElement>();
  render(draw({ headingRef: heading }));
  act(() => heading.current!.focus());
  expect(heading.current).toHaveFocus();
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
});

it('opens actions by keyboard and preserves navigation, Details and destructive dispatch', async () => {
  const user = userEvent.setup();
  const onDetailsOpenChange = vi.fn();
  const onMenuSelect = vi.fn();
  const onMoreAction = vi.fn();
  render(
    draw({
      onDetailsOpenChange,
      onMenuSelect,
      onMoreAction,
      menu: [{ id: 'rename-agent', label: 'Rename' }],
      moreActions: [
        { id: 'stop_agent', label: 'Stop agent' },
        { id: 'delete_agent', label: 'Delete agent', danger: true },
      ],
    }),
  );
  await user.tab();
  expect(screen.getByRole('button', { name: 'Show Agent details' })).toHaveFocus();
  await user.tab();
  await user.keyboard('{Enter}');
  expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveFocus();
  await user.keyboard('{Enter}');
  expect(onMenuSelect).toHaveBeenCalledWith('rename-agent');
  await user.click(screen.getByRole('button', { name: 'Show Agent details' }));
  expect(onDetailsOpenChange).toHaveBeenCalledWith(true);
  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  await user.click(screen.getByRole('menuitem', { name: 'Delete task' }));
  expect(onMoreAction).toHaveBeenCalledWith('delete_agent');
});

it('places Computer after Share and before More and reports its actual expanded state', async () => {
  const user = userEvent.setup();
  const onShare = vi.fn();
  const onFilesOpenChange = vi.fn();
  const { rerender } = render(draw({ onShare, onFilesOpenChange }));
  expect(
    screen
      .getAllByRole('button')
      .map((item) => item.getAttribute('aria-label') ?? item.textContent),
  ).toEqual(['Show Agent details', 'Share', 'Show Agent computer', 'More Agent actions']);
  await user.click(screen.getByRole('button', { name: 'Share' }));
  expect(onShare).toHaveBeenCalledOnce();
  await user.click(screen.getByRole('button', { name: 'Show Agent computer' }));
  expect(onFilesOpenChange).toHaveBeenCalledWith(true);
  rerender(draw({ filesOpen: true, onFilesOpenChange }));
  expect(screen.getByRole('button', { name: 'Hide Agent computer' })).toHaveAttribute(
    'aria-expanded',
    'true',
  );
});

it('retains a reason while access is uncertain and omits unavailable capabilities', async () => {
  const user = userEvent.setup();
  const onDetailsOpenChange = vi.fn();
  render(
    draw({
      detailsUnavailable: true,
      onDetailsOpenChange,
      moreActions: [{ id: 'stop_agent', label: 'Stop agent' }],
    }),
  );
  const row = screen.getByRole('button', { name: /^Show Agent details/ });
  expect(row).toBeDisabled();
  await user.click(row);
  expect(onDetailsOpenChange).not.toHaveBeenCalled();
  expect(screen.queryByRole('menuitem', { name: 'Stop agent' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Share' })).not.toBeInTheDocument();
});

it('orders all reviewed actions and dispatches configuration separately from history export', async () => {
  const onMenuSelect = vi.fn(),
    onMoreAction = vi.fn(),
    onViewDeletionStatus = vi.fn();
  render(
    draw({
      onMenuSelect,
      onMoreAction,
      onViewDeletionStatus,
      menu: [
        { id: 'rename-agent', label: 'Rename' },
        { id: 'update-prompt', label: 'Update system prompt' },
        { id: 'update-skills', label: 'Update skills' },
      ],
      moreActions: [
        { id: 'export_history', label: 'Export history' },
        { id: 'delete_agent', label: 'Delete task', danger: true },
      ],
    }),
  );
  const user = userEvent.setup();
  for (const [label, id] of [
    ['Update system prompt', 'update-prompt'],
    ['Update skills', 'update-skills'],
    ['Export history', 'export_history'],
  ] as const) {
    await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
    expect(screen.getAllByRole('menuitem').map((item) => item.textContent)).toEqual([
      'Rename',
      'Update system prompt',
      'Update skills',
      'Export history',
      'View deletion status',
      'Delete task',
    ]);
    expect(screen.getByRole('menu').querySelectorAll('[role="separator"]')).toHaveLength(1);
    await user.click(screen.getByRole('menuitem', { name: label }));
    expect(id === 'export_history' ? onMoreAction : onMenuSelect).toHaveBeenCalledWith(id);
  }
});
