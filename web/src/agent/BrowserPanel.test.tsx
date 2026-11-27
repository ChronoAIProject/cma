import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient, type AgentEnvironment } from '@cma/client';
import { CmaClientProvider } from '../api/context';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { agentClient } from '../test/agent';
import { AgentSidePanel } from './AgentSidePanel';
import { AgentComputerObservation } from './environment/observation';
import { taskExperience } from '../test/taskExperience';

afterEach(() => {
  vi.useRealTimers();
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
});

it('releases hidden and closed observation, reconnects without enablement, and withdraws revoked frames', async () => {
  vi.useFakeTimers();
  let revoked = false;
  const fetch = vi.fn(async (input: RequestInfo | URL, options?: RequestInit) => {
    if (String(input).endsWith('/environment')) return Response.json(environment('available'));
    if (options?.method === 'DELETE')
      return Response.json({ ...observation(1), state: 'ready', frame: null });
    if (revoked) return Response.json({ title: 'Forbidden', status: 403 }, { status: 403 });
    return Response.json(observation(2));
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const client = agentClient({
    taskExperience: taskExperience('owner'),
    files: {
      ...emptyFiles,
      environment: (agent, options) => sdk.agentFiles.environment(agent, options),
    },
    browser: sdk.agentBrowser,
  });
  const scope = new ArtifactPublicationScope();
  const view = (open: boolean) => (
    <CmaClientProvider client={client}>
      <AgentSidePanel
        viewerId="owner"
        agentId="agent-a"
        shared={false}
        panel={open ? 'browser' : undefined}
        onClose={() => {}}
        onAccessChanged={() => {}}
        publicationScope={scope}
      />
    </CmaClientProvider>
  );
  const { rerender } = render(view(true));
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(screen.getByAltText('Agent browser page')).toBeInTheDocument();
  expect(screen.getByLabelText('Browser location')).toHaveTextContent('Local page');
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' });
  fireEvent(document, new Event('visibilitychange'));
  expect(screen.queryByAltText('Agent browser page')).not.toBeInTheDocument();
  expect(
    screen.getByText('Live view pauses while this tab is in the background.'),
  ).toBeInTheDocument();
  expect(screen.queryByText(/Agent paused/)).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(0));
  const count = fetch.mock.calls.length;
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(fetch).toHaveBeenCalledTimes(count);
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
  fireEvent(document, new Event('visibilitychange'));
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(screen.getByAltText('Agent browser page')).toBeInTheDocument();
  revoked = true;
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(screen.queryByAltText('Agent browser page')).not.toBeInTheDocument();
  expect(screen.getByText('Browser access is not permitted.')).toBeInTheDocument();
  rerender(view(false));
  await act(() => vi.advanceTimersByTimeAsync(0));
  const closed = fetch.mock.calls.length;
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(fetch).toHaveBeenCalledTimes(closed);
  expect(fetch.mock.calls.some(([, options]) => options?.method === 'PUT')).toBe(false);
  expect(fetch.mock.calls.at(-1)?.[1]?.method).toBe('DELETE');
});

it('keeps the live Browser observer across pending environment reads and clears it on policy change', async () => {
  vi.useFakeTimers();
  let environmentReads = 0;
  let sequence = 1;
  let policyRevision = 1;
  const accessDeadline = new Date(Date.now() + 20_000).toISOString();
  const fetch = vi.fn(async (input: RequestInfo | URL, options?: RequestInit) => {
    if (String(input).endsWith('/environment')) {
      environmentReads += 1;
      const current = environment('available');
      return Response.json({
        ...current,
        policy_revision: policyRevision,
        access_revalidate_at: accessDeadline,
        revalidate_at: new Date(Date.now() + 2000).toISOString(),
        browser:
          environmentReads === 1
            ? current.browser
            : { ...feature('temporarily_unavailable'), reason: 'observation_pending' },
      });
    }
    return Response.json(observation(options?.method === 'DELETE' ? sequence : sequence++));
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const client = agentClient({
    taskExperience: taskExperience('owner'),
    files: {
      ...emptyFiles,
      environment: (agent, options) => sdk.agentFiles.environment(agent, options),
    },
    browser: sdk.agentBrowser,
  });
  render(
    <CmaClientProvider client={client}>
      <AgentSidePanel
        viewerId="owner"
        agentId="agent-a"
        shared={false}
        panel="browser"
        onClose={() => {}}
        onAccessChanged={() => {}}
        publicationScope={new ArtifactPublicationScope()}
      />
    </CmaClientProvider>,
  );
  await act(() => vi.advanceTimersByTimeAsync(1));
  fireEvent.load(screen.getByAltText('Agent browser page'));
  expect(screen.getByRole('figure', { name: 'Live browser page' })).toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(4000));
  expect(environmentReads).toBe(3);
  fireEvent.load(screen.getByAltText('Agent browser page'));
  expect(screen.getByRole('figure', { name: 'Live browser page' })).toBeInTheDocument();
  const observers = fetch.mock.calls.flatMap(([input]) => {
    const observer = new URL(String(input)).searchParams.get('observer');
    return observer ? [observer] : [];
  });
  expect(new Set(observers).size).toBe(1);
  expect(fetch.mock.calls.some(([, options]) => options?.method === 'DELETE')).toBe(false);
  policyRevision += 1;
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(screen.queryByAltText('Agent browser page')).not.toBeInTheDocument();
  expect(fetch.mock.calls.some(([, options]) => options?.method === 'DELETE')).toBe(true);
});

it('shows ordered frames, rejects delayed A, loses Live on disconnect, and resumes the same context', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-03T00:00:00Z'));
  let sequence = 1;
  let disconnected = false;
  const fetch = vi.fn(async (input: RequestInfo | URL, options?: RequestInit) => {
    const path = String(input);
    if (path.endsWith('/environment'))
      return Response.json({
        agent_id: 'agent-a',
        policy_revision: 1,
        runtime_incarnation: 'incarnation',
        revalidate_at: null,
        files: feature('available'),
        operation_output: feature('available'),
        browser: feature('available'),
        replay: feature('unsupported'),
        control: feature('unsupported'),
      });
    if (options?.method === 'DELETE') return Response.json(observation(sequence));
    if (disconnected) throw new TypeError('offline');
    return Response.json({
      ...observation(sequence >= 3 ? sequence++ : sequence),
      address: sequence === 1 ? 'http://localhost:5173' : 'https://example.com',
    });
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const client = agentClient({
    taskExperience: taskExperience('owner'),
    files: {
      ...emptyFiles,
      environment: (agent, options) => sdk.agentFiles.environment(agent, options),
    },
    browser: sdk.agentBrowser,
  });
  const scope = new ArtifactPublicationScope();
  const { rerender } = render(
    <CmaClientProvider client={client}>
      <AgentSidePanel
        viewerId="owner"
        agentId="agent-a"
        shared={false}
        panel="browser"
        onClose={() => {}}
        onAccessChanged={() => {}}
        publicationScope={scope}
      />
    </CmaClientProvider>,
  );
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1);
  });
  const first = screen.getByAltText('Agent browser page');
  const initial = first.getAttribute('src');
  expect(screen.getByLabelText('Browser location')).toHaveTextContent('localhost:5173');
  fireEvent.load(first);
  expect(screen.getByRole('figure', { name: 'Live browser page' })).toBeInTheDocument();
  sequence = 2;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1000);
  });
  fireEvent.load(screen.getByAltText('Agent browser page'));
  const second = screen.getByAltText('Agent browser page').getAttribute('src');
  expect(screen.getByLabelText('Browser location')).toHaveTextContent('example.com');
  expect(second).not.toBe(initial);
  sequence = 1;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1000);
  });
  expect(screen.getByAltText('Agent browser page')).toHaveAttribute('src', second);
  disconnected = true;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(4000);
  });
  expect(screen.queryByRole('figure', { name: 'Live browser page' })).not.toBeInTheDocument();
  expect(screen.getByText('Last frame · Reconnecting…')).toBeInTheDocument();
  disconnected = false;
  sequence = 3;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(9000);
  });
  fireEvent.load(screen.getByAltText('Agent browser page'));
  expect(screen.getByRole('figure', { name: 'Live browser page' })).toBeInTheDocument();
  expect(fetch.mock.calls.some(([input]) => String(input).includes('context_id=context'))).toBe(
    true,
  );
  rerender(
    <CmaClientProvider client={client}>
      <AgentSidePanel
        viewerId="owner"
        agentId="agent-b"
        shared={false}
        panel="browser"
        onClose={() => {}}
        onAccessChanged={() => {}}
        publicationScope={scope}
      />
    </CmaClientProvider>,
  );
  expect(screen.queryByAltText('Agent browser page')).not.toBeInTheDocument();
});

