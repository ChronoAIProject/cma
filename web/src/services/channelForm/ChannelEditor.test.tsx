import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel, type EditorSection } from '../ChannelEditor';
import { channelDraft, channelRequest, defaultProfile, object } from './model';
import { routingRequest } from './RoutingFields';
const reply = (v: unknown) => new Response(JSON.stringify(v));
afterEach(() => vi.unstubAllGlobals());
it.each([undefined, '', 'Connection_Retained.Exact-123'])(
  'keeps the saved connection %s read-only and exact in unrelated saves',
  async (connection) => {
    const channel = fixture();
    if (connection !== undefined) channel.definition.connection_ref = connection;
    const done = vi.fn();
    const fetcher = vi.fn().mockResolvedValue(reply({ ...channel, version: 8 }));
    vi.stubGlobal('fetch', fetcher);
    render(<ChannelEditor channel={channel} onDone={done} denied={vi.fn()} />);
    const reference = screen.getByRole('textbox', { name: 'Connection reference' });
    expect(reference).toHaveAttribute('readonly');
    expect(reference).toHaveValue(connection || 'No shared connection');
    const user = userEvent.setup();
    await user.type(reference, 'replacement');
    await user.keyboard('{Control>}a{/Control}{Backspace}');
    expect(reference).toHaveValue(connection || 'No shared connection');
    fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'Renamed' } });
    expect(fetcher).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Save channel' }));
    await waitFor(() => expect(done).toHaveBeenCalledOnce());
    expect(fetcher).toHaveBeenCalledOnce();
    const [path, request] = fetcher.mock.calls[0]!;
    expect(path).toBe('/api/event-gateway/v1/channels/channel_fixture');
    expect(request.method).toBe('PUT');
    const body = JSON.parse(request.body);
    expect(body).toMatchObject({ name: 'Renamed', expected_version: 7 });
    if (connection === undefined) expect(body).not.toHaveProperty('connection_ref');
    else expect(body.connection_ref).toBe(connection);
    expect(body).not.toHaveProperty('credentials');
    expect(body.sources).toEqual(channelRequest(channelDraft(channel), {}, channel).sources);
  },
);
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
it('includes an environment by default only when creating a profile-managed CMA channel', () => {
  const draft = channelDraft(undefined, 'owner');
  const credentials = { agent_key: 'fixture-provider' };
  const profile = object(draft.profile);
  const provider = object(draft.provider);
  const defaultEnvironment = {
    size: { cpu_cores: 1, memory_gib: 2 },
    workspace: { type: 'directory', name: 'workspace' },
  };
  expect(channelRequest(draft, credentials)).toHaveProperty(
    'profile.environment',
    defaultEnvironment,
  );
  const managed = { ...draft, provider: { ...provider, target: { type: 'profile' } } };
  expect(channelRequest(managed, credentials)).toHaveProperty(
    'profile.environment',
    defaultEnvironment,
  );
  const environment = { ...defaultEnvironment, size: { cpu_cores: 2, memory_gib: 4 } };
  expect(
    channelRequest({ ...managed, profile: { ...profile, environment } }, credentials),
  ).toHaveProperty('profile.environment', environment);
  const existing = { ...draft, provider: { ...provider, target: { type: 'existing_agent' } } };
  const external = { ...draft, provider: { ...provider, slug: 'external' } };
  expect(channelRequest(existing, credentials)).not.toHaveProperty('profile.environment');
  expect(channelRequest(external, credentials)).not.toHaveProperty('profile.environment');
  expect(channelRequest(managed, {}, fixture())).not.toHaveProperty('profile.environment');
});
it('preserves unknown writable fields and exact existing bindings while excluding server-only fields and unchanged credentials', () => {
  const source = fixture();
  source.definition.connection_ref = 'connection_retained';
  const draft = channelDraft(source);
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
  expect(screen.getByLabelText('Maximum wait (seconds)')).toHaveValue(300);
});
it('saves a queue policy while preserving the existing Agent destination and profile fields', async () => {
  const channel = fixture();
  const fetcher = vi.fn().mockResolvedValue(reply({ ...channel, version: 8 }));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ChannelEditor
      channel={channel}
      section={{ kind: 'consumer', id: 'default' }}
      onDone={vi.fn()}
      denied={vi.fn()}
    />,
  );
  fireEvent.change(screen.getByLabelText('Maximum wait (seconds)'), { target: { value: '86400' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save agent settings' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalled());
  const sent = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(sent.profile.queue_lifetime_seconds).toBe(86400);
  expect(sent.profile.future_profile).toEqual({ retained: true });
  expect(sent.provider.target).toEqual({ type: 'existing_agent', agent_id: 'agt_retained' });
});
it('offers neither a configuration JSON editor nor a Custom event schema the v1 gateway refuses', () => {
  const source = fixture();
  source.definition.sources = [{ type: 'custom', auth: { type: 'hmac', key_id: 'events' } }];
  vi.stubGlobal('fetch', vi.fn());
  render(
    <ChannelEditor
      channel={source}
      section={{ kind: 'producer', index: 0 }}
      onDone={vi.fn()}
      denied={vi.fn()}
    />,
  );
  expect(screen.queryByLabelText(/configuration JSON/)).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Add schema validation' })).not.toBeInTheDocument();
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
  // A Custom producer needs a grouping the gateway accepts for custom events.
  source.definition.partition = { version: 1, key: 'fixed' };
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

it.each([
  ['retained-private-service', 'personal-custom-service'],
  ['aa480b39-3ba0-42af-9c21-10d02885337c', '106f2888-6bb9-4ff1-a8d8-249af96c06ac'],
])(
  'shows actionable scope refusal for %s with unchanged saves before and after',
  async (retained, added) => {
    const source = fixture();
    source.definition.provider = { slug: 'cma', binding_id: 'binding_original' };
    source.definition.partition = { version: 1, key: 'per_event' };
    const scope = ['chrono-llm-public', 'ornn-api', retained];
    source.definition.profile = {
      ...object(source.definition.profile),
      permission: { action_mode: 'autonomous', service_scope: scope },
    };
    const saved = { ...source, version: 8 };
    const detail =
      "This channel's existing agent configuration is pinned. Changing its service scope, skills, prompt, model, environment, or conversation affinity through a channel update is not supported. To use a different service scope, create a new binding with the required scope. The provider binding, credentials, and retained work have not been changed. Keep the current profile to save other channel settings, and rotate credentials separately using that profile. Retrying with the same key will not apply this change.";
    const fetcher = vi.fn<typeof fetch>().mockImplementation(async (_url, init) => {
      const body = JSON.parse(String(init?.body));
      return body.profile.permission.service_scope.includes(added)
        ? new Response(JSON.stringify({ status: 409, code: 'provider_rejected', detail }), {
            status: 409,
            headers: { 'Content-Type': 'application/problem+json' },
          })
        : reply(saved);
    });
    vi.stubGlobal('fetch', fetcher);
    const done = vi.fn();
    const view = (channel: Channel) => (
      <ChannelEditor
        channel={channel}
        section={{ kind: 'consumer', id: 'default' }}
        onDone={done}
        denied={vi.fn()}
      />
    );
    const editor = render(view(source));
    await userEvent.click(screen.getByRole('button', { name: 'Save agent settings' }));
    await screen.findByText('Changes saved.');
    await userEvent.click(screen.getByRole('button', { name: 'Add Service scope' }));
    fireEvent.change(screen.getByLabelText('Service scope 4'), { target: { value: added } });
    await userEvent.click(screen.getByRole('button', { name: 'Save agent settings' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(detail);
    expect(done).toHaveBeenCalledTimes(1);
    expect(JSON.parse(String(fetcher.mock.calls[1]?.[1]?.body))).toMatchObject({
      expected_version: 8,
      profile: { permission: { service_scope: [...scope, added] } },
    });

    editor.unmount();
    render(view(saved));
    await userEvent.click(screen.getByRole('button', { name: 'Save agent settings' }));
    await screen.findByText('Changes saved.');
    expect(done).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(fetcher.mock.calls.map(([url]) => url)).toEqual(
      Array(3).fill('/api/event-gateway/v1/channels/channel_fixture'),
    );
    for (const index of [0, 2]) {
      expect(fetcher.mock.calls[index]?.[1]?.method).toBe('PUT');
      expect(
        JSON.parse(String(fetcher.mock.calls[index]?.[1]?.body)).profile.permission.service_scope,
      ).toEqual(scope);
    }
  },
);
