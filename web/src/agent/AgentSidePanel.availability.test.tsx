import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { AgentSidePanel, type AgentPanel } from './AgentSidePanel';
import { CmaClientProvider } from '../api/context';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import type { AgentEnvironment } from '@cma/client';
import { AgentComputerObservation } from './environment/observation';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const unavailable = {
  state: 'unsupported' as const,
  writable: false,
  reason: 'unsupported',
  scope: 'agent',
};
const environment: AgentEnvironment = {
  agent_id: 'agt_1',
  policy_revision: 1,
  revalidate_at: null,
  runtime_incarnation: null,
  replay: unavailable,
  files: unavailable,
  browser: unavailable,
  control: unavailable,
  operation_output: unavailable,
};
const available = { ...unavailable, state: 'available' as const, writable: true };
function files(read: () => Promise<AgentEnvironment>) {
  return {
    environment: vi.fn(read),
    list: vi.fn(async () => []),
    read: vi.fn(),
    download: vi.fn(),
    write: vi.fn(),
    remove: vi.fn(),
  };
}
function panel(client: ReturnType<typeof agentClient>, tab: AgentPanel = 'files', shared = false) {
  const observation = new AgentComputerObservation({
    client,
    subject: 'owner-a',
    agentId: 'agt_1',
  });
  observation.publish([
    {
      id: 'command',
      runId: '1',
      title: 'printf capability-proof',
      kind: 'execute',
      status: 'completed',
      output: { text: 'capability-proof', truncated: false },
    },
  ]);
  return (
    <CmaClientProvider client={client}>
      <AgentSidePanel
        agentId="agt_1"
        shared={shared}
        viewerId="owner-a"
        observation={observation}
        panel={tab}
        detail={{ ...AGENT_DETAIL, desktop: false }}
        onClose={vi.fn()}
        onAccessChanged={vi.fn()}
        publicationScope={new ArtifactPublicationScope()}
      />
    </CmaClientProvider>
  );
}
const currentTab = () => within(screen.getByRole('tabpanel'));

it.each(['files', 'activity'] as const)(
  'shows pending environment checks in %s without file IO',
  async (tab) => {
    const fileClient = files(() => new Promise(() => {}));
    render(panel(agentClient({ files: fileClient }), tab));
    await act(async () => {});
    expect(currentTab().getByText('Checking environment capabilities…')).toBeVisible();
    expect(
      currentTab().queryByText('File access is unavailable for this Agent.'),
    ).not.toBeInTheDocument();
    expect(fileClient.list).not.toHaveBeenCalled();
    expect(fileClient.read).not.toHaveBeenCalled();
    expect(fileClient.write).not.toHaveBeenCalled();
  },
);

it('offers a safe environment retry in owner Files and coalesces pending focus events', async () => {
  let resolve!: (value: AgentEnvironment) => void;
  const fileClient = files(
    vi
      .fn()
      .mockRejectedValueOnce(new ApiError(503, '<private upstream error>'))
      .mockImplementation(
        () =>
          new Promise<AgentEnvironment>((done) => {
            resolve = done;
          }),
      ),
  );
  render(panel(agentClient({ files: fileClient })));
  await act(async () => {});
  expect(
    currentTab().getByText('Environment capabilities are temporarily unavailable. Try again.'),
  ).toBeVisible();
  expect(screen.queryByText('<private upstream error>')).not.toBeInTheDocument();
  expect(fileClient.list).not.toHaveBeenCalled();
  const retry = currentTab().getByRole('button', { name: 'Check environment again' });
  fireEvent.click(retry);
  fireEvent.click(retry);
  fireEvent.focus(window);
  fireEvent.focus(window);
  await act(async () => {});
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(currentTab().getByText('Checking environment capabilities…')).toBeVisible();
  await act(async () => resolve({ ...environment, files: available }));
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  expect(
    currentTab().queryByRole('button', { name: 'Check environment again' }),
  ).not.toBeInTheDocument();
  expect(fileClient.write).not.toHaveBeenCalled();
});