it('enables only after owner action, exposes failure, and preserves the explicitly selected Files tab', async () => {
  vi.useFakeTimers();
  let enabled = false;
  let fail = true;
  const fetch = vi.fn(async (input: RequestInfo | URL, options?: RequestInit) => {
    if (String(input).endsWith('/environment'))
      return Response.json(environment(enabled ? 'available' : 'not_enabled'));
    if (options?.method === 'PUT') {
      expect(JSON.parse(String(options.body))).toEqual({
        runtime_incarnation: 'incarnation',
        enabled: true,
      });
      if (fail) return Response.json({ title: 'Unavailable', status: 503 }, { status: 503 });
      enabled = true;
      return Response.json({ ...observation(1), state: 'loading', frame: null });
    }
    return Response.json(observation(2));
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const client = agentClient({
    taskExperience: taskExperience('owner'),
    files: {
      ...emptyFiles,
      environment: (agent, options) => sdk.agentFiles.environment(agent, options),
    },
    browser: sdk.agentBrowser,
  });
  const scope = new ArtifactPublicationScope();
  const onSelect = vi.fn();
  const view = (panel: 'browser' | 'files') => (
    <CmaClientProvider client={client}>
      <AgentSidePanel
        viewerId="owner"
        agentId="agent-a"
        shared={false}
        panel={panel}
        onSelect={onSelect}
        onClose={() => {}}
        onAccessChanged={() => {}}
        publicationScope={scope}
      />
    </CmaClientProvider>
  );
  const first = render(view('browser'));
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(fetch.mock.calls.every(([input]) => String(input).endsWith('/environment'))).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Enable browser' }));
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(screen.getByRole('alert')).toHaveTextContent('Browser could not be enabled. Try again.');
  expect(screen.queryByAltText('Agent browser page')).not.toBeInTheDocument();
  fail = false;
  fireEvent.click(screen.getByRole('button', { name: 'Enable browser' }));
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(screen.getByAltText('Agent browser page')).toBeInTheDocument();
  first.unmount();
  render(view('files'));
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(onSelect).not.toHaveBeenCalled();
  expect(screen.getByRole('tab', { name: 'Files' })).toHaveAttribute('aria-selected', 'true');
  expect(screen.queryByRole('tab', { name: 'Operations' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('tab', { name: 'Screen' }));
  fireEvent(window, new Event('focus'));
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(onSelect.mock.calls).toEqual([['browser']]);
  expect(fetch.mock.calls.filter(([, options]) => options?.method === 'PUT')).toHaveLength(2);
});

it('keeps an explicit Operations jump when an enabled browser becomes available', async () => {
  vi.useFakeTimers();
  const client = agentClient({
    taskExperience: taskExperience('owner'),
    files: { ...emptyFiles, environment: async () => environment('available') },
  });
  const selected = new AgentComputerObservation({ client, subject: 'owner', agentId: 'agent-a' });
  selected.publish([
    {
      id: 'command-1',
      title: 'printf output',
      kind: 'execute',
      status: 'completed',
      output: { text: 'output', truncated: false },
    },
  ]);
  selected.selectTerminal('command-1');
  const onSelect = vi.fn();
  const view = (panel: 'activity' | 'files' | undefined) => (
    <CmaClientProvider client={client}>
      <AgentSidePanel
        viewerId="owner"
        agentId="agent-a"
        shared={false}
        panel={panel}
        onSelect={onSelect}
        observation={selected}
        onClose={() => {}}
        onAccessChanged={() => {}}
        publicationScope={new ArtifactPublicationScope()}
      />
    </CmaClientProvider>
  );
  const { rerender } = render(view('activity'));
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(onSelect).not.toHaveBeenCalledWith('browser', 'activity');
  await act(() => vi.advanceTimersByTimeAsync(1));
  rerender(view(undefined));
  await act(() => vi.advanceTimersByTimeAsync(1));
  rerender(view('files'));
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(onSelect).not.toHaveBeenCalledWith('browser', 'activity');
  expect(onSelect).not.toHaveBeenCalled();
  expect(selected.terminalSnapshot()?.id).toBe('command-1');
});

const feature = (state: AgentEnvironment['browser']['state']): AgentEnvironment['browser'] => ({
  state,
  reason: 'proof',
  scope: 'agent_browser',
  writable: false,
});
const environment = (state: AgentEnvironment['browser']['state']): AgentEnvironment => ({
  agent_id: 'agent-a',
  policy_revision: 1,
  runtime_incarnation: 'incarnation',
  revalidate_at: null,
  files: feature('available'),
  operation_output: feature('available'),
  browser: feature(state),
  replay: feature('unsupported'),
  control: feature('unsupported'),
});
const emptyFiles = {
  list: async () => [],
  read: async () => {
    throw new Error('unused');
  },
  download: async () => new Blob(),
  write: async () => ({ path: '' }),
  remove: async () => {},
};
function observation(sequence: number) {
  return {
    agent_id: 'agent-a',
    runtime_incarnation: 'incarnation',
    context_id: 'context',
    state: 'live',
    location: 'Local page',
    observers: 1,
    captures: sequence,
    bytes: 3,
    min_interval_ms: 1000,
    fresh_for_ms: 3000,
    max_frame_bytes: 262144,
    max_observers: 16,
    frame: {
      sequence,
      captured_at: new Date(Date.now() - (sequence === 1 ? 500 : 0)).toISOString(),
      media_type: 'image/jpeg',
      width: 1280,
      height: 720,
      data: sequence === 1 ? '/9j/AA==' : sequence === 2 ? '/9j/BB==' : '/9j/CC==',
    },
  };
}

it('returns to Screen when a selected Operations tab is unavailable for the next task', async () => {
  const client = agentClient();
  const selected = new AgentComputerObservation({ client, subject: 'owner', agentId: 'agent-a' });
  selected.publish([]);
  const onSelect = vi.fn();
  render(
    <CmaClientProvider client={client}>
      <AgentSidePanel
        viewerId="owner"
        agentId="agent-a"
        shared={false}
        panel="activity"
        observation={selected}
        onSelect={onSelect}
        onClose={vi.fn()}
        onAccessChanged={vi.fn()}
        publicationScope={new ArtifactPublicationScope()}
      />
    </CmaClientProvider>,
  );
  await act(async () => {});
  expect(screen.queryByRole('tab', { name: 'Operations' })).not.toBeInTheDocument();
  expect(onSelect).toHaveBeenCalledExactlyOnceWith('browser', 'activity');
});
