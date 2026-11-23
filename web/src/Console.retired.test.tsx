import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import { ConsoleContent } from './ConsoleContent';
import type { ConsoleProps } from './ConsoleTypes';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from './test/agent';
import { persistDeletion, restoreDeletions } from './deletionRecovery';
import type { SelectedRetiredAgent } from './useDeletionNoticeReconciliation';
import { OwnerViewsProvider } from './resources/retainedOwnerViews';

afterEach(() => vi.restoreAllMocks());

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
  await act(async () => {});
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

it.each(['ready', 'error'] as const)(
  'unmounts runtime panels throughout retained loading/%s/unconfirmed and restores the active selection',
  async (outcome) => {
    const witness: SelectedRetiredAgent = { detail: retired, viewerId: 'owner', authEpoch: 7 };
    const client = agentClient();
    const transcript = vi.spyOn(client, 'readAgentTranscript');
    const replay = vi.spyOn(client, 'replayAgent');
    const draw = (selectedRetired?: SelectedRetiredAgent, authEpoch = 7) => (
      <CmaClientProvider client={client}>
        <OwnerViewsProvider viewerId="owner">
          <ConsoleContent
            {...props}
            settlements={undefined}
            authEpoch={authEpoch}
            selectedRetired={selectedRetired}
          />
        </OwnerViewsProvider>
      </CmaClientProvider>
    );
    const view = render(draw());
    const message = await screen.findByRole('textbox', { name: 'Message' });
    fireEvent.change(message, { target: { value: 'Keep this unsent draft' } });
    const history = screen.getByText('The repository is ready.');
    await waitFor(() => expect(replay).toHaveBeenCalled());
    const liveSignal = replay.mock.lastCall![1].signal;
    if (outcome === 'ready') {
      fireEvent.click(screen.getByRole('button', { name: 'Show Agent computer' }));
      fireEvent.click(screen.getByRole('tab', { name: 'Files' }));
    } else {
      fireEvent.click(screen.getByRole('button', { name: 'Show Agent details' }));
    }
    const selection = outcome === 'ready' ? 'Agent computer' : 'Agent details';
    expect(screen.getByRole('complementary', { name: selection })).toBeVisible();
    let resolve!: (value: typeof AGENT_TRANSCRIPT) => void;
    let reject!: (error: Error) => void;
    transcript.mockImplementationOnce(
      () =>
        new Promise((accept, refuse) => {
          resolve = accept;
          reject = refuse;
        }),
    );
    const before = transcript.mock.calls.length;
    view.rerender(draw(witness));
    await waitFor(() => expect(transcript).toHaveBeenCalledTimes(before + 1));
    expect(transcript).toHaveBeenLastCalledWith(
      'agt_1',
      expect.objectContaining({ cache: 'no-store', maxPages: 256 }),
    );
    expect(liveSignal?.aborted).toBe(true);
    const expectRetained = () => {
      expect(document.querySelector('.agent-side-panel')).toBeNull();
      expect(document.querySelector('.agent-computer')).toBeNull();
      expect(document.querySelector('.frame')).not.toHaveAttribute('data-details-overlay');
      expect(document.querySelector('.frame')).not.toHaveAttribute('data-details-inline');
      expect(screen.getByRole('textbox', { name: 'Message' })).toBe(message);
      expect(message).toHaveValue('Keep this unsent draft');
      expect(screen.getByText('The repository is ready.')).toBe(history);
    };
    expectRetained();
    await act(async () => {
      if (outcome === 'ready') resolve(AGENT_TRANSCRIPT);
      else reject(new Error('Retained history unavailable'));
    });
    expectRetained();
    const replays = replay.mock.calls.length;
    view.rerender(draw());
    expectRetained();
    expect(replay).toHaveBeenCalledTimes(replays);
    view.rerender(draw(undefined, 8));
    await waitFor(() => expect(replay.mock.calls.length).toBeGreaterThan(replays));
    expect(screen.getByRole('complementary', { name: selection })).toBeVisible();
    if (outcome === 'ready')
      expect(screen.getByRole('tab', { name: 'Files' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(message);
    expect(message).toHaveValue('Keep this unsent draft');
    expect(screen.getByText('The repository is ready.')).toBe(history);
  },
);
