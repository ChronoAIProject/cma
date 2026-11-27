import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient } from '@cma/client';
import { AgentSidePanel, type AgentPanel } from './AgentSidePanel';
import { CmaClientProvider } from '../api/context';
import type { ReadOptions } from '../api/client';
import type { AgentFilesClient } from '../api/agentFiles';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import type { AgentEnvironment } from '@cma/client';
import { AgentComputerObservation } from './environment/observation';
import { computerClient } from '../api/rest/computer';

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
function files(read: (agentId: string, options?: ReadOptions) => Promise<AgentEnvironment>) {
  return {
    environment: vi.fn(read),
    list: vi.fn(async () => []),
    read: vi.fn(),
    download: vi.fn(),
    write: vi.fn(),
    remove: vi.fn(),
  };
}
function panel(
  client: ReturnType<typeof agentClient>,
  tab: AgentPanel = 'files',
  shared = false,
  { viewerId = 'owner-a', agentId = 'agt_1', runtimeVersion = 'runtime-a' } = {},
) {
  const observation = new AgentComputerObservation({
    client,
    subject: viewerId,
    agentId,
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
        agentId={agentId}
        shared={shared}
        viewerId={viewerId}
        observation={observation}
        panel={tab}
        detail={{ ...AGENT_DETAIL, id: agentId, desktop: false }}
        resourceAvailability={{ state: 'live', runtimeVersion }}
        onClose={vi.fn()}
        onAccessChanged={vi.fn()}
        publicationScope={new ArtifactPublicationScope()}
      />
    </CmaClientProvider>
  );
}
const currentTab = () => within(screen.getByRole('tabpanel'));

it.each([
  ['files', 'Checking file access…'],
  ['activity', 'Checking operation availability…'],
  ['browser', 'Checking screen availability…'],
] as const)('gives %s one specific pending state', async (tab, message) => {
  const fileClient = files(() => new Promise(() => {}));
  render(panel(agentClient({ files: fileClient }), tab));
  await act(async () => {});
  expect(currentTab().getAllByRole('status')).toHaveLength(1);
  expect(currentTab().getByRole('status')).toHaveTextContent(message);
  expect(currentTab().queryByText(/private runtime state|Authorized Sandbox files/)).toBeNull();
  expect(fileClient.list).not.toHaveBeenCalled();
});

it('explains an unavailable live screen without claiming the Agent is paused or enabling it', async () => {
  const fileClient = files(async () => ({
    ...environment,
    browser: { ...unavailable, state: 'temporarily_unavailable', reason: 'browser_paused' },
  }));
  const browser = { read: vi.fn(), release: vi.fn(), configure: vi.fn() };
  render(panel(agentClient({ files: fileClient, browser }), 'browser'));
  await act(async () => {});
  expect(currentTab().getAllByRole('status')).toHaveLength(1);
  expect(currentTab().getByRole('status')).toHaveTextContent(
    'No live screen is available right now.',
  );
  expect(currentTab().getByText(/You can continue reading this conversation/)).toBeVisible();
  expect(currentTab().queryByText(/paused/i)).toBeNull();
  fireEvent.click(currentTab().getByRole('button', { name: 'Check again' }));
  await act(async () => {});
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(browser.configure).not.toHaveBeenCalled();
  expect(browser.read).not.toHaveBeenCalled();
});

it.each(['files', 'activity'] as const)(
  'shows pending environment checks in %s without file IO',
  async (tab) => {
    const fileClient = files(() => new Promise(() => {}));
    render(panel(agentClient({ files: fileClient }), tab));
    await act(async () => {});
    expect(
      currentTab().getByText(
        tab === 'files' ? 'Checking file access…' : 'Checking operation availability…',
      ),
    ).toBeVisible();
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
  expect(screen.queryByText('<private upstream error>')).not.toBeInTheDocument();
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  fireEvent.focus(window);
  fireEvent.focus(window);
  await act(async () => {});
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  await act(async () => resolve({ ...environment, files: available }));
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  expect(currentTab().queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();
  expect(fileClient.write).not.toHaveBeenCalled();
});

it('attempts the Files listing during a transient environment failure', async () => {
  const fileClient = files(vi.fn().mockRejectedValueOnce(new ApiError(503, 'Runtime unavailable')));
  fileClient.list.mockRejectedValueOnce(new Error('File listing unavailable'));
  render(panel(agentClient({ files: fileClient })));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  expect(currentTab().getByText('File listing unavailable')).toBeVisible();
  expect(currentTab().queryByText('Checking file access…')).not.toBeInTheDocument();
});

it('keeps an in-flight Files listing alive through environment revalidation loading', async () => {
  let resolveEnvironment!: (value: AgentEnvironment) => void;
  let listSignal: AbortSignal | undefined;
  const fileClient = files(
    vi
      .fn()
      .mockResolvedValueOnce({ ...environment, files: available })
      .mockImplementationOnce(
        () => new Promise<AgentEnvironment>((resolve) => (resolveEnvironment = resolve)),
      ),
  );
  const listing = vi.fn(
    (_agentId: string, _path: string, options?: ReadOptions) =>
      new Promise<never[]>(() => {
        listSignal = options?.signal;
      }),
  );
  fileClient.list = listing as typeof fileClient.list;
  render(panel(agentClient({ files: fileClient })));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  fireEvent.focus(window);
  await act(async () => {});
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(listSignal?.aborted).toBe(false);
  await act(async () => resolveEnvironment({ ...environment, files: available }));
  expect(listSignal?.aborted).toBe(false);
});

it('keeps Files read-only when a previously available environment becomes transient', async () => {
  const fileClient = files(
    vi
      .fn()
      .mockResolvedValueOnce({ ...environment, files: { ...available, writable: false } })
      .mockRejectedValueOnce(new ApiError(503, 'Runtime unavailable')),
  );
  render(panel(agentClient({ files: fileClient })));
  await act(async () => {});
  expect(currentTab().queryByRole('button', { name: 'New file' })).toBeNull();
  fireEvent.focus(window);
  await act(async () => {});
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(currentTab().queryByRole('button', { name: 'New file' })).toBeNull();
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
    const fileDescription = {
      not_enabled: 'File browsing is not available for this environment.',
      unsupported: 'This environment does not support file browsing.',
      forbidden: 'You do not have access to these files.',
      temporarily_unavailable: 'We couldn’t check file availability.',
    }[state];
    expect(currentTab().getByText(fileDescription)).toBeVisible();
    expect(currentTab().getAllByRole('status')).toHaveLength(1);
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
      expect(screen.getByText('Live screen is not supported by this environment.')).toBeVisible();
    else {
      expect(
        screen.queryByText('Live screen is not supported by this environment.'),
      ).not.toBeInTheDocument();
      expect(
        currentTab().getByText(
          mode === 'loading'
            ? 'Checking screen availability…'
            : mode === 'missing-detail'
              ? 'Checking Agent desktop availability…'
              : 'We couldn’t check screen availability.',
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
    const subject = tab === 'files' ? 'file' : tab === 'activity' ? 'operation' : 'screen';
    if (tab !== 'files')
      expect(currentTab().getByText(`We couldn’t check ${subject} availability.`)).toBeVisible();
  }
  await act(() => vi.advanceTimersByTimeAsync(29_999));
  expect(fileClient.environment).toHaveBeenCalledTimes(1);
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(currentTab().getByText('Live screen is not enabled')).toBeVisible();
  expect(currentTab().getByRole('button', { name: 'Enable browser' })).toBeEnabled();
  expect(fileClient.list).toHaveBeenCalledTimes(2);
  view.rerender(panel(client, 'activity'));
  expect(currentTab().getByText('capability-proof', { exact: true })).toBeVisible();
  view.rerender(panel(client));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(3);
  expect(screen.queryByText('We couldn’t check file availability.')).not.toBeInTheDocument();
  view.rerender(panel(client, 'details'));
  view.rerender(panel(client));
  expect(fileClient.list).toHaveBeenCalledTimes(3);
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
  for (let count = 0; count < 5; count++) fireEvent.focus(window);
  await act(() => vi.advanceTimersByTimeAsync(40_499));
  expect(fileClient.environment).toHaveBeenCalledTimes(1);
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  expect(currentTab().queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();
});

it('honors the owner retry header across first open and same-scope reopen', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  let recover!: (response: Response) => void;
  const fetch = vi
    .fn<(request: RequestInfo | URL, options?: RequestInit) => Promise<Response>>()
    .mockResolvedValueOnce(
      Response.json(
        {
          type: 'about:blank',
          title: 'Agent owner unavailable',
          status: 503,
          code: 'agent_owner_unavailable',
        },
        {
          status: 503,
          headers: { 'content-type': 'application/problem+json', 'retry-after': '1' },
        },
      ),
    )
    .mockImplementationOnce(() => new Promise<Response>((resolve) => (recover = resolve)));
  const rest = computerClient(createClient({ baseUrl: 'https://fixture.invalid', fetch }));
  const fileClient = files(() => rest.environment('agt_1'));
  const client = agentClient({ files: { ...fileClient, environment: rest.environment } });
  const view = render(panel(client));
  await act(async () => {});
  fireEvent.focus(window);
  fireEvent.focus(window);
  view.rerender(panel(client, 'details'));
  await act(() => vi.advanceTimersByTimeAsync(200));
  expect(fetch).toHaveBeenCalledTimes(1);
  view.rerender(panel(client));
  await act(() => vi.advanceTimersByTimeAsync(799));
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(fileClient.list).toHaveBeenCalledTimes(2);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(fetch).toHaveBeenCalledTimes(2);
  fireEvent.focus(window);
  fireEvent.focus(window);
  await act(async () => recover(Response.json({ ...environment, files: available })));

  expect(fetch).toHaveBeenCalledTimes(2);
  for (const [request, options] of fetch.mock.calls) {
    expect(String(request)).toBe('https://fixture.invalid/api/v1/agents/agt_1/environment');
    expect(options?.method).toBe('GET');
  }
  expect(fileClient.list).toHaveBeenCalledTimes(2);
});

it.each([{ viewerId: 'owner-b' }, { agentId: 'agt_2' }, { runtimeVersion: 'runtime-b' }])(
  'quarantines an old Environment retry after the scope changes to %o',
  async (scope) => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0);
    let recover!: (value: AgentEnvironment) => void;
    const fileClient = files(
      vi
        .fn()
        .mockRejectedValueOnce(
          new ApiError(503, 'Agent owner unavailable', 'agent_owner_unavailable', undefined, 1000),
        )
        .mockImplementationOnce(
          () => new Promise<AgentEnvironment>((resolve) => (recover = resolve)),
        )
        .mockRejectedValueOnce(new ApiError(403, 'Forbidden')),
    );
    const client = agentClient({ files: fileClient });
    const view = render(panel(client));
    await act(async () => {});
    await act(() => vi.advanceTimersByTimeAsync(1000));
    const request = fileClient.environment.mock.calls[1]?.[1];
    view.rerender(panel(client, 'files', false, scope));
    await act(async () => recover({ ...environment, files: available }));
    expect(request?.signal?.aborted).toBe(true);
    expect(fileClient.environment).toHaveBeenCalledTimes(3);
    expect(fileClient.environment).toHaveBeenLastCalledWith(
      scope.agentId ?? 'agt_1',
      expect.anything(),
    );
    expect(currentTab().getByText('File access is unavailable.')).toBeVisible();
    expect(fileClient.list).toHaveBeenCalled();
  },
);

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
  expect(currentTab().getByText('File access is unavailable.')).toBeVisible();
  expect(currentTab().queryByText('We couldn’t check file availability.')).not.toBeInTheDocument();
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
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  visibility.mockReturnValue('hidden');
  fireEvent(document, new Event('visibilitychange'));
  await act(() => vi.advanceTimersByTimeAsync(60_000));
  expect(fileClient.environment).toHaveBeenCalledTimes(1);
  expect(fileClient.list).toHaveBeenCalledTimes(1);
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
it('review 4221 preserves an in-flight listing on a direct transient refresh failure', async () => {
  const client = files(
    vi
      .fn()
      .mockResolvedValueOnce({ ...environment, files: available })
      .mockRejectedValueOnce(new ApiError(503, 'Runtime unavailable')),
  );
  client.list.mockImplementationOnce(() => new Promise(() => {}));
  render(panel(agentClient({ files: client })));
  await act(async () => {});
  expect(client.list).toHaveBeenCalledTimes(1);
  const signal = (client.list.mock.calls[0] as unknown as [string, string, ReadOptions])[2].signal;
  expect(signal?.aborted).toBe(false);
  fireEvent.focus(window);
  await act(async () => {});
  expect(client.environment).toHaveBeenCalledTimes(2);
  expect(client.list).toHaveBeenCalledTimes(1);
  expect(signal?.aborted).toBe(false);
});

it('review 4221 preserves read-only file controls when a refresh fails', async () => {
  const client = files(
    vi
      .fn()
      .mockResolvedValueOnce({
        ...environment,
        files: { ...available, writable: false },
      })
      .mockRejectedValueOnce(new ApiError(503, 'Runtime unavailable')),
  );
  render(panel(agentClient({ files: client })));
  await act(async () => {});
  expect(currentTab().queryByRole('button', { name: 'New file' })).toBeNull();
  fireEvent.focus(window);
  await act(async () => {});
  expect(client.environment).toHaveBeenCalledTimes(2);
  expect(currentTab().queryByRole('button', { name: 'New file' })).toBeNull();
});

it('review 4221 preserves the listing across a same-scope refresh that enters loading before 503', async () => {
  vi.useFakeTimers();
  let reject!: (error: Error) => void;
  const client = files(
    vi
      .fn()
      .mockResolvedValueOnce({
        ...environment,
        revalidate_at: new Date(Date.now() + 1000).toISOString(),
        files: available,
      })
      .mockImplementationOnce(
        () =>
          new Promise((_, fail) => {
            reject = fail;
          }),
      ),
  );
  client.list.mockImplementationOnce(() => new Promise(() => {}));
  render(panel(agentClient({ files: client })));
  await act(async () => {});
  expect(client.list).toHaveBeenCalledTimes(1);
  const signal = (client.list.mock.calls[0] as unknown as [string, string, ReadOptions])[2].signal;
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(client.environment).toHaveBeenCalledTimes(2);
  await act(async () => reject(new ApiError(503, 'Runtime unavailable')));
  expect(signal?.aborted).toBe(false);
  expect(client.list).toHaveBeenCalledTimes(1);
});

it.each([
  ['HTTP 403', new ApiError(403, 'Access denied')],
  [
    'confirmed forbidden feature',
    { ...environment, files: { ...unavailable, state: 'forbidden' as const } },
  ],
])('review 4221 cancels an in-flight listing on %s', async (_label, denied) => {
  const read = vi
    .fn()
    .mockResolvedValueOnce({ ...environment, files: available })
    .mockImplementationOnce(async () => {
      if (denied instanceof ApiError) throw denied;
      return denied;
    });
  const fileClient = files(read);
  fileClient.list.mockImplementationOnce(() => new Promise(() => {}));
  render(panel(agentClient({ files: fileClient })));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  const request = (fileClient.list.mock.calls[0] as unknown as [string, string, ReadOptions])[2]
    .signal;
  expect(request?.aborted).toBe(false);
  fireEvent.focus(window);
  await act(async () => {});
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(request?.aborted).toBe(true);
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  expect(currentTab().queryByRole('button', { name: 'New file' })).toBeNull();
});

it('review 4221 drops the old listing when the runtime incarnation changes', async () => {
  let completeNewEnvironment!: (value: AgentEnvironment) => void;
  const fileClient = files(
    vi
      .fn()
      .mockResolvedValueOnce({ ...environment, files: available })
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            completeNewEnvironment = resolve;
          }),
      ),
  );
  fileClient.list.mockImplementationOnce(() => new Promise(() => {}));
  const client = agentClient({ files: fileClient });
  const view = render(panel(client));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  const request = (fileClient.list.mock.calls[0] as unknown as [string, string, ReadOptions])[2]
    .signal;
  expect(request?.aborted).toBe(false);
  view.rerender(panel(client, 'files', false, { runtimeVersion: 'runtime-b' }));
  await act(async () => {});
  expect(request?.aborted).toBe(true);
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  await act(async () => completeNewEnvironment({ ...environment, files: available }));
  expect(fileClient.list).toHaveBeenCalledTimes(2);
});

