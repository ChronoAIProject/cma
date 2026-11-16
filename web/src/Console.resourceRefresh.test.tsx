import { ApiError } from '@chronoai/cma-sdk';
import { connectionClient } from './connections/client';
import { useEffect } from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE, agentClient } from './test/agent';
import { sandboxFixture, workspaceFixture } from './test/hierarchy';
import type { AgentEvent, ResourcePage, SandboxStatus } from './api/types';

const desktop = vi.hoisted(() => ({ mount: vi.fn(), unmount: vi.fn() }));
vi.mock('./agent/DesktopPanel', () => ({
  DesktopPanel: ({ runtimeReady }: { runtimeReady: boolean }) => {
    useEffect(() => {
      desktop.mount();
      return desktop.unmount;
    }, []);
    return <div data-testid="desktop">{runtimeReady ? 'Desktop ready' : 'Desktop waiting'}</div>;
  },
}));

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

it.each([false, true])(
  'retains the current desktop and history through unrelated Sandbox acceptance/completion (pending history: %s)',
  async (pendingHistory) => {
    vi.useFakeTimers();
    const originalMedia = window.matchMedia;
    vi.spyOn(window, 'matchMedia').mockImplementation((query) => ({
      ...originalMedia(query),
      matches: query.includes('min-width'),
    }));
    let status: SandboxStatus = 'active';
    const other = () => ({
      ...sandboxFixture(status),
      id: 'sbx_2',
      name: 'Borealis',
      workspaces: [],
    });
    let finish!: (value: ResourcePage<AgentEvent, number>) => void;
    const page = new Promise<ResourcePage<AgentEvent, number>>((resolve) => {
      finish = resolve;
    });
    const readAgentEvents = vi.fn().mockResolvedValue({ items: [] });
    if (pendingHistory)
      readAgentEvents.mockResolvedValueOnce({ items: [], next: 4 }).mockReturnValueOnce(page);
    const readAgent = vi.fn(async () => ({ ...AGENT_DETAIL, desktop: true }));
    const readAgentTranscript = vi.fn(async () => AGENT_TRANSCRIPT);
    const readAgentUsage = vi.fn(async () => AGENT_USAGE);
    const pauseSandbox = vi.fn(async () => {
      status = 'pausing';
    });
    const client = agentClient({
      listSandboxes: async () => ({ items: [sandboxFixture(), other()] }),
      readSandbox: async (id) => (id === 'sbx_2' ? other() : sandboxFixture()),
      readAgent,
      readAgentTranscript,
      readAgentUsage,
      readAgentEvents,
      pauseSandbox,
    });
    const replay = vi.spyOn(client, 'replayAgent');
    const view = render(
      <CmaClientProvider client={client}>
        <Console
          user={{
            id: 'owner',
            handle: 'Owner',
            visitor: false,
            capabilities: ['use_own_sandboxes'],
          }}
          route={{ kind: 'agent', agentId: 'agt_1' }}
          onNavigate={vi.fn()}
          onSignOut={vi.fn()}
        />
      </CmaClientProvider>,
    );
    await act(() => vi.advanceTimersByTimeAsync(0));
    const panel = screen.getByTestId('desktop');
    expect(panel).toHaveTextContent('Desktop ready');
    const message = screen.getByRole('textbox', { name: 'Message' });
    fireEvent.change(message, { target: { value: 'Retain this draft' } });
    const messages = [...document.querySelectorAll('article.message')];
    const signals = readAgentEvents.mock.calls.map((call) => call[1]?.signal as AbortSignal);
    const initialEvents = readAgentEvents.mock.calls.length;
    const initialReplay = replay.mock.calls.length;
    fireEvent.click(screen.getByRole('button', { name: 'Filter' }));
    fireEvent.click(screen.getByRole('button', { name: 'Manage environments' }));
    fireEvent.click(screen.getByRole('button', { name: 'Options for Borealis' }));
    await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Pause' })));
    expect(pauseSandbox).toHaveBeenCalledOnce();
    expect(screen.getByText('Pause request accepted.')).toBeVisible();
    status = 'paused';
    await act(() => vi.advanceTimersByTimeAsync(250));
    expect(screen.getByText('Sandbox paused.')).toBeVisible();
    expect(readAgent).toHaveBeenCalledOnce();
    expect(readAgentTranscript).toHaveBeenCalledOnce();
    expect(readAgentUsage).toHaveBeenCalledOnce();
    expect(readAgentEvents).toHaveBeenCalledTimes(initialEvents);
    expect(replay).toHaveBeenCalledTimes(initialReplay);
    expect(signals.every((signal) => !signal.aborted)).toBe(true);
    expect(screen.getByTestId('desktop')).toBe(panel);
    expect(desktop.mount).toHaveBeenCalledOnce();
    expect(desktop.unmount).not.toHaveBeenCalled();
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(message);
    expect(message).toHaveValue('Retain this draft');
    expect([...document.querySelectorAll('article.message')]).toEqual(messages);
    if (pendingHistory) await act(async () => finish({ items: [] }));
    view.unmount();
  },
);

