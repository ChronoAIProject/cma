import { act, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import type { ConsoleProps } from './ConsoleTypes';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { persistDeletion, restoreDeletions } from './deletionRecovery';

const props: ConsoleProps = {
  user: { id: 'owner', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] },
  route: { kind: 'agent', agentId: 'agt_1' },
  onNavigate: vi.fn(),
  onSignOut: vi.fn(),
};
const retired = {
  ...AGENT_DETAIL,
  ownerTitle: 'Retained original task',
  sandbox: { ...AGENT_DETAIL.sandbox, status: 'retired' as const },
};

it('projects the authorized selected retired Agent despite an empty hierarchy and no current activity', async () => {
  const request = {
    action: { kind: 'delete_sandbox' as const, sandboxId: 'sbx_1' },
    key: 'accepted-parent',
    accepted: true,
  };
  const sibling = {
    ...request,
    key: 'unknown-sibling',
    accepted: false,
    action: { kind: 'delete_sandbox' as const, sandboxId: 'sbx_other' },
  };
  persistDeletion('owner', request);
  persistDeletion('owner', sibling);
  const client = agentClient({
    readAgent: async () => retired,
    listSandboxes: async () => ({ items: [] }),
  });
  const draw = (changes: Partial<ConsoleProps> = {}) => (
    <CmaClientProvider client={client}>
      <Console {...props} {...changes} />
    </CmaClientProvider>
  );
  const view = render(draw());
  const group = await screen.findByRole('group', { name: 'Retired' });
  expect(
    within(group).getByRole('link', { name: 'Retained original task, Retired' }),
  ).toHaveAttribute('aria-current', 'page');
  expect(group.querySelector('.character-avatar')).toHaveAttribute('data-motion', 'still');
  expect(screen.getByText('No other agents to show.')).toBeVisible();
  expect(restoreDeletions('owner')).toEqual([sibling]);
  expect(
    screen.queryByText('Sandbox delete request accepted. Cleanup continues in the background.'),
  ).not.toBeInTheDocument();
  expect(screen.getByText('Sandbox delete request is not confirmed.')).toBeVisible();
  expect(
    screen.getByText('1 deletions need checking').closest('.manager-navigation__list'),
  ).toBeNull();
  view.rerender(draw({ route: { kind: 'home' } }));
  expect(screen.queryByRole('group', { name: 'Retired' })).not.toBeInTheDocument();
  expect(restoreDeletions('owner')).toEqual([sibling]);
});

it('withdraws a retained sidebar witness immediately on an authentication epoch change', async () => {
  const client = agentClient({
    readAgent: async () => retired,
    listSandboxes: async () => ({ items: [] }),
  });
  const draw = (authEpoch: number) => (
    <CmaClientProvider client={client}>
      <Console {...props} authEpoch={authEpoch} />
    </CmaClientProvider>
  );
  const view = render(draw(1));
  await screen.findByRole('group', { name: 'Retired' });
  view.rerender(draw(2));
  expect(screen.queryByRole('group', { name: 'Retired' })).not.toBeInTheDocument();
});

it('refuses a late retired Agent read from the previous selected route', async () => {
  let finish!: (value: typeof retired) => void;
  const client = agentClient({
    readAgent: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
    listSandboxes: async () => ({ items: [] }),
  });
  const draw = (home: boolean) => (
    <CmaClientProvider client={client}>
      <Console {...props} route={home ? { kind: 'home' } : props.route} />
    </CmaClientProvider>
  );
  const view = render(draw(false));
  await act(async () => {});
  view.rerender(draw(true));
  await act(async () => {
    finish(retired);
  });
  expect(screen.queryByRole('group', { name: 'Retired' })).not.toBeInTheDocument();
});
