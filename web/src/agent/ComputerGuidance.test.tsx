import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type {
  AgentBrowserClient,
  BrowserObservation,
  AgentEnvironment,
  EnvironmentFeature,
} from '@cma/client';
import { CmaClientProvider } from '../api/context';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { taskExperience } from '../test/taskExperience';
import { AgentSidePanel, type AgentPanel } from './AgentSidePanel';
import { Composer } from './Composer';
import { ComputerContext } from './environment/context';
import { AgentComputerObservation } from './environment/observation';
import { environmentFixture } from './environmentReview.fixture';

afterEach(() => vi.restoreAllMocks());
const restriction =
  'This service does not currently allow you to enable a live screen for this environment.';

function fixture(
  options: {
    state?: EnvironmentFeature['state'];
    policy?: boolean;
    shared?: boolean;
    incarnation?: string | null;
    policyRead?: 'pending' | 'failed' | 'wrong-owner';
    preparing?: boolean;
    running?: boolean;
    controlsUnavailable?: boolean;
    steeringMode?: 'queue' | 'interrupt';
  } = {},
) {
  const feature = { ...environmentFixture.browser, state: options.state ?? 'not_enabled' };
  const value: AgentEnvironment = {
    ...environmentFixture,
    runtime_incarnation: options.incarnation === undefined ? 'runtime-one' : options.incarnation,
    browser: feature,
    files: feature,
  };
  const environment = vi.fn(async () => value);
  const browser: BrowserObservation = {
    agent_id: 'agt_1',
    runtime_incarnation: 'runtime-one',
    context_id: null,
    state: 'ready' as const,
    location: 'Blank page',
    frame: null,
    observers: 0,
    captures: 0,
    bytes: 0,
    min_interval_ms: 1000,
    fresh_for_ms: 3000,
    max_frame_bytes: 262144,
    max_observers: 16,
  };
  const configure = vi.fn<AgentBrowserClient['configure']>(async () => browser);
  const read = vi.fn();
  const write = vi.fn();
  const send = vi.fn();
  const client = agentClient({
    files: {
      environment,
      list: vi.fn(async () => []),
      read,
      write,
      remove: vi.fn(),
      download: vi.fn(),
    },
    browser: { configure, read: vi.fn(), release: vi.fn() },
    taskExperience:
      options.policyRead === 'pending'
        ? { read: () => new Promise(() => {}) }
        : options.policyRead === 'failed'
          ? {
              read: async () => {
                throw new Error('unavailable');
              },
            }
          : taskExperience(
              options.policyRead === 'wrong-owner' ? 'another-owner' : 'owner',
              options.policy ?? false,
            ),
  });
  const observation = new AgentComputerObservation({ client, subject: 'owner', agentId: 'agt_1' });
  const publicationScope = new ArtifactPublicationScope();
  function View({
    canContinue = true,
    runtimeVersion = 'runtime-one',
  }: {
    canContinue?: boolean;
    runtimeVersion?: string;
  }) {
    const [panel, setPanel] = useState<AgentPanel>('browser');
    return (
      <CmaClientProvider client={client}>
        <ComputerContext value={observation}>
          <Composer
            status="ready"
            running={options.running ?? false}
            controlsUnavailable={options.controlsUnavailable}
            steeringMode={options.steeringMode}
            queueWhileRunning
            busy={false}
            inputAvailability={{ allowed: canContinue }}
            onSend={send}
            onStop={vi.fn()}
          />
          <AgentSidePanel
            agentId="agt_1"
            viewerId="owner"
            shared={options.shared ?? false}
            detail={{ ...AGENT_DETAIL, desktop: false }}
            panel={panel}
            onSelect={setPanel}
            onClose={vi.fn()}
            onAccessChanged={vi.fn()}
            observation={observation}
            publicationScope={publicationScope}
            resourceAvailability={
              options.preparing
                ? { state: 'inactive', notice: 'Preparing your environment.' }
                : { state: 'live', runtimeVersion }
            }
          />
        </ComputerContext>
      </CmaClientProvider>
    );
  }
  return { View, environment, configure, read, write, send, value, observation, browser };
}

it('explains policy-disabled Screen, offers keyboard help and returns to the usable composer without writes', async () => {
  const f = fixture();
  const view = render(<f.View />);
  expect(await screen.findByText(restriction)).toBeVisible();
  const tab = within(screen.getByRole('tabpanel'));
  expect(tab.getAllByRole('status')).toHaveLength(1);
  expect(tab.getByRole('status')).toHaveTextContent('Live screen is not enabled');
  expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Enable browser' })).toBeNull();
  fireEvent.click(tab.getByRole('button', { name: 'About Computer tools' }));
  const help = await screen.findByRole('dialog', { name: 'About Computer tools' });
  expect(within(help).getByText(/cannot enable a tool that this service/)).toBeVisible();
  fireEvent.keyDown(help, { key: 'Escape' });
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  fireEvent.click(tab.getByRole('button', { name: 'Back to conversation' }));
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Message' })).toHaveFocus());
  view.rerender(<f.View canContinue={false} />);
  expect(tab.queryByRole('button', { name: 'Back to conversation' })).toBeNull();
  expect(tab.queryByText('You can continue the conversation')).toBeNull();
  expect(f.configure).not.toHaveBeenCalled();
  expect(f.read).not.toHaveBeenCalled();
  expect(f.write).not.toHaveBeenCalled();
  expect(f.send).not.toHaveBeenCalled();
});

