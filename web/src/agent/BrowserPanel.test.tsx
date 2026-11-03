import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { agentClient } from '../test/agent';
import { AgentSidePanel } from './AgentSidePanel';

afterEach(() => vi.useRealTimers());

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
    return Response.json(observation(sequence >= 3 ? sequence++ : sequence));
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const client = agentClient({
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
  fireEvent.load(first);
  expect(screen.getByText('Live')).toBeInTheDocument();
  sequence = 2;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1000);
  });
  fireEvent.load(screen.getByAltText('Agent browser page'));
  const second = screen.getByAltText('Agent browser page').getAttribute('src');
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
  expect(screen.queryByText('Live')).not.toBeInTheDocument();
  expect(screen.getByText('Last frame · Reconnecting…')).toBeInTheDocument();
  disconnected = false;
  sequence = 3;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(9000);
  });
  fireEvent.load(screen.getByAltText('Agent browser page'));
  expect(screen.getByText('Live')).toBeInTheDocument();
  expect(fetch.mock.calls.some(([input]) => String(input).includes('context_id=context'))).toBe(
    true,
  );
  rerender(
    <CmaClientProvider client={client}>
      <AgentSidePanel
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

const feature = (state: string) => ({
  state,
  reason: 'proof',
  scope: 'agent_browser',
  writable: false,
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