it.each(['not_enabled', 'unsupported', 'forbidden', 'temporarily_unavailable'] as const)(
  'shows confirmed feature state %s without a contradictory file denial',
  async (state) => {
    const feature = { ...unavailable, state };
    const fileClient = files(async () => ({
      ...environment,
      files: feature,
      operation_output: feature,
    }));
    const client = agentClient({ files: fileClient });
    const view = render(panel(client));
    await act(async () => {});
    const description = {
      not_enabled: 'is not enabled for this environment.',
      unsupported: 'is not supported by this environment.',
      forbidden: 'access is not permitted.',
      temporarily_unavailable: 'is temporarily unavailable. Try again.',
    }[state];
    expect(currentTab().getByText(`Files ${description}`)).toBeVisible();
    expect(
      currentTab().queryByText('File access is unavailable for this Agent.'),
    ).not.toBeInTheDocument();
    view.rerender(panel(client, 'activity'));
    expect(currentTab().getByText(`Operation output ${description}`)).toBeVisible();
    expect(fileClient.list).not.toHaveBeenCalled();
  },
);

it('checks shared file authority before reporting environment capability state', async () => {
  const fileClient = files(async () => environment);
  render(panel(agentClient({ files: fileClient }), 'files', true));
  await act(async () => {});
  expect(currentTab().getByText('Checking file access…')).toBeVisible();
  expect(
    currentTab().queryByText('Files is not supported by this environment.'),
  ).not.toBeInTheDocument();
  expect(fileClient.list).not.toHaveBeenCalled();
});
it.each(['loading', 'failed', 'missing-detail', 'unsupported'] as const)(
  'distinguishes unknown desktop/environment from a confirmed unsupported browser: %s',
  async (mode) => {
    const read = vi.fn(async () => {
      if (mode === 'loading') return new Promise<AgentEnvironment>(() => {});
      if (mode === 'failed') throw new ApiError(429, 'Busy', undefined, undefined, 20_000);
      return environment;
    });
    const client = agentClient({ files: { environment: read } as never });
    const view = render(
      <CmaClientProvider client={client}>
        <AgentSidePanel
          agentId="agt_1"
          shared={false}
          panel="browser"
          detail={mode === 'missing-detail' ? undefined : { ...AGENT_DETAIL, desktop: false }}
          onClose={vi.fn()}
          onAccessChanged={vi.fn()}
          publicationScope={new ArtifactPublicationScope()}
        />
      </CmaClientProvider>,
    );
    await act(async () => {});
    if (mode === 'unsupported')
      expect(screen.getByText('Browser is not supported by this environment.')).toBeVisible();
    else {
      expect(
        screen.queryByText('Browser is not supported by this environment.'),
      ).not.toBeInTheDocument();
      expect(
        currentTab().getByText(
          mode === 'loading'
            ? 'Checking environment capabilities…'
            : mode === 'missing-detail'
              ? 'Checking Agent desktop availability…'
              : 'Environment capabilities are temporarily unavailable. Try again.',
        ),
      ).toBeVisible();
    }
    view.unmount();
  },
);

it('recovers all views on the existing visible 30-second refresh without enabling the browser', async () => {
  vi.useFakeTimers();
  const configure = vi.fn();
  const changeControl = vi.fn();
  const fileClient = files(
    vi
      .fn()
      .mockRejectedValueOnce(new ApiError(503, 'Unavailable'))
      .mockResolvedValue({
        ...environment,
        runtime_incarnation: 'boot-one',
        files: available,
        operation_output: available,
        browser: { ...unavailable, state: 'not_enabled' },
      }),
  );
  const client = agentClient({
    files: fileClient,
    browser: { configure } as never,
    browserControl: { change: changeControl } as never,
    taskExperience: {
      read: async () => ({
        account_id: 'owner-a',
        capabilities: {
          task_home_admission: false,
          suggested_followup_admission: false,
          desktop_enable: true,
          browser_takeover: true,
          replay_presentation: false,
        },
      }),
    } as never,
  });
  const view = render(panel(client));
  await act(async () => {});
  for (const tab of ['files', 'activity', 'browser'] as const) {
    view.rerender(panel(client, tab));
    expect(
      currentTab().getByText('Environment capabilities are temporarily unavailable. Try again.'),
    ).toBeVisible();
  }
  await act(() => vi.advanceTimersByTimeAsync(29_999));
  expect(fileClient.environment).toHaveBeenCalledTimes(1);
  expect(fileClient.list).not.toHaveBeenCalled();
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(currentTab().getByText('Browser is not enabled for this environment.')).toBeVisible();
  expect(currentTab().getByRole('button', { name: 'Enable browser' })).toBeEnabled();
  expect(fileClient.list).not.toHaveBeenCalled();
  view.rerender(panel(client, 'activity'));
  expect(currentTab().getByText('capability-proof', { exact: true })).toBeVisible();
  view.rerender(panel(client));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  expect(
    screen.queryByText('Environment capabilities are temporarily unavailable. Try again.'),
  ).not.toBeInTheDocument();
  view.rerender(panel(client, 'details'));
  view.rerender(panel(client));
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  expect(configure).not.toHaveBeenCalled();
  expect(changeControl).not.toHaveBeenCalled();
  expect(fileClient.write).not.toHaveBeenCalled();
});