it('review 4221 cancels the old Runtime listing with stable props on an incarnation change', async () => {
  let completeNewEnvironment!: (value: AgentEnvironment) => void;
  const fileClient = files(
    vi
      .fn()
      .mockResolvedValueOnce({ ...environment, files: available })
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            completeNewEnvironment = resolve;
          }),
      ),
  );
  fileClient.list.mockImplementationOnce(() => new Promise(() => {}));
  const client = agentClient({ files: fileClient });
  const onClose = vi.fn();
  const onAccessChanged = vi.fn();
  const publicationScope = new ArtifactPublicationScope();
  const observation = new AgentComputerObservation({
    client,
    subject: 'owner-a',
    agentId: 'agt_1',
  });
  const detail = { ...AGENT_DETAIL, id: 'agt_1', desktop: false };
  const view = (runtimeVersion: string) => (
    <CmaClientProvider client={client}>
      <AgentSidePanel
        agentId="agt_1"
        shared={false}
        viewerId="owner-a"
        observation={observation}
        panel="files"
        detail={detail}
        resourceAvailability={{ state: 'live', runtimeVersion }}
        onClose={onClose}
        onAccessChanged={onAccessChanged}
        publicationScope={publicationScope}
      />
    </CmaClientProvider>
  );
  const rendered = render(view('runtime-a'));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  const request = (fileClient.list.mock.calls[0] as unknown as [string, string, ReadOptions])[2]
    .signal;
  expect(request?.aborted).toBe(false);
  rendered.rerender(view('runtime-b'));
  await act(async () => {});
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(request?.aborted).toBe(true);
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  await act(async () => completeNewEnvironment({ ...environment, files: available }));
  expect(fileClient.list).toHaveBeenCalledTimes(2);
});

