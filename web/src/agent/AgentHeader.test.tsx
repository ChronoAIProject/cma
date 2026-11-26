import { createRef } from 'react';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { AgentHeader } from './AgentHeader';
import { agentHeaderMenu } from './headerActions';
import type { TreePermissions } from '../shell/Tree';
import { sandboxFixture } from '../test/hierarchy';

const permissions: TreePermissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

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

it('retains Agent provenance and recovery supplied alongside curated actions', async () => {
  const user = userEvent.setup();
  const onMenuSelect = vi.fn();
  render(
    draw({
      onMenuSelect,
      menu: [
        { id: 'rename-agent', label: 'Rename' },
        { type: 'label', id: 'creation-origin', text: 'Created by Trigger' },
        { id: 'published-trigger', label: 'Published Trigger: Review', href: '/t/trigger-1' },
        { id: 'subscription-sign-in', label: 'Sign in to your ChatGPT subscription again' },
      ],
    }),
  );

  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  expect(screen.getByText('Created by Trigger')).toBeVisible();
  expect(screen.getByRole('menuitem', { name: 'Published Trigger: Review' })).toHaveAttribute(
    'href',
    '/t/trigger-1',
  );
  expect(screen.getAllByRole('menuitem', { name: /^Rename$/ })).toHaveLength(1);
  await user.click(
    screen.getByRole('menuitem', { name: 'Sign in to your ChatGPT subscription again' }),
  );
  expect(onMenuSelect).toHaveBeenCalledExactlyOnceWith('subscription-sign-in');
});

it('retains a reason while access is uncertain and omits unavailable capabilities', async () => {
  const user = userEvent.setup();
  const onDetailsOpenChange = vi.fn();
  render(
    draw({
      detailsUnavailable: true,
      onDetailsOpenChange,
      moreActions: agentHeaderMenu(
        'ready',
        { ...permissions, stopAgent: false },
        { sandbox: sandboxFixture() },
      ),
    }),
  );
  const row = screen.getByRole('button', { name: /^Show Agent details/ });
  expect(row).toBeDisabled();
  await user.click(row);
  expect(onDetailsOpenChange).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
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
        { id: 'export_history', label: 'Export raw history' },
        { id: 'export_retained_conversation', label: 'Export retained conversation' },
        { id: 'stop_agent', label: 'Stop agent' },
        { id: 'delete_agent', label: 'Delete task', danger: true },
      ],
    }),
  );
  const user = userEvent.setup();
  for (const [label, id] of [
    ['Update system prompt', 'update-prompt'],
    ['Update skills', 'update-skills'],
    ['Export raw history', 'export_history'],
    ['Export retained conversation', 'export_retained_conversation'],
  ] as const) {
    await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
    expect(screen.getAllByRole('menuitem').map((item) => item.textContent)).toEqual([
      'Rename',
      'Update system prompt',
      'Update skills',
      'Export retained conversation',
      'Export raw history',
      'View deletion status',
      'Stop agent',
      'Delete task',
    ]);
    expect(screen.getByRole('menu').querySelectorAll('[role="separator"]')).toHaveLength(1);
    await user.click(screen.getByRole('menuitem', { name: label }));
    expect(id.startsWith('export_') ? onMoreAction : onMenuSelect).toHaveBeenCalledWith(id);
  }
});

it.each(['mouse', 'keyboard'] as const)(
  'dispatches the authorized Ready/Completed Stop action by %s and restores focus',
  async (input) => {
    const onMoreAction = vi.fn();
    const onMenuSelect = vi.fn();
    render(
      draw({
        activityStatus: 'completed',
        moreActions: agentHeaderMenu('ready', permissions, { sandbox: sandboxFixture() }),
        onMoreAction,
        onMenuSelect,
      }),
    );
    const user = userEvent.setup();
    const more = screen.getByRole('button', { name: 'More Agent actions' });
    await user.click(more);
    const stop = screen.getByRole('menuitem', { name: 'Stop agent' });
    expect(stop).not.toHaveAttribute('aria-disabled', 'true');
    expect(stop.querySelector('svg')).not.toBeNull();
    const remove = screen.getByRole('menuitem', { name: /^Delete task/ });
    expect(remove).toHaveAttribute('aria-disabled', 'true');
    expect(remove).toHaveTextContent(
      'Current owner access must be checked before stopping and deleting this Agent.',
    );
    if (input === 'mouse') await user.click(stop);
    else {
      await user.keyboard('{Home}');
      for (let index = 0; index < 8 && document.activeElement !== stop; index++)
        await user.keyboard('{ArrowDown}');
      expect(stop).toHaveFocus();
      await user.keyboard('{Enter}');
    }
    expect(onMoreAction).toHaveBeenCalledExactlyOnceWith('stop_agent');
    expect(onMenuSelect).not.toHaveBeenCalled();
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(more).toHaveFocus();
  },
);

it.each([
  'stopped',
  'failed',
  'stopped-without-runtime',
  'failed-without-runtime',
  'runtime-unavailable',
  'sandbox-paused',
  'sandbox-failed',
  'busy',
  'no-permission',
  'shared',
] as const)('preserves sidebar Stop eligibility for %s', async (condition) => {
  const onMoreAction = vi.fn();
  const status = condition.startsWith('stopped')
    ? 'stopped'
    : condition.startsWith('failed')
      ? 'failed'
      : 'ready';
  render(
    draw({
      status,
      moreActions: agentHeaderMenu(
        status,
        condition === 'shared'
          ? undefined
          : { ...permissions, stopAgent: condition !== 'no-permission' },
        {
          sandbox:
            condition === 'runtime-unavailable' || condition.endsWith('without-runtime')
              ? undefined
              : sandboxFixture(
                  condition === 'sandbox-paused'
                    ? 'paused'
                    : condition === 'sandbox-failed'
                      ? 'failed'
                      : 'active',
                ),
          busy: condition === 'busy',
        },
      ),
      onMoreAction,
    }),
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  const stop = screen.queryByRole('menuitem', { name: /^Stop agent/ });
  if (condition === 'shared' || condition === 'no-permission') expect(stop).not.toBeInTheDocument();
  else {
    expect(stop).toHaveAttribute('aria-disabled', 'true');
    if (status === 'stopped') expect(stop).toHaveTextContent('Agent is already stopped.');
    if (status === 'failed') expect(stop).toHaveTextContent('There is no running Agent to stop.');
    if (condition === 'busy')
      expect(stop).toHaveTextContent('Another action is still being confirmed.');
    if (condition === 'runtime-unavailable')
      expect(stop).toHaveTextContent('Sandbox status is unavailable. Refresh and try again.');
    if (condition === 'sandbox-paused') expect(stop).toHaveTextContent('The Sandbox is paused.');
    if (condition === 'sandbox-failed')
      expect(stop).toHaveTextContent('The Sandbox runtime is unavailable.');
    await user.click(stop!);
  }
  expect(onMoreAction).not.toHaveBeenCalled();
  if (condition === 'stopped' || condition === 'failed')
    expect(screen.getByRole('menuitem', { name: 'Delete task' })).not.toHaveAttribute(
      'aria-disabled',
      'true',
    );
});
