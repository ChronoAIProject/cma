import { useState } from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel } from '../ChannelEditor';
import { CredentialsFields } from './CredentialsFields';
import { defaultProfile, type Fields } from './model';

const secret = btoa('synthetic signing material');
const enter = (label: string, value: string) =>
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
const choose = (label: string, option: string) => {
  fireEvent.click(screen.getByRole('button', { name: label }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: option }));
};
const addCredential = (key = 'events', value = secret) => {
  fireEvent.click(screen.getByRole('button', { name: 'Add HMAC credential' }));
  enter('HMAC 1 key ID', key);
  enter('HMAC 1 secret', value);
};

function fixture(): Channel {
  return {
    channel_id: 'synthetic-channel',
    record_id: 'synthetic-record',
    version: 2,
    provider: { slug: 'cma_codex', level: 'l3' },
    definition: {
      name: 'Synthetic events',
      schema_version: 1,
      profile: defaultProfile('synthetic-owner'),
      provider: { slug: 'cma_codex' },
      sources: [{ type: 'custom', auth: { type: 'hmac', key_id: 'events' } }],
      partition: { version: 1, key: 'event_subject' },
      reply: { auto_final: true, ack_after_ms: 1500, sinks: [{ type: 'sync' }] },
    },
    endpoints: { custom_event_url: '', nyxid_callback_url: '' },
  };
}

function startCreate(fetcher = vi.fn()) {
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor ownerSubject="synthetic-owner" onDone={vi.fn()} denied={vi.fn()} />);
  enter('Channel name', 'Synthetic events');
  enter('Provider agent key', 'synthetic-provider');
  choose('Producer 1 connection type', 'Custom event');
  enter('HMAC key ID', 'events');
  return fetcher;
}

afterEach(() => vi.unstubAllGlobals());

it('shows initial HMAC setup directly without edit-only replacement semantics (#3196)', () => {
  startCreate();
  expect(screen.getByText(/Set the initial HMAC credentials/)).toBeVisible();
  expect(screen.queryByLabelText('Replace HMAC credentials')).not.toBeInTheDocument();
  expect(screen.queryByText(/replaces the whole HMAC key list/)).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Add HMAC credential' })).toBeVisible();
});

it('keeps saved credentials unless replacement is explicitly selected (#3196)', async () => {
  const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(fixture())));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ChannelEditor
      channel={fixture()}
      section={{ kind: 'producer', index: 0 }}
      onDone={vi.fn()}
      denied={vi.fn()}
    />,
  );
  expect(screen.getByLabelText('Replace HMAC credentials')).not.toBeChecked();
  expect(screen.queryByRole('button', { name: 'Add HMAC credential' })).not.toBeInTheDocument();
  enter('Prompt template', 'Summarize the event.');
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(JSON.parse(fetcher.mock.calls[0]![1].body)).not.toHaveProperty('credentials');
  await screen.findByText('Changes saved.');
  fireEvent.click(screen.getByLabelText('Replace HMAC credentials'));
  expect(screen.getByText(/replaces the whole HMAC key list/)).toBeVisible();
  addCredential();
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  expect(JSON.parse(fetcher.mock.calls[1]![1].body).credentials).toEqual({
    hmac_secrets: [{ key_id: 'events', secret }],
  });
});

it('blocks an invalid secret inline before any create request and clears its error when fixed (#3198)', async () => {
  const fetcher = startCreate(vi.fn().mockResolvedValue(new Response(JSON.stringify(fixture()))));
  addCredential('events', 'not base64!');
  const input = screen.getByLabelText('HMAC 1 secret');
  expect(input).toHaveAttribute('type', 'password');
  expect(input).toBeInvalid();
  expect(input).toHaveAccessibleDescription(/standard base64.*required padding/i);
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  expect(input).toHaveFocus();
  expect(fetcher).not.toHaveBeenCalled();
  expect(screen.queryByText(/\/credentials\/hmac_secrets/)).not.toBeInTheDocument();
  enter('HMAC 1 secret', secret);
  expect(input).toBeValid();
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(JSON.parse(fetcher.mock.calls[0]![1].body).credentials.hmac_secrets).toEqual([
    { key_id: 'events', secret },
  ]);
});

