import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel } from '../ChannelEditor';
import { ChannelMap } from '../ChannelMap';
import {
  RelayCapabilitiesContext,
  nativeDiscordUnavailable,
  type RelayDiscovery,
} from '../RelayCapabilities';
import { NodeFields, nodeCredentials } from './NodeFields';
import { SourceFields } from './SourceFields';
import { channelRequest, credentialsPatch, defaultSource } from './model';
import { validateRelayAuthoring } from './relayAuthoring';

const available: RelayDiscovery = {
  status: 'ready',
  authoring: {
    native_discord_writes_enabled: true,
    native_discord_auto_activation_enabled: true,
    native_discord_capacity: 64,
    multi_consumer_writes_enabled: true,
    dynamic_sources_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  },
};
const native = { ...defaultSource('discord'), bot_id: '77' };
const channel: Channel = {
  channel_id: 'owned',
  record_id: '11111111-1111-4111-8111-111111111111',
  version: 2,
  provider: { slug: 'cma_codex', level: 'l3' },
  definition: { name: 'Direct', sources: [native] },
  endpoints: { custom_event_url: '', nyxid_callback_url: '' },
};
afterEach(() => vi.unstubAllGlobals());
it('requires explicit native authoring and automatic activation confirmation for new channels', () => {
  expect(nativeDiscordUnavailable({ status: 'legacy' })).toContain('not enabled');
  const canary = {
    ...available,
    authoring: { ...available.authoring!, native_discord_auto_activation_enabled: false },
  };
  expect(() =>
    validateRelayAuthoring(canary, { sources: [native] }, undefined, {
      discord_bot_token: 'private',
    }),
  ).toThrow('automatic activation');
  expect(() =>
    validateRelayAuthoring(available, { sources: [native] }, undefined, {
      discord_bot_token: 'private',
    }),
  ).not.toThrow();
  expect(() => validateRelayAuthoring(available, { sources: [native] })).toThrow(
    'Enter the Discord bot token',
  );
});
it('retains exact saved sources and blank credentials when authoring is disabled', () => {
  expect(() =>
    validateRelayAuthoring(
      { status: 'legacy' },
      { sources: [native] },
      { sources: [native] },
      { discord_bot_token: '' },
    ),
  ).not.toThrow();
  expect(credentialsPatch({ discord_bot_token: '' }, false)).toBeUndefined();
  expect(() =>
    validateRelayAuthoring(
      { status: 'legacy' },
      { sources: [native] },
      { sources: [native] },
      { discord_bot_token: 'new-token' },
    ),
  ).toThrow('not enabled');
  expect(
    nodeCredentials(
      { discord_bot_token: 'new-token', agent_key: 'private-agent' },
      { kind: 'producer', index: 0 },
      { sources: [native] },
    ),
  ).toEqual({ discord_bot_token: 'new-token' });
  expect(
    channelRequest(
      { sources: [native] },
      { agent_key: 'private-agent', discord_bot_token: 'private-bot' },
      undefined,
      channel.record_id,
    ),
  ).toMatchObject({
    record_id: channel.record_id,
    credentials: { discord_bot_token: 'private-bot' },
  });
  expect(
    channelRequest(
      { sources: [{ ...defaultSource('custom'), auth: { type: 'nyxid_key', key_hashes: [] } }] },
      { agent_key: 'private-agent', discord_bot_token: 'stale-bot' },
      undefined,
      channel.record_id,
    ),
  ).toMatchObject({ credentials: { agent_key: 'private-agent' } });
  expect(
    JSON.stringify(
      channelRequest(
        { sources: [{ ...defaultSource('custom'), auth: { type: 'nyxid_key', key_hashes: [] } }] },
        { agent_key: 'private-agent', discord_bot_token: 'stale-bot' },
        undefined,
        channel.record_id,
      ),
    ),
  ).not.toContain('stale-bot');
});
it('renders Discord identity, supported intents and admission without relay verification controls', () => {
  const onChange = vi.fn();
  render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={native} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  expect(screen.getByLabelText('Discord bot user ID')).toHaveValue('77');
  expect(screen.queryByLabelText('NyxID issuer')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('Messages from bots')).not.toBeInTheDocument();
  fireEvent.click(screen.getByLabelText('Receive direct messages'));
  expect(onChange).toHaveBeenLastCalledWith({ ...native, intents: 33281 });
});
it('exposes a direct Discord producer choice separately from NyxID and a password-only token input', () => {
  render(
    <RelayCapabilitiesContext value={available}>
      <NodeFields
        draft={{ sources: [native] }}
        onChange={vi.fn()}
        credentials={{}}
        onCredentials={vi.fn()}
        section={{ kind: 'channel' }}
        create
        sourceCount={0}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>,
  );
  const token = screen.getByLabelText('Discord bot token');
  expect(token).toHaveAttribute('type', 'password');
  expect(token).toHaveValue('');
  fireEvent.click(screen.getByRole('button', { name: 'Producer 1 connection type' }));
  expect(screen.getByRole('menuitemradio', { name: 'Direct Discord bot' })).toBeInTheDocument();
  expect(
    screen.getByRole('menuitemradio', { name: 'NyxID messaging connection' }),
  ).toBeInTheDocument();
});
it('sends token replacement once, clears it after success, and displays capacity refusal', async () => {
  const saved = {
    ...channel,
    version: 3,
    native_discord_activation: {
      state: 'capacity_exceeded',
      detail:
        'Native Discord activation capacity is full. This channel is saved but is not receiving native messages.',
      capacity: 64,
      selected_records: 64,
    },
  };
  const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(saved)));
  vi.stubGlobal('fetch', fetcher);
  render(
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor
        channel={channel}
        section={{ kind: 'producer', index: 0 }}
        onDone={vi.fn()}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>,
  );
  fireEvent.change(screen.getByLabelText('Discord bot token'), {
    target: { value: 'replacement-private-token' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(1));
  expect(JSON.parse(fetcher.mock.calls[0]![1].body)).toMatchObject({
    credentials: { discord_bot_token: 'replacement-private-token' },
  });
  await screen.findByText(/Changes saved.*capacity is full/);
  expect(screen.getByLabelText('Discord bot token')).toHaveValue('');
  expect(screen.queryByText('replacement-private-token')).not.toBeInTheDocument();
});

