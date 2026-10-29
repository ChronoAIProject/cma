import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createClient } from '@chronoai/cma-sdk';
import { expect, it } from 'vitest';
import { ChannelUsage } from './ChannelUsage';

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

it('retains the open history page across a buffered focus recheck while clearing private rows', async () => {
  const pages: (string | null)[] = [];
  let holdSummary = false;
  let finishSummary: ((value: Response) => void) | undefined;
  const summary = () =>
    Response.json({
      agent_id: 'agt_1',
      channel_usage: {
        active: false,
        has_history: true,
        first_used_at: null,
        last_used_at: null,
      },
      bindings: [],
    });
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const url = new URL(String(input));
      if (url.pathname.includes('resource-access'))
        return Response.json({
          capabilities: ['manage_bridge'],
          revalidate_at: null,
          policy_revision: 1,
        });
      if (!url.pathname.endsWith('/history'))
        return holdSummary
          ? new Promise<Response>((resolve) => {
              finishSummary = resolve;
            })
          : summary();
      const cursor = url.searchParams.get('cursor');
      pages.push(cursor);
      const second = cursor !== null;
      return Response.json({
        agent_id: 'agt_1',
        items: [
          {
            revision: second ? 2 : 1,
            actor: {
              kind: 'human',
              subject: second ? 'second-page-manager' : 'first-page-manager',
            },
            at,
            detached_at: second ? at : null,
            reason: second ? 'detachment' : 'attachment',
            change: { kind: second ? 'detach' : 'attach', binding_id: binding.binding_id },
            before: second ? [binding] : [],
            after: second ? [] : [binding],
          },
        ],
        ...(second ? {} : { next: '1' }),
      });
    },
  });
  render(<ChannelUsage client={client} id="agt_1" />);
  fireEvent.click(await screen.findByRole('button', { name: 'Channel history' }));
  expect(await screen.findByText('first-page-manager')).not.toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Next channel events' }));
  expect(await screen.findByText('second-page-manager')).not.toBeVisible();
  const beforeFocus = pages.length;

  holdSummary = true;
  act(() => window.dispatchEvent(new Event('focus')));
  await waitFor(() => expect(finishSummary).toBeDefined());
  expect(screen.getByText('Previously linked to a channel')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Channel history' })).toBeDisabled();
  expect(screen.getByRole('dialog', { name: 'Channel history' })).toBeInTheDocument();
  expect(screen.queryByText(/^(first|second)-page-manager$/)).not.toBeInTheDocument();
  expect(pages).toHaveLength(beforeFocus);

  holdSummary = false;
  await act(async () => finishSummary?.(summary()));
  expect(await screen.findByText('second-page-manager')).not.toBeVisible();
  expect(screen.queryByText('first-page-manager')).not.toBeInTheDocument();
  expect(pages.at(-1)).toBe('1');
  expect(screen.getByRole('button', { name: 'Previous channel events' })).toBeEnabled();
});