it('validates source-to-credential references as IDs and rows change (#3201)', async () => {
  const fetcher = startCreate(vi.fn().mockResolvedValue(new Response(JSON.stringify(fixture()))));
  addCredential('different');
  const sourceId = screen.getByLabelText('HMAC key ID');
  expect(sourceId).toBeInvalid();
  expect(sourceId).toHaveAccessibleDescription(/exact key ID/);
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  expect(sourceId).toHaveFocus();
  expect(fetcher).not.toHaveBeenCalled();
  enter('HMAC 1 key ID', 'events');
  expect(sourceId).toBeValid();
  fireEvent.click(screen.getByRole('button', { name: 'Remove credential 1' }));
  expect(sourceId).toBeInvalid();
  addCredential();
  expect(sourceId).toBeValid();
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
});

it('removes HMAC-only controls and omits their draft after choosing agent-key auth (#3197)', async () => {
  const fetcher = startCreate(vi.fn().mockResolvedValue(new Response(JSON.stringify(fixture()))));
  addCredential('events', 'invalid hidden draft');
  choose('Authentication method', 'NyxID agent key');
  expect(screen.queryByLabelText('HMAC 1 secret')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Add HMAC credential' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  const body = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(body.sources[0].auth).toEqual({ type: 'nyxid_key', key_hashes: [] });
  expect(body.credentials).toEqual({ agent_key: 'synthetic-provider' });
});

it('associates a backend HMAC refusal with its field without showing a JSON pointer (#3198)', async () => {
  const fetcher = startCreate(
    vi
      .fn()
      .mockResolvedValue(
        new Response(
          JSON.stringify({ detail: '/credentials/hmac_secrets/0/secret: must be base64' }),
          { status: 400 },
        ),
      ),
  );
  addCredential();
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent('HMAC 1 secret');
  expect(alert).toHaveTextContent('must be base64');
  expect(alert).not.toHaveTextContent('/credentials/');
  expect(alert).not.toHaveTextContent(secret);
  expect(screen.getByLabelText('HMAC 1 secret')).toHaveFocus();
  expect(fetcher).toHaveBeenCalledOnce();
});

it('keeps uncertain HMAC submission bytes frozen for an explicit retry', async () => {
  const fetcher = startCreate(
    vi
      .fn()
      .mockRejectedValueOnce(new TypeError('Synthetic connection interruption'))
      .mockResolvedValueOnce(new Response(JSON.stringify(fixture()))),
  );
  addCredential();
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  await screen.findByRole('button', { name: 'Retry same request' });
  expect(screen.getByLabelText('HMAC 1 secret')).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await screen.findByText('Changes saved.');
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(fetcher.mock.calls[1]![1].body).toBe(fetcher.mock.calls[0]![1].body);
  expect(fetcher.mock.calls[1]![1].headers).toEqual(fetcher.mock.calls[0]![1].headers);
});

it('marks both duplicate credential IDs inline and clears them after correction', () => {
  function Harness() {
    const [value, setValue] = useState<Fields>({
      hmac_secrets: [
        { key_id: 'events', secret },
        { key_id: 'events', secret },
      ],
    });
    return <CredentialsFields value={value} onChange={setValue} keys={[]} create hmac />;
  }
  render(<Harness />);
  expect(screen.getByLabelText('HMAC 1 key ID')).toBeInvalid();
  expect(screen.getByLabelText('HMAC 2 key ID')).toBeInvalid();
  enter('HMAC 2 key ID', 'other');
  expect(screen.getByLabelText('HMAC 1 key ID')).toBeValid();
  expect(screen.getByLabelText('HMAC 2 key ID')).toBeValid();
});
