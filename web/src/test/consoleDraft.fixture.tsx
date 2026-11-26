import { vi } from 'vitest';
import { createClient } from '@cma/client';
import { CmaClientProvider } from '../api/context';
import type { CmaWebClient } from '../api/client';
import { Console } from '../Console';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE, agentClient } from './agent';
import { WIRE_AGENT } from './rest';

export function consoleDraftFixture(overrides: Partial<CmaWebClient> = {}) {
  const sdk = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const id = new URL(String(input)).pathname.match(/\/agents\/([^/]+)$/)?.[1] ?? 'agt_1';
      return Response.json({
        ...WIRE_AGENT,
        id,
        workspace: { ...WIRE_AGENT.workspace, id: AGENT_DETAIL.workspace.id },
        sandbox: { ...WIRE_AGENT.sandbox, id: AGENT_DETAIL.sandbox.id },
      });
    },
  });
  const readAccessible = vi.spyOn(sdk.agents, 'readAccessible');
  const streams: AbortSignal[] = [];
  const runAgent = vi.fn();
  const uploadAttachment = vi.fn();
  const client = agentClient({
    resourceViews: sdk,
    readAgent: async (id) => ({ ...AGENT_DETAIL, id }),
    readAgentTranscript: async (id) => ({ ...AGENT_TRANSCRIPT, agentId: id }),
    readAgentUsage: async (id) => ({ ...AGENT_USAGE, agentId: id }),
    runAgent,
    uploadAgentAttachment: uploadAttachment,
    replayAgent: async (_id, options) =>
      new Promise<void>((resolve) => {
        streams.push(options.signal!);
        options.signal?.addEventListener('abort', () => resolve(), { once: true });
      }),
    ...overrides,
  });
  const show = (viewerId = 'owner', agentId = 'agt_1', selectedClient = client) => (
    <CmaClientProvider client={selectedClient}>
      <Console
        user={{
          id: viewerId,
          handle: viewerId,
          visitor: false,
          capabilities: ['use_own_sandboxes'],
        }}
        route={{ kind: 'agent', agentId }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>
  );
  const share = async (agentId: string) => {
    const personal = await sdk.agents.readAccessible(agentId);
    readAccessible.mockImplementation(async (id) =>
      id === agentId
        ? { ...personal, access: 'shared', id, title: 'Legitimate shared Agent' }
        : { ...personal, id },
    );
    vi.spyOn(sdk.resourceAccess, 'effective').mockImplementation(async () => ({
      capabilities: ['view_details', 'view_history'],
      revalidate_at: new Date(Date.now() + 50_000).toISOString(),
      policy_revision: 1,
      shared_by: { subject: 'another-owner', display_name: 'Another Owner', avatar_url: null },
    }));
  };
  return { client, show, readAccessible, streams, runAgent, uploadAttachment, share };
}
