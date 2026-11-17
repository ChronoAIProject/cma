import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { AgentSidePanel } from './AgentSidePanel';
import { CmaClientProvider } from '../api/context';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import type { AgentEnvironment } from '@cma/client';

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
        screen.getByText(
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