it('honors the environment 429 deadline through retry buttons, focus and the periodic refresh', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
  const fileClient = files(
    vi
      .fn()
      .mockRejectedValueOnce(new ApiError(429, 'Busy', undefined, undefined, 40_000))
      .mockResolvedValue({ ...environment, files: available }),
  );
  render(panel(agentClient({ files: fileClient })));
  await act(async () => {});
  for (let count = 0; count < 5; count++) {
    fireEvent.click(currentTab().getByRole('button', { name: 'Check environment again' }));
    fireEvent.focus(window);
  }
  await act(() => vi.advanceTimersByTimeAsync(40_499));
  expect(fileClient.environment).toHaveBeenCalledTimes(1);
  expect(fileClient.list).not.toHaveBeenCalled();
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  expect(
    currentTab().queryByRole('button', { name: 'Check environment again' }),
  ).not.toBeInTheDocument();
});

it('withdraws file IO on 403 and distinguishes denial from a transient read failure', async () => {
  const fileClient = files(
    vi
      .fn()
      .mockResolvedValueOnce({ ...environment, files: available })
      .mockRejectedValue(new ApiError(403, 'Private refusal detail')),
  );
  const client = agentClient({ files: fileClient });
  const view = render(panel(client));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  fireEvent.focus(window);
  await act(async () => {});
  expect(
    currentTab().getByText('Environment capabilities are unavailable. Try again.'),
  ).toBeVisible();
  expect(
    currentTab().queryByText('Environment capabilities are temporarily unavailable. Try again.'),
  ).not.toBeInTheDocument();
  expect(screen.queryByText('Private refusal detail')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'New file' })).not.toBeInTheDocument();
  view.rerender(panel(client, 'activity'));
  expect(currentTab().queryByText('capability-proof', { exact: true })).not.toBeInTheDocument();
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  expect(fileClient.read).not.toHaveBeenCalled();
  expect(fileClient.write).not.toHaveBeenCalled();
});

it('waits while hidden and ignores a timed-out old incarnation after recovery', async () => {
  vi.useFakeTimers();
  const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  let resolve!: (value: AgentEnvironment) => void;
  const fileClient = files(
    vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise<AgentEnvironment>((done) => {
            resolve = done;
          }),
      )
      .mockResolvedValue({ ...environment, runtime_incarnation: 'boot-new', files: available }),
  );
  render(panel(agentClient({ files: fileClient })));
  await act(() => vi.advanceTimersByTimeAsync(15_000));
  expect(
    currentTab().getByText('Environment capabilities are temporarily unavailable. Try again.'),
  ).toBeVisible();
  visibility.mockReturnValue('hidden');
  fireEvent(document, new Event('visibilitychange'));
  await act(() => vi.advanceTimersByTimeAsync(60_000));
  expect(fileClient.environment).toHaveBeenCalledTimes(1);
  expect(fileClient.list).not.toHaveBeenCalled();
  visibility.mockReturnValue('visible');
  fireEvent(document, new Event('visibilitychange'));
  fireEvent.focus(window);
  await act(async () => {});
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  await act(async () => resolve({ ...environment, runtime_incarnation: 'boot-old' }));
  expect(
    currentTab().queryByText('Files is not supported by this environment.'),
  ).not.toBeInTheDocument();
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  expect(fileClient.write).not.toHaveBeenCalled();
});