it('keeps activation refusal visible on the delivery map after the create editor closes', () => {
  render(
    <ChannelMap
      channel={{
        ...channel,
        native_discord_activation: {
          state: 'capacity_exceeded',
          detail: 'Native Discord activation capacity is full.',
          capacity: 64,
          selected_records: 64,
        },
      }}
      selection={{ kind: 'channel' }}
      onSelect={vi.fn()}
      inspectorId="inspector"
    />,
  );
  expect(screen.getByRole('status', { name: 'Native Discord activation' })).toHaveTextContent(
    'Native Discord activation capacity is full.',
  );
});

it('exposes bounded thread history only after capability confirmation', () => {
  const onChange = vi.fn();
  const enabled = {
    ...available,
    authoring: { ...available.authoring!, native_discord_thread_history_enabled: true },
  };
  const { rerender } = render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={native} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  expect(
    screen.queryByRole('checkbox', { name: 'Read earlier thread messages' }),
  ).not.toBeInTheDocument();
  rerender(
    <RelayCapabilitiesContext value={enabled}>
      <SourceFields value={native} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('checkbox', { name: 'Read earlier thread messages' }));
  expect(onChange).toHaveBeenLastCalledWith({ ...native, thread_history: true });
  rerender(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={{ ...native, thread_history: true }} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('checkbox', { name: 'Read earlier thread messages' }));
  expect(onChange).toHaveBeenLastCalledWith(native);
});

it('retains saved thread history access and blocks a new opt-in when the gate is off', () => {
  const source = { ...native, thread_history: true };
  expect(() =>
    validateRelayAuthoring(available, { sources: [source] }, { sources: [native] }),
  ).toThrow('thread history access');
  expect(() =>
    validateRelayAuthoring(available, { sources: [source] }, { sources: [source] }),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring(available, { sources: [native] }, { sources: [source] }),
  ).not.toThrow();
});

