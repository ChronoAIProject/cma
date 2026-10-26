import { act, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { AgentProvenance } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import { OwnerViewsProvider } from '../resources/retainedOwnerViews';
import { useAgentProvenance, useOwnerCreationLabel } from './provenance';

const provenance: AgentProvenance = {
  agent_id: 'agt_one',
  created_at: '2026-09-06T00:00:00Z',
  creation_origin: 'manual',
  details: {
    created_by: { kind: 'human', subject: 'owner' },
    trigger: null,
    channel_bridge: null,
    workspace_profile_index: null,
  },
};
function Probe({ enabled = true, owner = 'owner', id = 'agt_one' }) {
  const value = useAgentProvenance(id, enabled);
  const label = useOwnerCreationLabel(value, owner, true);
  return (
    <>
      <p>{label}</p>
      <p>
        {value.status === 'ready' && value.value.details ? 'Detailed facts' : 'No detailed facts'}
      </p>
    </>
  );
}
function fixture() {
  const readAgentProvenance = vi.fn(async () => provenance);
  const client = agentClient({ readAgentProvenance });
  const draw = (
    show: boolean,
    enabled = true,
    viewer = 'owner',
    activeClient = client,
    resourceOwner = 'owner',
    id = 'agt_one',
  ) => (
    <CmaClientProvider client={activeClient}>
      <OwnerViewsProvider viewerId={viewer}>
        {show ? <Probe enabled={enabled} owner={resourceOwner} id={id} /> : null}
      </OwnerViewsProvider>
    </CmaClientProvider>
  );
  return { readAgentProvenance, client, draw };
}

it('retains only the owner creation category while facts and authority revalidate on return', async () => {
  const f = fixture();
  const view = render(f.draw(true));
  await screen.findByText('Detailed facts');
  f.readAgentProvenance.mockImplementation(() => new Promise(() => {}));
  view.rerender(f.draw(false));
  view.rerender(f.draw(true, false));
  expect(screen.getByText('Created manually')).toBeInTheDocument();
  expect(screen.getByText('No detailed facts')).toBeInTheDocument();
  view.rerender(f.draw(true));
  expect(screen.getByText('Created manually')).toBeInTheDocument();
  expect(screen.getByText('No detailed facts')).toBeInTheDocument();
});

it.each(['viewer', 'client', 'owner', 'resource'] as const)(
  'does not reuse an origin across %s changes',
  async (change) => {
    const f = fixture();
    const view = render(f.draw(true));
    await screen.findByText('Detailed facts');
    f.readAgentProvenance.mockImplementation(() => new Promise(() => {}));
    view.rerender(f.draw(false));
    view.rerender(
      f.draw(
        true,
        true,
        change === 'viewer' ? 'other' : 'owner',
        change === 'client' ? { ...f.client } : f.client,
        change === 'owner' ? 'other' : 'owner',
        change === 'resource' ? 'agt_other' : 'agt_one',
      ),
    );
    expect(screen.getByText('Loading creation origin')).toBeInTheDocument();
    expect(screen.queryByText('Created manually')).not.toBeInTheDocument();
  },
);

it('drops a retained category after failure and ignores the prior visit late reply', async () => {
  const f = fixture();
  const view = render(f.draw(true));
  await screen.findByText('Detailed facts');
  let finish!: (value: AgentProvenance) => void;
  f.readAgentProvenance
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    )
    .mockRejectedValue(new Error('Access removed'));
  view.rerender(f.draw(false));
  view.rerender(f.draw(true));
  view.rerender(f.draw(false));
  view.rerender(f.draw(true));
  await screen.findByText('Creation origin unavailable');
  await act(async () => finish(provenance));
  expect(screen.queryByText('Created manually')).not.toBeInTheDocument();
  f.readAgentProvenance.mockImplementation(() => new Promise(() => {}));
  view.rerender(f.draw(false));
  view.rerender(f.draw(true));
  expect(screen.getByText('Loading creation origin')).toBeInTheDocument();
});