it('keeps the desktop mounted while its own Sandbox pauses and resumes, including pending history', async () => {
  vi.useFakeTimers();
  const originalMedia = window.matchMedia;
  vi.spyOn(window, 'matchMedia').mockImplementation((query) => ({
    ...originalMedia(query),
    matches: query.includes('min-width'),
  }));
  let status: SandboxStatus = 'active';
  let finish!: (value: ResourcePage<AgentEvent, number>) => void;
  const remaining = new Promise<ResourcePage<AgentEvent, number>>((resolve) => {
    finish = resolve;
  });
  const readAgentEvents = vi
    .fn()
    .mockResolvedValueOnce({ items: [], next: 4 })
    .mockReturnValueOnce(remaining);
  const readAgent = vi.fn(async () => ({
    ...AGENT_DETAIL,
    desktop: true,
    sandbox: { ...AGENT_DETAIL.sandbox, status },
  }));
  const readAgentTranscript = vi.fn(async () => AGENT_TRANSCRIPT);
  const client = agentClient({
    listSandboxes: async () => ({ items: [sandboxFixture(status)] }),
    readSandbox: async () => sandboxFixture(status),
    readAgent,
    readAgentEvents,
    readAgentTranscript,
    pauseSandbox: async () => {
      status = 'pausing';
    },
    reviveSandbox: async () => {
      status = 'reviving';
    },
  });
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: AGENT_DETAIL.updatedAt,
    services: [],
  });
  const view = render(
    <CmaClientProvider client={client}>
      <Console
        user={{ id: 'owner', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  const panel = screen.getByTestId('desktop');
  const message = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(message, { target: { value: 'Keep my draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Filter' }));
  fireEvent.click(screen.getByRole('button', { name: 'Manage environments' }));
  fireEvent.click(screen.getByRole('button', { name: 'Options for Atlas' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Pause' })));
  expect(readAgent).toHaveBeenCalledTimes(2);
  expect(panel).toHaveTextContent('Desktop waiting');
  status = 'paused';
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(readAgent).toHaveBeenCalledTimes(3);
  expect(screen.getByText('Sandbox paused.')).toBeVisible();
  // Finishing the old history request must not restore its opening Active metadata.
  await act(async () => finish({ items: [] }));
  expect(panel).toHaveTextContent('Desktop waiting');
  fireEvent.click(screen.getByRole('button', { name: 'Options for Atlas' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Resume' })));
  expect(readAgent).toHaveBeenCalledTimes(4);
  status = 'active';
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(readAgent).toHaveBeenCalledTimes(5);
  expect(panel).toHaveTextContent('Desktop ready');
  expect(screen.getByTestId('desktop')).toBe(panel);
  expect(desktop.mount).toHaveBeenCalledOnce();
  expect(desktop.unmount).not.toHaveBeenCalled();
  expect(readAgentTranscript).toHaveBeenCalledOnce();
  expect(readAgentEvents.mock.calls.map((call) => call[1].cursor)).toEqual([0, 4]);
  expect(readAgentEvents.mock.calls[1]![1].signal.aborted).toBe(false);
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(message);
  expect(message).toHaveValue('Keep my draft');
  view.unmount();
});

it('leaves the existing unavailable state after deleting the current Workspace', async () => {
  vi.useFakeTimers();
  vi.spyOn(console, 'warn').mockImplementation(() => undefined);
  let removed = false;
  const removeWorkspace = vi.fn(async () => {
    removed = true;
  });
  const readAgent = vi.fn(async () => {
    if (removed) throw new ApiError(404, 'Agent was deleted with its Workspace');
    return { ...AGENT_DETAIL, status: 'stopped' as const };
  });
  const readWorkspace = vi.fn(async () => ({
    ...workspaceFixture('stopped'),
    status: removed ? ('removed' as const) : ('ready' as const),
  }));
  const client = agentClient({ readAgent, readWorkspace, removeWorkspace });
  const view = render(
    <CmaClientProvider client={client}>
      <Console
        user={{ id: 'owner', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  fireEvent.click(screen.getByRole('button', { name: 'Filter' }));
  fireEvent.click(screen.getByRole('button', { name: 'Manage environments' }));
  fireEvent.click(screen.getByRole('button', { name: /Options for.*chrono-bot/ }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Remove' })));
  fireEvent.click(screen.getByRole('checkbox'));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Delete Workspace' })));
  expect(removeWorkspace).toHaveBeenCalledOnce();
  expect(screen.getByRole('heading', { name: 'Agent unavailable' })).toBeVisible();
  expect(screen.getByText(/Agent was deleted with its Workspace/)).toBeVisible();
  view.unmount();
});