it('keeps Screen and Files explanations separate when switching tabs', async () => {
  const f = fixture();
  render(<f.View />);
  await screen.findByText(restriction);
  fireEvent.click(screen.getByRole('tab', { name: 'Files' }));
  const tab = within(screen.getByRole('tabpanel'));
  expect(tab.getByRole('status')).toHaveTextContent(
    'File browsing is not available for this environment.',
  );
  expect(tab.getAllByRole('status')).toHaveLength(1);
  expect(tab.queryByText(restriction)).toBeNull();
  expect(tab.queryByText('Live screen is not enabled')).toBeNull();
  expect(tab.getByRole('button', { name: 'About Computer tools' })).toBeVisible();
  expect(tab.getByRole('button', { name: 'Back to conversation' })).toBeVisible();
  expect(f.configure).not.toHaveBeenCalled();
  expect(f.write).not.toHaveBeenCalled();
});

it.each([
  { running: false, steeringMode: 'queue' as const, available: true },
  { running: true, steeringMode: 'queue' as const, available: true },
  { running: true, steeringMode: 'interrupt' as const, available: false },
])('matches input admission when response controls are unbound: %j', async (state) => {
  const f = fixture({ ...state, controlsUnavailable: true });
  render(<f.View />);
  await screen.findByText(restriction);
  const back = screen.queryByRole('button', { name: 'Back to conversation' });
  if (state.available) expect(back).toBeVisible();
  else expect(back).toBeNull();
  expect(f.configure).not.toHaveBeenCalled();
  expect(f.send).not.toHaveBeenCalled();
});

it.each(['pending', 'failed', 'wrong-owner'] as const)(
  'does not claim a confirmed policy restriction for a %s policy read',
  async (policyRead) => {
    const f = fixture({ policyRead });
    render(<f.View />);
    await screen.findByText('Live screen is not enabled');
    expect(screen.queryByText(restriction)).toBeNull();
    expect(screen.queryByRole('button', { name: 'Enable browser' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
    expect(f.configure).not.toHaveBeenCalled();
  },
);

it('retains explicit enablement only for the current permitted runtime', async () => {
  const f = fixture({ policy: true });
  render(<f.View />);
  const enable = await screen.findByRole('button', { name: 'Enable browser' });
  expect(screen.queryByText(restriction)).toBeNull();
  expect(f.configure).not.toHaveBeenCalled();
  fireEvent.click(enable);
  await waitFor(() => expect(f.configure).toHaveBeenCalledOnce());
  expect(f.configure).toHaveBeenCalledWith(
    'agt_1',
    'runtime-one',
    true,
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(f.send).not.toHaveBeenCalled();
});

it.each([{ shared: true }, { incarnation: null }])(
  'does not offer Enable without owner/runtime authority: %j',
  async (options) => {
    const f = fixture({ ...options, policy: true });
    render(<f.View />);
    await screen.findByText('Live screen is not enabled');
    expect(screen.queryByRole('button', { name: 'Enable browser' })).toBeNull();
    expect(screen.queryByText(restriction)).toBeNull();
    expect(f.configure).not.toHaveBeenCalled();
  },
);

it('retires an in-flight enable request when the runtime changes and requires a fresh explicit action', async () => {
  const f = fixture({ policy: true });
  let complete!: (value: BrowserObservation) => void;
  f.configure.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        complete = resolve;
      }),
  );
  const view = render(<f.View />);
  fireEvent.click(await screen.findByRole('button', { name: 'Enable browser' }));
  expect(f.configure).toHaveBeenCalledOnce();
  const originalRequest = f.configure.mock.calls[0]?.[3]?.signal;
  expect(originalRequest?.aborted).toBe(false);
  f.environment.mockResolvedValue({ ...f.value, runtime_incarnation: 'runtime-two' });
  view.rerender(<f.View runtimeVersion="runtime-two" />);
  const enable = await screen.findByRole('button', { name: 'Enable browser' });
  expect(originalRequest?.aborted).toBe(true);
  await act(async () => complete(f.browser));
  expect(f.environment).toHaveBeenCalledTimes(2);
  expect(f.configure).toHaveBeenCalledOnce();
  fireEvent.click(enable);
  await waitFor(() => expect(f.configure).toHaveBeenCalledTimes(2));
  expect(f.configure).toHaveBeenLastCalledWith(
    'agt_1',
    'runtime-two',
    true,
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(f.send).not.toHaveBeenCalled();
});

it.each(['temporarily_unavailable', 'forbidden'] as const)(
  'keeps %s distinct from not-enabled and retains a useful check',
  async (state) => {
    const f = fixture({ state });
    render(<f.View />);
    const retry = await screen.findByRole('button', { name: 'Try again' });
    fireEvent.click(retry);
    await waitFor(() => expect(f.environment).toHaveBeenCalledTimes(2));
    expect(screen.queryByText(restriction)).toBeNull();
    expect(screen.queryByText('Live screen is not enabled')).toBeNull();
    expect(f.configure).not.toHaveBeenCalled();
  },
);

it('keeps preparation separate and does not advertise disabled capability actions', async () => {
  const f = fixture({ preparing: true });
  render(<f.View />);
  expect(screen.getByText('Preparing your environment.')).toBeVisible();
  expect(screen.queryByText(restriction)).toBeNull();
  expect(screen.queryByRole('button', { name: 'Enable browser' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
  expect(f.environment).not.toHaveBeenCalled();
});

it('withdraws the conversation action when its composer unmounts', async () => {
  const f = fixture();
  const view = render(<f.View />);
  await screen.findByRole('button', { name: 'Back to conversation' });
  view.unmount();
  expect(f.observation.conversationSnapshot()).toBeUndefined();
  act(() => f.observation.returnToConversation());
  expect(f.send).not.toHaveBeenCalled();
});