it('review 4221 preserves the legacy Files controls when the environment route returns 404', async () => {
  const fileClient = files(vi.fn().mockRejectedValueOnce(new ApiError(404, 'Not found')));
  render(panel(agentClient({ files: fileClient })));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  expect(currentTab().getByRole('button', { name: 'New file' })).toBeEnabled();
});

it('review 4221 holds a confirmed denial while explicit environment retry is pending', async () => {
  let completeEnvironment!: (value: AgentEnvironment) => void;
  const fileClient = files(
    vi
      .fn()
      .mockResolvedValueOnce({ ...environment, files: available })
      .mockRejectedValueOnce(new ApiError(403, 'Access denied'))
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            completeEnvironment = resolve;
          }),
      ),
  );
  render(panel(agentClient({ files: fileClient })));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  fireEvent.focus(window);
  await act(async () => {});
  expect(currentTab().getByText('File access is unavailable.')).toBeVisible();
  fireEvent.click(currentTab().getByRole('button', { name: 'Try again' }));
  await act(async () => {});
  expect(fileClient.environment).toHaveBeenCalledTimes(3);
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  await act(async () => completeEnvironment({ ...environment, files: available }));
  expect(fileClient.list).toHaveBeenCalledTimes(2);
});

it('review 4221 keeps a confirmed denial when its retry also fails transiently', async () => {
  const fileClient = files(
    vi
      .fn()
      .mockResolvedValueOnce({ ...environment, files: available })
      .mockRejectedValueOnce(new ApiError(403, 'Access denied'))
      .mockRejectedValueOnce(new ApiError(503, 'Runtime unavailable')),
  );
  render(panel(agentClient({ files: fileClient })));
  await act(async () => {});
  expect(fileClient.list).toHaveBeenCalledTimes(1);
  fireEvent.focus(window);
  await act(async () => {});
  expect(currentTab().getByText('File access is unavailable.')).toBeVisible();
  fireEvent.focus(window);
  await act(async () => {});
  expect(fileClient.environment).toHaveBeenCalledTimes(3);
  expect(fileClient.list).toHaveBeenCalledTimes(1);
});

