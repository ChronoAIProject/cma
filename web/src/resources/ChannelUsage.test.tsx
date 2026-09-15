import { act, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { ChannelUsage } from './ChannelUsage';

it('shows active or historical use without rendering private binding identifiers and clears revoked facts', async () => {
  let state = 'active';
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async () =>
      state === 'revoked'
        ? Response.json({ code: 'resource_not_found', detail: 'Access removed' }, { status: 404 })
        : Response.json({
            agent_id: 'agt_1',
            channel_usage: {
              active: state === 'active',
              has_history: true,
              first_used_at: null,
              last_used_at: null,
            },
            bindings:
              state === 'active'
                ? [
                    {
                      binding_id: 'private-binding',
                      bridge_id: 'private-bridge',
                      partition_id: 'private-partition',
                      generation: 1,
                      previous_binding_id: null,
                      attached_at: '2026-09-15T06:00:00Z',
                      first_used_at: null,
                      last_used_at: null,
                    },
                  ]
                : [],
          }),
  });
  render(<ChannelUsage client={client} id="agt_1" />);
  await screen.findByText('Linked to a channel');
  expect(screen.queryByText(/private-/)).not.toBeInTheDocument();
  state = 'historical';
  act(() => window.dispatchEvent(new Event('focus')));
  await screen.findByText('Previously linked to a channel');
  state = 'revoked';
  act(() => window.dispatchEvent(new Event('focus')));
  await screen.findByText('Channel activity unavailable.');
  expect(screen.queryByText('Previously linked to a channel')).not.toBeInTheDocument();
});
it('discards a late channel summary after the resource changes', async () => {
  let finish: ((response: Response) => void) | undefined;
  const fetch = vi.fn(async (input: RequestInfo | URL) =>
    String(input).includes('agt_1')
      ? new Promise<Response>((resolve) => {
          finish = resolve;
        })
      : Response.json({
          agent_id: 'agt_2',
          channel_usage: {
            active: false,
            has_history: false,
            first_used_at: null,
            last_used_at: null,
          },
        }),
  );
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const view = render(<ChannelUsage client={client} id="agt_1" />);
  await waitFor(() => expect(finish).toBeDefined());
  view.rerender(<ChannelUsage client={client} id="agt_2" />);
  await screen.findByText('No channel activity recorded');
  await act(async () =>
    finish?.(
      Response.json({
        agent_id: 'agt_1',
        channel_usage: { active: true, has_history: true, first_used_at: null, last_used_at: null },
      }),
    ),
  );
  expect(screen.queryByText('Linked to a channel')).not.toBeInTheDocument();
});
