import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createClient } from '@chronoai/cma-sdk';
import { expect, it } from 'vitest';
import { ChannelHistory } from './ChannelHistory';

const at = '2026-09-15T06:00:00Z';
const binding = {
  binding_id: 'binding-original',
  bridge_id: 'bridge-original',
  partition_id: 'partition-private',
  generation: 1,
  previous_binding_id: null,
  attached_at: at,
  first_used_at: null,
  last_used_at: null,
};
const first = {
  revision: 1,
  actor: { kind: 'human', subject: 'original-manager' },
  at,
  detached_at: null,
  reason: 'attachment',
  change: { kind: 'attach', binding_id: binding.binding_id },
  before: [],
  after: [binding],
};
const second = {
  revision: 2,
  actor: { kind: 'human', subject: 'later-manager' },
  at,
  detached_at: at,
  reason: 'administrative_intervention',
  change: { kind: 'detach', binding_id: binding.binding_id },
  before: [binding],
  after: [],
};
const access = { capabilities: ['manage_bridge'], revalidate_at: null, policy_revision: 1 };

it('pages retained before/after binding history and discards it when management authority is revoked', async () => {
  let revoked = false;
  const requests: string[] = [];
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const url = new URL(String(input));
      if (url.pathname.includes('resource-access'))
        return Response.json(revoked ? { ...access, capabilities: ['view_details'] } : access);
      requests.push(url.search);
      return Response.json({
        agent_id: 'agt_1',
        items: url.searchParams.has('cursor') ? [second] : [first],
        ...(!url.searchParams.has('cursor') ? { next: '1' } : {}),
      });
    },
  });
  render(<ChannelHistory client={client} id="agt_1" />);
  await screen.findByText('Channel attached');
  expect(screen.getByText(/Person original-manager/)).toBeInTheDocument();
  fireEvent.click(screen.getByText('Binding changes for event 1'));
  expect(screen.getByText('binding-original')).toBeVisible();
  expect(screen.queryByText('partition-private')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Next channel events' }));
  await screen.findByText('Administrator intervention');
  expect(requests.at(-1)).toContain('cursor=1');
  expect(screen.queryByText(/Person original-manager/)).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Next channel events' })).toBeDisabled();
  revoked = true;
  act(() => window.dispatchEvent(new Event('focus')));
  await screen.findByText('Current access does not allow channel history.');
  expect(screen.queryByText(/later-manager|binding-original/)).not.toBeInTheDocument();
});

it('withholds a buffered history response if policy changes during the read', async () => {
  let finish: ((value: Response) => void) | undefined;
  let revision = 1;
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      if (String(input).includes('resource-access'))
        return Response.json({ ...access, policy_revision: revision });
      return new Promise<Response>((resolve) => {
        finish = resolve;
      });
    },
  });
  render(<ChannelHistory client={client} id="agt_1" />);
  await waitFor(() => expect(finish).toBeDefined());
  revision = 2;
  await act(async () => finish?.(Response.json({ agent_id: 'agt_1', items: [first] })));
  await screen.findByText('Channel access changed. Refresh to check your current access.');
  expect(screen.queryByText(/original-manager|binding-original/)).not.toBeInTheDocument();
});
