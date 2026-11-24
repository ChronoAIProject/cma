import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor } from '../ChannelEditor';
import { channel } from './NativeLarkSourceFields.fixtures';

const choose = (name: string, value: string) => {
  fireEvent.click(screen.getByRole('button', { name }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: value }));
};
const change = (label: string, value: string) =>
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
const unload = () => {
  const event = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(event);
  return event.defaultPrevented;
};
afterEach(() => vi.unstubAllGlobals());

it('retains separate producer drafts and secrets across type switches without sending inactive fields', async () => {
  const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(channel)));
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor onDone={vi.fn()} denied={vi.fn()} />);
  change('NyxID issuer', 'https://relay.example.test');
  change('Signing key ID', 'draft-signing-id');
  change('NyxID channel key', 'unsent-relay-value');
  choose('Producer 1 connection type', 'Direct Telegram bot');
  change('Bot ID', '123456');
  change('Telegram bot token', 'synthetic-bot-value');
  choose('Producer 1 connection type', 'NyxID messaging connection');
  expect(screen.getByLabelText('NyxID issuer')).toHaveValue('https://relay.example.test');
  expect(screen.getByLabelText('Signing key ID')).toHaveValue('draft-signing-id');
  expect(screen.getByLabelText('NyxID channel key')).toHaveValue('unsent-relay-value');
  choose('Producer 1 connection type', 'Direct Telegram bot');
  expect(screen.getByLabelText('Bot ID')).toHaveValue('123456');
  expect(screen.getByLabelText('Telegram bot token')).toHaveValue('synthetic-bot-value');
  change('Channel name', 'Recovery channel');
  change('Provider agent key', 'synthetic-provider-value');
  fireEvent.submit(screen.getByRole('button', { name: 'Create channel' }).closest('form')!);
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  const sent = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(sent.sources[0]).toMatchObject({ type: 'telegram', bot_id: '123456' });
  expect(sent.sources[0]).not.toHaveProperty('issuer');
  expect(sent.credentials).not.toHaveProperty('channel_key');
});

it('warns before reload only after edits, retains the draft on cancellation, and detaches on unmount', () => {
  const view = render(<ChannelEditor onDone={vi.fn()} denied={vi.fn()} />);
  expect(unload()).toBe(false);
  change('NyxID channel key', 'synthetic-private-draft');
  expect(unload()).toBe(true);
  expect(screen.getByLabelText('NyxID channel key')).toHaveValue('synthetic-private-draft');
  expect(localStorage.length).toBe(0);
  expect(sessionStorage.length).toBe(0);
  view.unmount();
  expect(unload()).toBe(false);
});

it('clears a rejected editable error when the producer type changes', async () => {
  const fetcher = vi.fn().mockResolvedValue(
    new Response(JSON.stringify({ detail: '/sources/0/bot_id: Invalid bot identity' }), {
      status: 400,
    }),
  );
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor onDone={vi.fn()} denied={vi.fn()} />);
  choose('Producer 1 connection type', 'Direct Telegram bot');
  change('Channel name', 'Recovery channel');
  change('Bot ID', '123');
  change('Provider agent key', 'synthetic-provider-value');
  fireEvent.submit(screen.getByRole('button', { name: 'Create channel' }).closest('form')!);
  await screen.findByRole('alert');
  choose('Producer 1 connection type', 'NyxID messaging connection');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(fetcher).toHaveBeenCalledOnce();
});

it('keeps unknown-write errors and the original request through edits and retry, and removes the reload warning after success', async () => {
  const fetcher = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Unconfirmed transport'))
    .mockResolvedValueOnce(new Response(JSON.stringify(channel)));
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor onDone={vi.fn()} denied={vi.fn()} />);
  choose('Producer 1 connection type', 'Direct Telegram bot');
  change('Channel name', 'Recovery channel');
  change('Bot ID', '123');
  change('Provider agent key', 'synthetic-provider-value');
  fireEvent.submit(screen.getByRole('button', { name: 'Create channel' }).closest('form')!);
  await screen.findByText('Unconfirmed transport');
  expect(screen.getByLabelText('Bot ID')).toBeDisabled();
  expect(unload()).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await waitFor(() => expect(screen.getByText('Changes saved.')).toBeInTheDocument());
  expect(fetcher.mock.calls[1]![0]).toBe(fetcher.mock.calls[0]![0]);
  for (const key of ['method', 'body', 'headers'])
    expect(fetcher.mock.calls[1]![1][key]).toEqual(fetcher.mock.calls[0]![1][key]);
  await waitFor(() => expect(unload()).toBe(false));
});

it('clears the rejected field validity and description after correction', async () => {
  const fetcher = vi.fn().mockResolvedValue(
    new Response(JSON.stringify({ detail: '/sources/0/bot_id: Check bot identity.' }), {
      status: 400,
    }),
  );
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor onDone={vi.fn()} denied={vi.fn()} />);
  choose('Producer 1 connection type', 'Direct Telegram bot');
  change('Channel name', 'Recovery channel');
  change('Bot ID', '123');
  change('Provider agent key', 'synthetic-provider-value');
  fireEvent.submit(screen.getByRole('button', { name: 'Create channel' }).closest('form')!);
  await screen.findByRole('alert');
  expect(screen.getByLabelText('Bot ID')).toBeInvalid();
  change('Bot ID', '456');
  const field = screen.getByLabelText('Bot ID');
  expect(field).toBeValid();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(field).not.toHaveAccessibleDescription(/Check bot identity/);
  for (const id of (field.getAttribute('aria-describedby') ?? '').split(/\s+/).filter(Boolean))
    expect(document.getElementById(id)).not.toBeNull();
});
