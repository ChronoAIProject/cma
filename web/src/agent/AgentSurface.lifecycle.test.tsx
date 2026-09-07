import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import type { SandboxDetail } from '../api/types';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';

afterEach(() => vi.useRealTimers());

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
    ['rotating', 'image_rotation', 'publish_active', 'Updating sandbox'],
  ] as const) {
    sandbox = {
      ...sandboxFixture(status),
      lifecycle: {
        ...sandboxFixture(status).lifecycle!,
        operation: { purpose, phase, startedAt: '2026-09-08T00:00:00Z' },
      },
    };
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(draft).toBeEnabled();
    expect(draft).toHaveValue('Keep this unsubmitted draft');
    expect(draft).toHaveAccessibleDescription(new RegExp(label));
    expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
    fireEvent.keyDown(draft, { key: 'Enter' });
    fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
    expect(runAgent).not.toHaveBeenCalled();
    expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
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
