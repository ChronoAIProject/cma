import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { createClient } from '@cma/client';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import { ConsoleContent } from './ConsoleContent';
import type { ConsoleProps } from './ConsoleTypes';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from './test/agent';
import { persistDeletion, restoreDeletions } from './deletionRecovery';
import type { SelectedRetiredAgent } from './useDeletionNoticeReconciliation';
import { OwnerViewsProvider } from './resources/retainedOwnerViews';
import { WIRE_AGENT } from './test/rest';
import { chooseOption, expandNavigationFilters } from './test/select';
import { retainTaskViewer } from './create/taskIdentity';
import { openTaskDraftStore } from './create/taskDraftStorage';
import { agentDraftKey, readAgentDraft } from './agent/draftStorage';

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
  expect(group.querySelector('img')?.getAttribute('src')).toContain('/rest/poster-');
  expect(screen.getByText('No other agents to show.')).toBeVisible();
  expect(restoreDeletions('owner')).toEqual([sibling]);
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  await userEvent.setup().click(screen.getByRole('button', { name: 'Environments' }));
  expect(screen.getByRole('region', { name: 'Deletion request' })).toHaveTextContent(
    '1 deletion request · 1 unconfirmed',
  );
  expect(
    screen.getByText('1 deletions need checking').closest('.manager-navigation__list'),
  ).toBeNull();
  await userEvent.setup().click(screen.getByRole('button', { name: 'Tasks' }));
  expect(screen.getByRole('group', { name: 'Retired' })).toBeVisible();
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
    let accessGate = Promise.resolve();
    const resourceViews = createClient({
      baseUrl: 'https://cma.test',
      fetch: async (input) => {
        const path = new URL(String(input)).pathname;
        if (path === '/api/v1/organizations') return Response.json({ items: [] });
        if (path === '/api/v1/agents/agt_1') {
          await accessGate;
          return Response.json({
            ...WIRE_AGENT,
            id: 'agt_1',
            worktree: { ...WIRE_AGENT.worktree, id: AGENT_DETAIL.worktree.id },
            workspace: { ...WIRE_AGENT.workspace, id: AGENT_DETAIL.workspace.id },
            sandbox: { ...WIRE_AGENT.sandbox, id: AGENT_DETAIL.sandbox.id },
          });
        }
        if (path.includes('/resource-access/'))
          return Response.json({
            owner: { kind: 'person', is_current_person: true },
            capabilities: ['view_details'],
            policy_revision: 1,
            revalidate_at: null,
          });
        if (path.endsWith('/channel-usage'))
          return Response.json({
            agent_id: 'agt_1',
            channel_usage: {
              active: false,
              has_history: false,
              first_used_at: null,
              last_used_at: null,
            },
          });
        throw new Error(`Unexpected request: ${path}`);
      },
    });
    const readAccessible = vi.spyOn(resourceViews.agents, 'readAccessible');
    const client = agentClient({ resourceViews });
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
    expect(screen.getByText('The repository is ready.')).toBeVisible();
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
    const storedDraft = readAgentDraft('owner', 'agt_1');
    expect(storedDraft.content).toBe('Keep this unsent draft');
    const storedBytes = sessionStorage.getItem(agentDraftKey('owner', 'agt_1'));
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
    expect(screen.getByText('Loading retained conversation.')).toBeVisible();
    const expectRetained = () => {
      expect(document.querySelector('.agent-side-panel')).toBeNull();
      expect(document.querySelector('.agent-computer')).toBeNull();
      expect(document.querySelector('.frame')).not.toHaveAttribute('data-details-overlay');
      expect(document.querySelector('.frame')).not.toHaveAttribute('data-details-inline');
      expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
      expect(readAgentDraft('owner', 'agt_1')).toEqual(storedDraft);
      expect(sessionStorage.getItem(agentDraftKey('owner', 'agt_1'))).toBe(storedBytes);
    };
    expectRetained();
    await act(async () => {
      if (outcome === 'ready') resolve(AGENT_TRANSCRIPT);
      else reject(new Error('Retained history unavailable'));
    });
    expectRetained();
    expect(
      screen.getByText(
        outcome === 'ready' ? 'The repository is ready.' : 'Conversation couldn’t be loaded',
      ),
    ).toBeVisible();
    const replays = replay.mock.calls.length;
    view.rerender(draw());
    expectRetained();
    expect(screen.getByText('Checking access to the retained conversation.')).toBeVisible();
    expect(replay).toHaveBeenCalledTimes(replays);
    const accessReads = readAccessible.mock.calls.length;
    let releaseAccess!: () => void;
    accessGate = new Promise((resolve) => {
      releaseAccess = resolve;
    });
    view.rerender(draw(undefined, 8));
    await waitFor(() => expect(readAccessible).toHaveBeenCalledTimes(accessReads + 1));
    expectRetained();
    expect(replay).toHaveBeenCalledTimes(replays);
    await act(async () => {
      releaseAccess();
    });
    await waitFor(() => expect(replay.mock.calls.length).toBeGreaterThan(replays));
    expect(screen.getByRole('complementary', { name: selection })).toBeVisible();
    if (outcome === 'ready')
      expect(screen.getByRole('tab', { name: 'Files' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Keep this unsent draft');
    expect(screen.getByText('The repository is ready.')).toBeVisible();
  },
);

it('reuses a retired request in personal Home after an explicit organization scope selection', async () => {
  retainTaskViewer('owner');
  const resourceViews = createClient({
    baseUrl: 'https://cma.test',
    fetch: async (input) => {
      const path = new URL(String(input)).pathname;
      if (path === '/api/v1/organizations')
        return Response.json({
          items: [{ id: 'org_research', role: 'admin', display_name: 'Research' }],
        });
      if (path === '/api/v1/agents/agt_1')
        return Response.json({
          ...WIRE_AGENT,
          id: 'agt_1',
          sandbox: { ...WIRE_AGENT.sandbox, status: 'retired' },
        });
      if (path.includes('/resource-access/'))
        return Response.json({
          owner: { kind: 'person', is_current_person: true },
          capabilities: ['view_details'],
          policy_revision: 1,
          revalidate_at: null,
        });
      if (path === '/api/v1/sandboxes' || path === '/api/v1/agents/provenance')
        return Response.json({ items: [] });
      throw new Error(`Unexpected request: ${path}`);
    },
  });
  const text = '  Exact admitted task\r\n请求 🧭\n';
  const readAgentOriginalRequest = vi.fn(async () => ({
    kind: 'ready' as const,
    agentId: 'agt_1',
    text,
    source: 'admitted-first-task' as const,
    turnSequence: null,
  }));
  const createAgentLifecycle = vi.fn();
  const runAgent = vi.fn();
  const client = agentClient({
    resourceViews,
    readAgentOriginalRequest,
    createAgentLifecycle,
    runAgent,
  });
  const onNavigate = vi.fn();
  function NavigableConsole() {
    const [route, setRoute] = useState(props.route);
    return (
      <CmaClientProvider client={client}>
        <Console
          {...props}
          route={route}
          authEpoch={7}
          onNavigate={(next) => {
            onNavigate(next);
            setRoute(next);
          }}
        />
      </CmaClientProvider>
    );
  }
  render(<NavigableConsole />);
  const start = await screen.findByRole('button', { name: 'Start a new agent with this request' });
  const actor = userEvent.setup();
  await expandNavigationFilters(actor);
  await chooseOption(actor, 'Navigation scope', 'Research (Organization)');
  expect(screen.getByRole('button', { name: 'Navigation scope' })).toHaveTextContent('Research');
  await actor.click(start);
  await waitFor(() => expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'home' }));
  expect(screen.getByRole('button', { name: 'Navigation scope' })).toHaveTextContent('Personal');
  expect(openTaskDraftStore('owner').read().draft.text).toBe(text);
  expect(readAgentOriginalRequest).toHaveBeenCalledOnce();
  expect(createAgentLifecycle).not.toHaveBeenCalled();
  expect(runAgent).not.toHaveBeenCalled();
});
