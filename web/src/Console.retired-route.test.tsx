import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient, type AccessibleAgent } from '@cma/client';
import { agent as parseAgent } from './api-client/agents/types';
import { agentDetail } from './api/rest/hierarchy';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import type { ConsoleProps } from './ConsoleTypes';
import { AGENT_TRANSCRIPT } from './test/agent';
import { WIRE_AGENT } from './test/rest';
import { harness } from './agent/transport/controller.fixture';
import { persistDeletion, restoreDeletions } from './deletionRecovery';
import * as connections from './connections/preparation';

afterEach(() => vi.restoreAllMocks());

const personal = parseAgent({
  ...WIRE_AGENT,
  id: 'agt_1',
  workspace: { ...WIRE_AGENT.workspace, id: 'wsp_1' },
  sandbox: { ...WIRE_AGENT.sandbox, id: 'sbx_1', status: 'retired' },
});
const props: ConsoleProps = {
  user: { id: 'alice', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] },
  route: { kind: 'agent', agentId: 'agt_1' },
  authEpoch: 7,
  onNavigate: vi.fn(),
  onSignOut: vi.fn(),
};
function fixture() {
  const h = harness();
  const sdk = createClient({ fetch: async () => Response.json({ items: [] }) });
  const readAccessible = vi.spyOn(sdk.agents, 'readAccessible').mockResolvedValue(personal);
  const client = { ...h.client, resourceViews: sdk };
  const prepare = vi.spyOn(client.agentTransport!, 'prepareOpening');
  const readAgent = vi.spyOn(client, 'readAgent').mockResolvedValue(agentDetail(personal));
  const transcript = vi.spyOn(client, 'readAgentTranscript').mockResolvedValue(AGENT_TRANSCRIPT);
  const usage = vi.spyOn(client, 'readAgentUsage');
  const connection = vi.spyOn(connections, 'readAgentConnections');
  const draw = (changes: Partial<ConsoleProps> = {}) => (
    <CmaClientProvider client={client}>
      <Console {...props} {...changes} />
    </CmaClientProvider>
  );
  const assertNoRuntime = () => {
    for (const call of [
      prepare,
      h.eligibility,
      h.snapshot,
      h.head,
      h.listQueued,
      h.post,
      client.replayAgent,
      client.runAgent,
      usage,
      connection,
    ])
      expect(call).not.toHaveBeenCalled();
    expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
    expect(document.querySelector('.agent-side-panel')).toBeNull();
  };
  return { ...h, client, readAccessible, readAgent, transcript, draw, assertNoRuntime };
}

it('opens and reloads a retired URL using only authorized retained history and exact parent settlement', async () => {
  const h = fixture();
  const parent = {
    action: { kind: 'delete_sandbox' as const, sandboxId: 'sbx_1' },
    key: 'accepted-parent',
    accepted: true,
  };
  const unknown = {
    ...parent,
    key: 'unknown-sibling',
    accepted: false,
    action: { kind: 'delete_sandbox' as const, sandboxId: 'sbx_other' },
  };
  persistDeletion('alice', parent);
  persistDeletion('alice', unknown);
  let view = render(h.draw());
  await screen.findByText('The repository is ready.');
  await waitFor(() =>
    expect(h.transcript).toHaveBeenCalledWith(
      'agt_1',
      expect.objectContaining({ cache: 'no-store' }),
    ),
  );
  expect(screen.getByRole('group', { name: 'Retired' })).toBeVisible();
  expect(restoreDeletions('alice')).toEqual([unknown]);
  h.assertNoRuntime();
  view.unmount();
  view = render(h.draw());
  await screen.findByText('The repository is ready.');
  h.assertNoRuntime();
  expect(restoreDeletions('alice')).toEqual([unknown]);
  view.unmount();
});

it.each(['route', 'viewer', 'client', 'unmount'] as const)(
  'does not open a late retired route after %s replacement',
  async (mode) => {
    const h = fixture();
    let finish!: (value: AccessibleAgent) => void;
    h.readAccessible.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const view = render(h.draw());
    await waitFor(() => expect(h.readAccessible).toHaveBeenCalled());
    h.assertNoRuntime();
    if (mode === 'unmount') view.unmount();
    else if (mode === 'client')
      view.rerender(
        <CmaClientProvider client={{ ...h.client, resourceViews: undefined }}>
          <Console {...props} route={{ kind: 'home' }} />
        </CmaClientProvider>,
      );
    else
      view.rerender(
        h.draw(
          mode === 'route'
            ? { route: { kind: 'home' } }
            : { user: { ...props.user, id: 'bob' }, route: { kind: 'home' } },
        ),
      );
    await act(async () => {
      finish(personal);
    });
    expect(screen.queryByRole('group', { name: 'Retired' })).not.toBeInTheDocument();
    expect(screen.queryByText('The repository is ready.')).not.toBeInTheDocument();
    expect(h.transcript).not.toHaveBeenCalled();
    h.assertNoRuntime();
  },
);

it('does not reopen a retired runtime from the previous authentication epoch', async () => {
  const h = fixture();
  const view = render(h.draw());
  await screen.findByText('The repository is ready.');
  h.assertNoRuntime();
  h.readAccessible.mockImplementation(() => new Promise(() => {}));
  view.rerender(h.draw({ authEpoch: 8 }));
  await act(async () => {});
  expect(screen.queryByText('The repository is ready.')).not.toBeInTheDocument();
  h.assertNoRuntime();
});
