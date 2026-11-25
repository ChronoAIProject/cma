import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { EventGateway } from './EventGateway';
import type { ChannelListItem } from './channelTypes';

const session = {
  authenticated: true as const,
  subject: 'owner',
  permissions: ['cma-eg:admin'],
  expires_at: 9999999999,
};
const json = (value: unknown) => new Response(JSON.stringify(value));
afterEach(() => vi.unstubAllGlobals());

it('keeps independent pending-create recovery when the list exposes its incomplete reservation', async () => {
  let reservation: ChannelListItem | undefined;
  const fetcher = vi.fn<typeof fetch>(async (url, init) => {
    if (init?.method === 'POST') {
      const body = JSON.parse(String(init.body)) as { record_id: string };
      reservation = {
        record_id: body.record_id,
        version: 1,
        provider: { slug: 'cma_codex', level: 'l2' },
        definition: { name: 'Pending creation', sources: [], creation: { ready: false } },
      };
      throw new TypeError('Unknown create result');
    }
    if (url === '/api/event-gateway/v1/channels')
      return json({ items: reservation ? [reservation] : [] });
    return json({});
  });
  vi.stubGlobal('fetch', fetcher);
  vi.stubGlobal('EventSource', undefined);
  await act(async () => {
    render(<EventGateway session={session} denied={vi.fn()} />);
  });
  fireEvent.click(screen.getAllByRole('button', { name: 'New channel' })[0]!);
  const form = screen.getByRole('button', { name: 'Create channel' }).closest('form')!;
  const editor = within(form.parentElement!);
  for (const [label, value] of [
    ['Channel name', 'Pending creation'],
    ['Provider agent key', 'fixture-key'],
    ['NyxID issuer', 'https://nyx.example'],
    ['Signing key ID', 'fixture-signing'],
    ['Route IDs 1', 'fixture-route'],
  ])
    fireEvent.change(within(form).getByLabelText(label!), { target: { value } });
  await act(async () => {
    fireEvent.submit(form);
  });
  expect(editor.getByRole('button', { name: 'Retry same request' })).toBeEnabled();
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Refresh channels' }));
  });
  const row = within(screen.getByRole('tree')).getByRole('button', { name: /Pending creation/ });
  expect(row).toHaveTextContent('Creation incomplete');
  fireEvent.click(row);
  expect(form).not.toBeVisible();
  fireEvent.click(screen.getAllByRole('button', { name: 'New channel' })[0]!);
  expect(form).toBeVisible();
  expect(within(form).getByLabelText('Channel name')).toHaveValue('Pending creation');
  await act(async () => {
    fireEvent.click(editor.getByRole('button', { name: 'Retry same request' }));
  });
  const writes = fetcher.mock.calls.filter(([, init]) => init?.method === 'POST');
  expect(writes).toHaveLength(2);
  expect(writes[1]?.[0]).toBe(writes[0]?.[0]);
  expect(writes[1]?.[1]).toMatchObject({
    method: writes[0]?.[1]?.method,
    body: writes[0]?.[1]?.body,
    headers: writes[0]?.[1]?.headers,
  });
});
