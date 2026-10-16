import '../test/connectedAccounts';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import type { SandboxDetail } from '../api/types';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';

afterEach(() => vi.useRealTimers());

it('retains usage and drafts across lifecycle changes without polling machine resources from the conversation', async () => {
  vi.useFakeTimers();
  let sandbox: SandboxDetail = { ...sandboxFixture('paused'), runtimeVersion: undefined };
  let acceptInitial: (value: SandboxDetail) => void = () => {};
  const readSandbox = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise<SandboxDetail>((resolve) => {
          acceptInitial = resolve;
        }),
    )
    .mockImplementation(async () => sandbox);
  const readAgentResources = vi.fn(async () => {
    if (sandbox.status !== 'active') throw new ApiError(422, 'No runtime');
    return {
      agent_id: 'agt_1',
      sandbox_id: 'sbx_1',
      uptime_ms: 10000,
      cpu_usage_usec: 1000000,
      cpu_millis_limit: 4000,
      memory_bytes_used: 1024 ** 3,
      memory_bytes_limit: 8 * 1024 ** 3,
      memory_bytes_peak: null,
      stall_permille: null,
      ceiling_hits: null,
      allocation_failures: null,
      kills: null,
      received_bytes: 2048,
      sent_bytes: 1024,
    };
  });
  const runAgent = vi.fn();
  const client = agentClient({ readSandbox, readAgentResources, runAgent });
  function ObservedAgent() {
    const controller = useAgentController(client, 'agt_1');
    return (
      <AgentSurface
        agentId="agt_1"
        controller={controller}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        onNavigate={vi.fn()}
      />
    );
  }
  render(
    <CmaClientProvider client={client}>
      <ObservedAgent />
    </CmaClientProvider>,
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(readSandbox).toHaveBeenCalledOnce();
  expect(readAgentResources).not.toHaveBeenCalled();
  await act(async () => acceptInitial(sandbox));
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(readSandbox).toHaveBeenCalledTimes(3);
  expect(readAgentResources).not.toHaveBeenCalled();
  expect(screen.queryByLabelText('Sandbox resources')).not.toBeInTheDocument();
  expect(screen.getByRole('region', { name: 'Agent usage' })).toHaveTextContent(
    'Total tokens · 120 input · 60 output',
  );
  expect(screen.getByRole('progressbar', { name: 'Context' })).toBeInTheDocument();
  const draft = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(draft, { target: { value: 'Preserve this draft' } });
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  sandbox = sandboxFixture('active');
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(readAgentResources).not.toHaveBeenCalled();
  expect(screen.queryByLabelText('Sandbox resources')).not.toBeInTheDocument();
  expect(draft).toHaveValue('Preserve this draft');
  expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
  expect(runAgent).not.toHaveBeenCalled();
});

it('holds Send and Enter during operator lifecycle changes while retaining editable drafts and history', async () => {
  vi.useFakeTimers();
  let sandbox: SandboxDetail = sandboxFixture();
  let unavailable = false;
  const runAgent = vi.fn().mockResolvedValue(undefined);
  const client = agentClient({
    readSandbox: async () => {
      if (unavailable) throw new Error('read unavailable');
      return sandbox;
    },
    runAgent,
  });
  function ObservedAgent() {
    const controller = useAgentController(client, 'agt_1');
    return (
      <AgentSurface
        agentId="agt_1"
        controller={controller}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        onNavigate={vi.fn()}
      />
    );
  }
  render(
    <CmaClientProvider client={client}>
      <ObservedAgent />
    </CmaClientProvider>,
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  const draft = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(draft, { target: { value: 'Keep this unsubmitted draft' } });
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();

  for (const [status, purpose, phase, label] of [
    ['pausing', 'pause', 'capture_and_publish_checkpoint', 'Pausing'],
    ['pausing', 'pause', 'teardown_runtime', 'Pausing'],
    ['reviving', 'revive', 'home_restore', 'Resuming'],
    ['rotating', 'image_rotation', 'publish_active', 'Resuming'],
  ] as const) {
    sandbox = {
      ...sandboxFixture(status),
      lifecycle: {
        ...sandboxFixture(status).lifecycle!,
        operation: { purpose, phase, startedAt: '2026-09-08T00:00:00Z' },
        interruption:
          status === 'reviving'
            ? {
                cause: 'out_of_memory',
                detectedAt: '2026-09-08T00:00:00Z',
                memoryLimitMib: 8192,
              }
            : undefined,
      },
    };
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(draft).toBeEnabled();
    expect(draft).toHaveValue('Keep this unsubmitted draft');
    expect(draft).toHaveAccessibleDescription(new RegExp(label));
    expect(screen.queryByRole('region', { name: 'Sandbox progress' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
    fireEvent.keyDown(draft, { key: 'Enter' });
    fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
    expect(runAgent).not.toHaveBeenCalled();
    expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
    if (status === 'reviving') {
      expect(screen.getByRole('region', { name: 'Sandbox recovery' })).toHaveTextContent(
        'Recovering your sandbox',
      );
      expect(screen.getByText(/ran out of memory. Configured memory: 8 GiB/)).toBeInTheDocument();
      expect(screen.getByText('Reconnecting your saved home')).toBeInTheDocument();
    }
  }

  unavailable = true;
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(draft).toHaveAccessibleDescription(/Reconnecting/);
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(draft).toHaveValue('Keep this unsubmitted draft');
  unavailable = false;
  sandbox = sandboxFixture('paused');
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  expect(draft).toHaveValue('Keep this unsubmitted draft');
  expect(runAgent).not.toHaveBeenCalled();
  await act(async () => {
    fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
  });
  expect(runAgent).toHaveBeenCalledOnce();
});
