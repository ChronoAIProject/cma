import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel, type EditorSection } from '../ChannelEditor';
import { channelDraft, channelRequest, defaultProfile, object } from './model';
import { routingRequest } from './RoutingFields';
const reply = (v: unknown) => new Response(JSON.stringify(v));
afterEach(() => vi.unstubAllGlobals());
function fixture(): Channel {
  return {
    channel_id: 'channel_fixture',
    record_id: 'record_fixture',
    version: 7,
    provider: { slug: 'cma', level: 'l3' },
    definition: {
      schema_version: 1,
      name: 'Existing channel',
      record_id: 'server_record',
      owner: { kind: 'human', subject: 'owner' },
      created_at: 'server_time',
      version: 7,
      creation: { ready: true },
      profile_digest: 'digest',
      secrets: { agent_key_hash: 'hash' },
      profile: { ...defaultProfile('owner'), future_profile: { retained: true } },
      provider: {
        slug: 'cma',
        binding_id: 'binding_original',
        level: 'l3',
        target: { type: 'existing_agent', agent_id: 'agt_retained' },
      },
      connection_ref: 'connection_retained',
      sources: [
        {
          type: 'nyxid_relay',
          platform: 'discord',
          issuer: 'https://nyxid.example.test',
          key_id: 'key',
          jwks_url: 'https://nyxid.example.test/jwks',
          route_ids: ['route_one'],
          bot_id: '123',
          trust_normalized: true,
          admission: { type: 'all' },
          future_source: { retained: true },
        },
        { type: 'telegram', bot_id: '456', admission: { type: 'all' } },
      ],
      partition: { version: 1, key: 'conversation_and_sender', future_partition: 'keep' },
      reply: { auto_final: true, ack_after_ms: 1500, sinks: [{ type: 'source' }] },
      future_channel: { keep: 'exact' },
      routing: {
        version: 1,
        default_consumer: { name: 'default' },
        consumers: [
          {
            id: 'reviewer',
            name: 'reviewer',
            label: 'Review',
            profile: defaultProfile('owner'),
            profile_digest: 'digest',
            provider: { slug: 'cma', binding_id: 'binding_reviewer' },
            agent_key_hash: 'public_hash',
            pending: false,
            created_at: 'server_time',
            future_policy: { keep: true },
          },
        ],
        default_targets: ['default'],
        rules: [],
        max_targets: 4,
        reply_label: 'auto',
      },
    },
    endpoints: {
      custom_event_url: 'https://example.test/events',
      nyxid_callback_url: 'https://example.test/callback',
    },
  };
}
it('uses the registered CMA provider consistently for a new channel without changing retained providers', () => {
  const fresh = channelDraft(undefined, 'owner');
  expect(fresh).toMatchObject({
    provider: { slug: 'cma_codex' },
    profile: { metadata: { provider: 'cma_codex', owner: { subject: 'owner' } } },
  });
  const retained = fixture();
  retained.definition.profile = {
    ...object(retained.definition.profile),
    metadata: { ...object(defaultProfile('owner').metadata), provider: 'cma' },
  };
  const draft = channelDraft(retained);
  expect(draft).toMatchObject({
    provider: { slug: 'cma', target: { type: 'existing_agent', agent_id: 'agt_retained' } },
    profile: { metadata: { provider: 'cma' } },
  });
});
it('preserves unknown writable fields and exact existing bindings while excluding server-only fields and unchanged credentials', () => {
  const source = fixture(),
    draft = channelDraft(source);
  const body = channelRequest({ ...draft, name: 'Renamed' }, {}, source);
  expect(body).toMatchObject({
    name: 'Renamed',
    expected_version: 7,
    connection_ref: 'connection_retained',
    future_channel: { keep: 'exact' },
    profile: { future_profile: { retained: true } },
    provider: { slug: 'cma', target: { type: 'existing_agent', agent_id: 'agt_retained' } },
    partition: { future_partition: 'keep' },
  });
  for (const key of [
    'record_id',
    'creation',
    'owner',
    'profile_digest',
    'secrets',
    'created_at',
    'version',
    'routing',
    'credentials',
  ])
    expect(body).not.toHaveProperty(key);
  expect(body.provider).not.toHaveProperty('binding_id');
  expect(body.provider).not.toHaveProperty('level');
  const sourceDraft = (body.sources as unknown[])[0];
  expect(sourceDraft).not.toHaveProperty('jwks_url');
  expect(sourceDraft).toMatchObject({ trust_normalized: true, future_source: { retained: true } });
});
it('only replaces explicitly entered credentials and preserves HMAC keys unless replacement is selected', () => {
  const source = fixture();
  const body = channelRequest(
    channelDraft(source),
    {
      agent_key: '',
      channel_key: 'replacement',
      hmac_secrets: [{ key_id: 'not-selected', secret: 'not-selected' }],
    },
    source,
  );
  expect(body.credentials).toEqual({ channel_key: 'replacement' });
  expect(
    channelRequest(channelDraft(source), { replace_hmac: true, hmac_secrets: [] }, source)
      .credentials,
  ).toEqual({ hmac_secrets: [] });
});
it('shows only the selected node, retains drafts across node selection, and saves only that producer', async () => {
  const source = fixture(),
    done = vi.fn();
  const fetcher = vi.fn().mockImplementation(async (_url, init) => {
    const body = JSON.parse(init.body);
    return reply({ ...source, version: 8, definition: { ...source.definition, ...body } });
  });
  vi.stubGlobal('fetch', fetcher);
  const view = (section: EditorSection) => (
    <ChannelEditor channel={source} section={section} onDone={done} denied={vi.fn()} />
  );
  const { rerender } = render(view({ kind: 'channel' }));
  fireEvent.change(screen.getByLabelText('Channel name'), {
    target: { value: 'Unsaved channel title' },
  });
  expect(screen.queryByLabelText('Bot ID')).not.toBeInTheDocument();
  rerender(view({ kind: 'producer', index: 0 }));
  expect(screen.queryByLabelText('Channel name')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('Model')).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Bot ID'), { target: { value: '789' } });
  rerender(view({ kind: 'producer', index: 1 }));
  expect(screen.getByLabelText('Bot ID')).toHaveValue('456');
  rerender(view({ kind: 'producer', index: 0 }));
  expect(screen.getByLabelText('Bot ID')).toHaveValue('789');
  await userEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await waitFor(() => expect(done).toHaveBeenCalledOnce());
  const body = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(body.name).toBe('Existing channel');
  expect(body.sources[0].bot_id).toBe('789');
  expect(body.sources[1].bot_id).toBe('456');
  expect(body.provider.target.agent_id).toBe('agt_retained');
  expect(body).not.toHaveProperty('credentials');
  rerender(view({ kind: 'channel' }));
  expect(screen.getByLabelText('Channel name')).toHaveValue('Unsaved channel title');
  await userEvent.click(screen.getByRole('button', { name: 'Save channel' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  expect(JSON.parse(fetcher.mock.calls[1]![1].body)).toMatchObject({
    expected_version: 8,
    name: 'Unsaved channel title',
  });
});
it('keeps an uncertain request body, key, endpoint and version exact after selecting a different node', async () => {
  const source = fixture(),
    fetcher = vi
      .fn()
      .mockRejectedValueOnce(new TypeError('Connection interrupted'))
      .mockResolvedValueOnce(reply({ ...source, version: 8 }));
  vi.stubGlobal('fetch', fetcher);
  const view = (section: EditorSection) => (
    <ChannelEditor channel={source} section={section} onDone={vi.fn()} denied={vi.fn()} />
  );
  const { rerender } = render(view({ kind: 'producer', index: 0 }));
  fireEvent.change(screen.getByLabelText('Bot ID'), { target: { value: '987' } });
  fireEvent.change(screen.getByLabelText('NyxID channel key'), {
    target: { value: 'replacement-secret' },
  });
  await userEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await screen.findByRole('button', { name: 'Retry same request' });
  rerender(view({ kind: 'channel' }));
  expect(screen.getByLabelText('Channel name')).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  expect(fetcher.mock.calls[1]![0]).toBe(fetcher.mock.calls[0]![0]);
  expect(fetcher.mock.calls[1]![1].body).toBe(fetcher.mock.calls[0]![1].body);
  expect(fetcher.mock.calls[1]![1].headers).toEqual(fetcher.mock.calls[0]![1].headers);
  expect(JSON.parse(fetcher.mock.calls[1]![1].body)).toMatchObject({
    expected_version: 7,
    credentials: { channel_key: 'replacement-secret' },
  });
});
it('edits only the selected consumer policy and omits every server-owned consumer field', async () => {
  const source = fixture(),
    fetcher = vi.fn().mockResolvedValue(reply({ ...source, version: 8 }));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ChannelEditor
      channel={source}
      section={{ kind: 'consumer', id: 'reviewer' }}
      onDone={vi.fn()}
      denied={vi.fn()}
    />,
  );
  expect(screen.queryByLabelText('Channel name')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('Model')).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Reply display name'), {
    target: { value: 'New reviewer' },
  });
  await userEvent.click(screen.getByRole('button', { name: 'Save consumer policy' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(fetcher.mock.calls[0]![0]).toBe('/api/event-gateway/v1/channels/channel_fixture/routing');
  const body = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(body).toMatchObject({
    expected_version: 7,
    routing: {
      default_consumer: { name: 'default' },
      consumers: [
        { id: 'reviewer', name: 'reviewer', label: 'New reviewer', future_policy: { keep: true } },
      ],
    },
  });
  for (const key of [
    'profile',
    'profile_digest',
    'provider',
    'agent_key_hash',
    'pending',
    'created_at',
  ])
    expect(body.routing.consumers[0]).not.toHaveProperty(key);
});
it('does not offer changes to an existing immutable agent destination', () => {
  render(
    <ChannelEditor
      channel={fixture()}
      section={{ kind: 'consumer', id: 'default' }}
      onDone={vi.fn()}
      denied={vi.fn()}
    />,
  );
  expect(screen.getByText('Existing agent: agt_retained')).toBeVisible();
  expect(screen.queryByLabelText('Agent ID')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Model')).toHaveValue('gpt-6');
  expect(screen.queryByLabelText('Bot ID')).not.toBeInTheDocument();
});
it('creates a typed custom event schema without a configuration JSON editor', async () => {
  const source = fixture();
  source.definition.sources = [{ type: 'custom', auth: { type: 'hmac', key_id: 'events' } }];
  const fetcher = vi.fn().mockResolvedValue(reply({ ...source, version: 8 }));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ChannelEditor
      channel={source}
      section={{ kind: 'producer', index: 0 }}
      onDone={vi.fn()}
      denied={vi.fn()}
    />,
  );
  expect(screen.queryByLabelText(/configuration JSON/)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Add schema validation' }));
  fireEvent.change(screen.getByLabelText('properties field name'), { target: { value: 'topic' } });
  const field = screen.getByLabelText('properties field name');
  await userEvent.click(
    within(field.closest('fieldset')!).getByRole('button', { name: 'Add field' }),
  );
  fireEvent.change(screen.getByLabelText('topic'), { target: { value: 'math' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(JSON.parse(fetcher.mock.calls[0]![1].body).sources[0].schema).toEqual({
    type: 'object',
    properties: { topic: 'math' },
  });
});
it('retains unfamiliar routing settings while stripping immutable consumer identity', () => {
  const routing = object(fixture().definition.routing);
  routing.future_routing = { enabled: true };
  expect(routingRequest(routing)).toMatchObject({
    future_routing: { enabled: true },
    consumers: [{ id: 'reviewer', future_policy: { keep: true } }],
  });
});

it('appends a producer through its own node without altering existing sources', async () => {
  const source = fixture(),
    fetcher = vi.fn().mockResolvedValue(reply({ ...source, version: 8 }));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ChannelEditor
      channel={source}
      section={{ kind: 'producer', index: 2 }}
      onDone={vi.fn()}
      denied={vi.fn()}
    />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Producer 3 connection type' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Custom event' }));
  fireEvent.change(screen.getByLabelText('HMAC key ID'), { target: { value: 'events-key' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  const body = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(body.sources).toHaveLength(3);
  expect(body.sources[0]).toEqual((channelDraft(source).sources as unknown[])[0]);
  expect(body.sources[1]).toEqual(source.definition.sources[1]);
  expect(body.sources[2]).toEqual({ type: 'custom', auth: { type: 'hmac', key_id: 'events-key' } });
  expect(body.name).toBe(source.definition.name);
  expect(body).not.toHaveProperty('credentials');
});

it('keeps unsaved consumer policy separate from a central routing save and then uses the new revision', async () => {
  const source = fixture(),
    fetcher = vi.fn().mockImplementation(async (_url, init) => {
      const body = JSON.parse(init.body);
      return reply({
        ...source,
        version: body.expected_version + 1,
        definition: { ...source.definition, routing: body.routing },
      });
    });
  vi.stubGlobal('fetch', fetcher);
  const view = (section: EditorSection) => (
    <ChannelEditor channel={source} section={section} onDone={vi.fn()} denied={vi.fn()} />
  );
  const { rerender } = render(view({ kind: 'consumer', id: 'reviewer' }));
  fireEvent.change(screen.getByLabelText('Reply display name'), {
    target: { value: 'Unsaved reviewer' },
  });
  rerender(view({ kind: 'channel' }));
  fireEvent.change(screen.getByLabelText('Maximum consumers per event'), {
    target: { value: '2' },
  });
  await userEvent.click(screen.getByRole('button', { name: 'Save routing' }));
  await screen.findByText('Changes saved.');
  const first = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(first.routing.max_targets).toBe(2);
  expect(first.routing.consumers[0].label).toBe('Review');
  rerender(view({ kind: 'consumer', id: 'reviewer' }));
  expect(screen.getByLabelText('Reply display name')).toHaveValue('Unsaved reviewer');
  await userEvent.click(screen.getByRole('button', { name: 'Save consumer policy' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  const second = JSON.parse(fetcher.mock.calls[1]![1].body);
  expect(second.expected_version).toBe(8);
  expect(second.routing.max_targets).toBe(2);
  expect(second.routing.consumers[0].label).toBe('Unsaved reviewer');
});
