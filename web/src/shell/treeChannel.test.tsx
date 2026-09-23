import { act, render, renderHook, screen, waitFor } from '@testing-library/react';
import { createClient } from '@chronoai/cma-sdk';
import { expect, it } from 'vitest';
import { useChannelFacts } from '../resources/useChannelFacts';
import { ChannelSummary } from './treeChannel';

const active = { active: true, has_history: true, first_used_at: null, last_used_at: null };
const historical = { ...active, active: false };

it('bounds optional reads, drops private binding refs and clears the prior viewer immediately', async () => {
  const pending = new Map<string, (value: Response) => void>();
  let live = 0;
  let maximum = 0;
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const id = new URL(String(input)).pathname.split('/')[4]!;
      live += 1;
      maximum = Math.max(maximum, live);
      return new Promise<Response>((resolve) =>
        pending.set(id, (value) => {
          live -= 1;
          resolve(value);
        }),
      );
    },
  });
  const ids = Array.from({ length: 6 }, (_, i) => `agt_${i}`);
  const value = (id: string) =>
    Response.json({ agent_id: id, channel_usage: historical, bindings: [] });
  const { result, rerender } = renderHook(
    ({ scope }) => useChannelFacts(client, ids, true, scope),
    { initialProps: { scope: 'one' } },
  );
  await waitFor(() => expect(pending.size).toBe(4));
  await act(async () => pending.get('agt_0')?.(value('agt_0')));
  await waitFor(() => expect(pending.has('agt_4')).toBe(true));
  await act(async () => pending.get('agt_1')?.(value('agt_1')));
  await waitFor(() => expect(pending.has('agt_5')).toBe(true));
  await act(async () => ids.slice(2).forEach((id) => pending.get(id)?.(value(id))));
  expect(maximum).toBe(4);
  await waitFor(() => expect(result.current.size).toBe(6));
  expect(
    [...result.current.values()].every((item) => item !== undefined && !('bindings' in item)),
  ).toBe(true);
  rerender({ scope: 'two' });
  expect(result.current.size).toBe(0);
});

it('keeps siblings separate and suppresses derived counts when any child fact is unavailable', () => {
  const facts = new Map([
    ['agt_active', active],
    ['agt_past', historical],
  ]);
  const view = render(
    <ChannelSummary ids={['agt_active', 'agt_past', 'agt_unknown']} facts={facts} />,
  );
  expect(screen.queryByText(/channel linked/)).not.toBeInTheDocument();
  view.rerender(<ChannelSummary ids={['agt_active', 'agt_past']} facts={facts} />);
  expect(screen.getByText('1 channel linked, 2 with channel history')).toBeInTheDocument();
});