it('replaces the pending Files tree when revalidation finds a new boot of the same image', async () => {
  vi.useFakeTimers();
  let finishEnvironment!: (value: AgentEnvironment) => void;
  let finishOldTree!: (value: Awaited<ReturnType<AgentFilesClient['list']>>) => void;
  const treeEntry = (name: string) => ({
    path: `/home/${name}`,
    name,
    kind: 'file' as const,
    size_bytes: 1,
    hidden: false,
  });
  const fileClient = files(
    vi
      .fn()
      .mockResolvedValueOnce({
        ...environment,
        files: available,
        runtime_incarnation: 'boot-one',
        revalidate_at: new Date(Date.now() + 1000).toISOString(),
      })
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishEnvironment = resolve;
          }),
      ),
  );
  const list = vi
    .fn<AgentFilesClient['list']>()
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishOldTree = resolve;
        }),
    )
    .mockResolvedValueOnce([treeEntry('current.txt')]);
  render(panel(agentClient({ files: { ...fileClient, list } })));
  await act(async () => {});
  expect(list).toHaveBeenCalledTimes(1);
  const oldSignal = list.mock.calls[0]?.[2]?.signal;

  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(fileClient.environment).toHaveBeenCalledTimes(2);
  expect(oldSignal?.aborted).toBe(false);
  expect(list).toHaveBeenCalledTimes(1);

  await act(async () =>
    finishEnvironment({ ...environment, files: available, runtime_incarnation: 'boot-two' }),
  );
  expect(oldSignal?.aborted).toBe(true);
  expect(list).toHaveBeenCalledTimes(2);
  await act(async () => finishOldTree([treeEntry('obsolete.txt')]));
  expect(currentTab().getByText('current.txt')).toBeVisible();
  expect(currentTab().queryByText('obsolete.txt')).toBeNull();
});
