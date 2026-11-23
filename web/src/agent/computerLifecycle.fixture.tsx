import { vi } from 'vitest';
import type { ComponentProps } from 'react';
import { CmaClientProvider } from '../api/context';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { AgentSidePanel } from './AgentSidePanel';
import { AgentComputerObservation } from './environment/observation';
import { environmentFixture } from './environmentReview.fixture';
import type { SandboxResourceAvailability } from './sandboxInput';
import type { CmaWebClient } from '../api/client';

export const paused: SandboxResourceAvailability = {
  state: 'paused',
  notice: 'Sandbox paused. Your work is preserved. Resume the Sandbox to use live Computer.',
};
export const live: SandboxResourceAvailability = { state: 'live', runtimeVersion: 'runtime-one' };

export function computerFixture(overrides: Partial<CmaWebClient> = {}) {
  const files = {
    environment: vi.fn(async () => environmentFixture),
    list: vi.fn(async () => []),
    read: vi.fn(),
    download: vi.fn(),
    write: vi.fn(),
    remove: vi.fn(),
  };
  const browser = { read: vi.fn(), release: vi.fn(), configure: vi.fn() };
  const mutations = { reviveSandbox: vi.fn(), runAgent: vi.fn(), stopAgent: vi.fn() };
  const client = agentClient({ files, browser, ...mutations, ...overrides });
  const observation = new AgentComputerObservation({ client, agentId: 'agt_1', subject: 'owner' });
  observation.publish([
    {
      id: 'retained',
      title: 'printf retained-output',
      kind: 'execute',
      status: 'completed',
      output: { text: 'retained-output', truncated: false },
    },
  ]);
  const onOpenSandbox = vi.fn();
  const onClose = vi.fn();
  const publicationScope = new ArtifactPublicationScope();
  const draw = (props: Partial<ComponentProps<typeof AgentSidePanel>> = {}) => (
    <CmaClientProvider client={client}>
      <AgentSidePanel
        agentId="agt_1"
        viewerId="owner"
        shared={false}
        detail={{
          ...AGENT_DETAIL,
          desktop: false,
          sandbox: { ...AGENT_DETAIL.sandbox, status: 'paused' },
        }}
        resourceAvailability={paused}
        panel="files"
        observation={observation}
        onClose={onClose}
        onAccessChanged={vi.fn()}
        onOpenSandbox={onOpenSandbox}
        publicationScope={publicationScope}
        {...props}
      />
    </CmaClientProvider>
  );
  return { files, browser, mutations, client, observation, onOpenSandbox, onClose, draw };
}
