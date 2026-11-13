import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { agentClient } from '../test/agent';
import { AgentSidePanel } from './AgentSidePanel';
import { AgentComputerObservation } from './environment/observation';

afterEach(() => vi.useRealTimers());

it('withdraws existing shared output when current read authority expires or renewal fails', async () => {
  let renewal: 'ready' | 'pending' | 'failed' = 'ready';
  const sdk = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      if (String(input).includes('/resource-access/')) {
        if (renewal === 'pending') return new Promise<Response>(() => {});
        if (renewal === 'failed')
          return Response.json({ code: 'unavailable', detail: 'Renewal failed' }, { status: 503 });
        return Response.json({
          capabilities: ['view_details', 'view_history'],
          revalidate_at: new Date(Date.now() + 50_000).toISOString(),
          policy_revision: 1,
        });
      }
      return Response.json({
        access: 'shared',
        id: 'agt_1',
        title: 'Shared investigation',
        status: 'ready',
        created_at: '2026-09-01T00:00:00Z',
        updated_at: '2026-09-01T00:00:00Z',
      });
    },
  });
  const client = agentClient({ resourceViews: sdk });
  const observation = new AgentComputerObservation({ client, subject: 'reader', agentId: 'agt_1' });
  observation.publish([
    {
      id: 'command',
      runId: '1',
      title: 'printf scoped-proof',
      kind: 'execute',
      status: 'completed',
      output: { text: 'scoped-proof', truncated: false },
    },
  ]);
  vi.useFakeTimers({
    toFake: ['Date', 'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'],
  });
  const view = render(
    <CmaClientProvider client={client}>
      <AgentSidePanel
        agentId="agt_1"
        shared
        panel="activity"
        onClose={vi.fn()}
        onAccessChanged={vi.fn()}
        publicationScope={new ArtifactPublicationScope()}
        observation={observation}
      />
    </CmaClientProvider>,
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByText('scoped-proof', { exact: true })).toBeVisible();
  expect(screen.getByText('scoped-proof', { exact: true })).toBeVisible();
  renewal = 'pending';
  vi.setSystemTime(Date.now() + 50_001);
  fireEvent.focus(window);
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.queryByText('scoped-proof', { exact: true })).not.toBeInTheDocument();
  expect(screen.getByText('Checking operation output access…')).toBeVisible();
  await act(() => vi.advanceTimersByTimeAsync(15_001));
  expect(screen.queryByText('scoped-proof', { exact: true })).not.toBeInTheDocument();
  expect(
    screen.getByText('Operation output access could not be checked. Try again.'),
  ).toBeVisible();
  renewal = 'ready';
  fireEvent.click(screen.getByRole('button', { name: 'Check output access again' }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByText('scoped-proof', { exact: true })).toBeVisible();
  expect(screen.getByText('scoped-proof', { exact: true })).toBeVisible();
  renewal = 'failed';
  fireEvent.focus(window);
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.queryByText('scoped-proof', { exact: true })).not.toBeInTheDocument();
  expect(
    screen.getByText('Operation output access could not be checked. Try again.'),
  ).toBeVisible();
  view.unmount();
});