it('reaction opt-in retains legacy defaults and follows the selected message scopes', () => {
  const onChange = vi.fn();
  const enabled = {
    ...available,
    authoring: { ...available.authoring!, native_discord_reactions_enabled: true },
  };
  const { rerender } = render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={native} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  expect(
    screen.queryByRole('checkbox', { name: 'Receive reactions to retained messages' }),
  ).not.toBeInTheDocument();
  expect(native).toMatchObject({ intents: 37377 });
  rerender(
    <RelayCapabilitiesContext value={enabled}>
      <SourceFields value={native} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('checkbox', { name: 'Receive reactions to retained messages' }));
  const opted = { ...native, intents: 37377 | 1024 | 8192 };
  expect(onChange).toHaveBeenLastCalledWith(opted);
  rerender(
    <RelayCapabilitiesContext value={enabled}>
      <SourceFields value={opted} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('checkbox', { name: 'Receive server messages' }));
  expect(onChange).toHaveBeenLastCalledWith({ ...native, intents: 32768 | 4096 | 8192 });
  expect(() =>
    validateRelayAuthoring(available, { sources: [opted] }, { sources: [native] }),
  ).toThrow('reaction access');
  expect(() =>
    validateRelayAuthoring(available, { sources: [opted] }, { sources: [opted] }),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring(available, { sources: [native] }, { sources: [opted] }),
  ).not.toThrow();
});

it('offers independent thread lifecycle consent only when advertised and permits removing retained consent', () => {
  const onChange = vi.fn();
  const enabled = {
    ...available,
    authoring: { ...available.authoring!, native_discord_thread_lifecycle_enabled: true },
  };
  const { rerender } = render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={native} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  expect(
    screen.queryByRole('checkbox', { name: 'Manage threads from incoming messages' }),
  ).not.toBeInTheDocument();
  rerender(
    <RelayCapabilitiesContext value={enabled}>
      <SourceFields value={native} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('checkbox', { name: 'Manage threads from incoming messages' }));
  expect(onChange).toHaveBeenLastCalledWith({ ...native, thread_lifecycle: true });
  expect(
    screen.queryByRole('checkbox', { name: 'Read earlier thread messages' }),
  ).not.toBeInTheDocument();
  const retained = { ...native, thread_lifecycle: true };
  expect(() =>
    validateRelayAuthoring(available, { sources: [retained] }, { sources: [native] }),
  ).toThrow('thread management access');
  expect(() =>
    validateRelayAuthoring(available, { sources: [retained] }, { sources: [retained] }),
  ).not.toThrow();
  rerender(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={retained} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('checkbox', { name: 'Manage threads from incoming messages' }));
  expect(onChange).toHaveBeenLastCalledWith(native);
  expect(() =>
    validateRelayAuthoring(available, { sources: [native] }, { sources: [retained] }),
  ).not.toThrow();
});

it('offers independent conversation history consent only when advertised and permits removing retained consent', () => {
  const onChange = vi.fn();
  const enabled = {
    ...available,
    authoring: { ...available.authoring!, native_discord_conversation_history_enabled: true },
  };
  const { rerender } = render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={native} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  expect(
    screen.queryByRole('checkbox', { name: 'Read earlier conversation messages' }),
  ).not.toBeInTheDocument();
  rerender(
    <RelayCapabilitiesContext value={enabled}>
      <SourceFields value={native} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('checkbox', { name: 'Read earlier conversation messages' }));
  expect(onChange).toHaveBeenLastCalledWith({ ...native, conversation_history: true });
  expect(
    screen.queryByRole('checkbox', { name: 'Read earlier thread messages' }),
  ).not.toBeInTheDocument();
  const retained = { ...native, conversation_history: true };
  expect(() =>
    validateRelayAuthoring(available, { sources: [retained] }, { sources: [native] }),
  ).toThrow('conversation history access');
  expect(() =>
    validateRelayAuthoring(available, { sources: [retained] }, { sources: [retained] }),
  ).not.toThrow();
  rerender(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={retained} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('checkbox', { name: 'Read earlier conversation messages' }));
  expect(onChange).toHaveBeenLastCalledWith(native);
  expect(() =>
    validateRelayAuthoring(available, { sources: [native] }, { sources: [retained] }),
  ).not.toThrow();
});
