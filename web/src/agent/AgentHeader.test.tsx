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
  expect(screen.getByRole('button', { name: 'More Agent actions' })).toHaveTextContent(
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
      menu: [{ id: 'rename', label: 'Rename' }],
      moreActions: [
        { id: 'stop_agent', label: 'Stop agent' },
        { id: 'delete_agent', label: 'Delete agent', danger: true },
      ],
    }),
  );
  await user.tab();
  await user.keyboard('{Enter}');
  expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveFocus();
  await user.keyboard('{Enter}');
  expect(onMenuSelect).toHaveBeenCalledWith('rename');
  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  await user.click(screen.getByRole('menuitem', { name: 'Show Agent details' }));
  expect(onDetailsOpenChange).toHaveBeenCalledWith(true);
  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  await user.click(screen.getByRole('menuitem', { name: 'Stop agent' }));
  expect(onMoreAction).toHaveBeenCalledWith('stop_agent');
});

it('places Files after Share and reports its actual expanded state', async () => {
  const user = userEvent.setup();
  const onShare = vi.fn();
  const onFilesOpenChange = vi.fn();
  const { rerender } = render(draw({ onShare, onFilesOpenChange }));
  expect(
    screen
      .getAllByRole('button')
      .map((item) => item.getAttribute('aria-label') ?? item.textContent),
  ).toEqual(['More Agent actions', 'Share', 'Show files']);
  await user.click(screen.getByRole('button', { name: 'Share' }));
  expect(onShare).toHaveBeenCalledOnce();
  await user.click(screen.getByRole('button', { name: 'Show files' }));
  expect(onFilesOpenChange).toHaveBeenCalledWith(true);
  rerender(draw({ filesOpen: true, onFilesOpenChange }));
  expect(screen.getByRole('button', { name: 'Hide files' })).toHaveAttribute(
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
  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  const row = screen.getByRole('menuitem', { name: /^Show Agent details/ });
  expect(row).toHaveAttribute('aria-disabled', 'true');
  await user.click(row);
  expect(onDetailsOpenChange).not.toHaveBeenCalled();
  expect(screen.queryByRole('menuitem', { name: 'Stop agent' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Share' })).not.toBeInTheDocument();
});
