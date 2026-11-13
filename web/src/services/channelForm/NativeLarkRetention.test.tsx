import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type EditorSection } from '../ChannelEditor';
import { ChannelMap } from '../ChannelMap';
import { NodeInspector } from '../NodeInspector';
import { RelayCapabilitiesContext } from '../RelayCapabilities';
import { SourceFields } from './SourceFields';
import { defaultSource } from './model';
import { validateRelayAuthoring } from './relayAuthoring';
import {
  available,
  identity,
  source,
  bundle,
  channel,
  enter,
  enterBundle,
  response,
} from './NativeLarkSourceFields.fixtures';
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it.each([false, true])(
  'retains an unrelated %s complete draft while another producer saves with gates off',
  async (complete) => {
    const mixed = {
      ...channel,
      definition: {
        ...channel.definition,
        sources: [source, { ...defaultSource('telegram'), bot_id: '321' }],
      },
    };
    const fetcher = vi.fn().mockImplementation(async () => response(mixed));
    vi.stubGlobal('fetch', fetcher);
    const view = (section: EditorSection, discovery = available) => (
      <RelayCapabilitiesContext value={discovery}>
        <ChannelEditor channel={mixed} section={section} onDone={vi.fn()} denied={vi.fn()} />
      </RelayCapabilitiesContext>
    );
    const { rerender } = render(view({ kind: 'producer', index: 0 }));
    if (complete) enterBundle();
    else enter('Encrypt Key', bundle.encrypt_key);
    enter('Tenant key', 'unsaved_tenant');
    rerender(view({ kind: 'producer', index: 1 }, { status: 'legacy' }));
    enter('Telegram bot token', 'telegram-private');
    fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
    await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
    expect(JSON.parse(fetcher.mock.calls[0]![1].body)).toMatchObject({
      sources: mixed.definition.sources,
      credentials: { telegram_bot_token: 'telegram-private' },
    });
    await waitFor(() => expect(screen.getByLabelText('Telegram bot token')).toHaveValue(''));
    rerender(view({ kind: 'producer', index: 0 }));
    expect(screen.getByLabelText('Encrypt Key')).toHaveValue(bundle.encrypt_key);
    expect(screen.getByLabelText('Tenant key')).toHaveValue('unsaved_tenant');
  },
);
it('edits channel metadata with disabled native writes while keeping saved source and credentials', async () => {
  const fetcher = vi.fn().mockImplementation(async () => response());
  vi.stubGlobal('fetch', fetcher);
  render(
    <RelayCapabilitiesContext value={{ status: 'legacy' }}>
      <ChannelEditor
        channel={channel}
        section={{ kind: 'channel' }}
        onDone={vi.fn()}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>,
  );
  enter('Channel name', 'Renamed channel');
  fireEvent.click(screen.getByRole('button', { name: 'Save channel' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  const body = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(body.name).toBe('Renamed channel');
  expect(body.sources).toEqual([source]);
  expect(body.credentials).toBeUndefined();
});
it('creates an exact separate app without submitting hidden relay credentials', async () => {
  const fetcher = vi.fn().mockImplementation(async () => response());
  vi.stubGlobal('fetch', fetcher);
  render(
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor ownerSubject="owner" onDone={vi.fn()} denied={vi.fn()} />
    </RelayCapabilitiesContext>,
  );
  enter('Channel name', 'New app');
  enter('Provider agent key', 'provider-private');
  enter('NyxID channel key', 'hidden-relay-private');
  fireEvent.click(screen.getByRole('button', { name: 'Producer 1 connection type' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: /^Direct Lark \/ Feishu app/ }));
  enter('App ID', identity.app_id);
  enter('Tenant key', identity.tenant_key);
  enter('Bot open ID', identity.bot_open_id);
  fireEvent.click(screen.getByRole('checkbox', { name: /This is a separate app/ }));
  fireEvent.click(screen.getByRole('button', { name: 'Add Allowed sender IDs' }));
  enter('Allowed sender IDs 1', 'ou_allowed_human');
  enterBundle();
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  const body = JSON.parse(fetcher.mock.calls[0]![1].body);
  expect(body.sources).toEqual([{ ...source, identity: { ...identity, region: 'lark' } }]);
  expect(body.credentials).toEqual({ agent_key: 'provider-private', native_lark: bundle });
  expect(body.record_id).toMatch(/^[0-9a-f-]{36}$/);
});
it.each(['lark', 'feishu'])(
  'uses the %s icon and native label and copies only its endpoint',
  async (region) => {
    const fetcher = vi.fn();
    vi.stubGlobal('fetch', fetcher);
    const copy = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: copy },
    });
    const platform = region === 'lark' ? 'Lark' : 'Feishu';
    const current = {
      ...channel,
      definition: {
        ...channel.definition,
        sources: [
          { ...source, identity: { ...identity, region } },
          { type: 'nyxid_relay', platform: region, route_ids: ['saved-route'] },
        ],
      },
    };
    render(
      <>
        <ChannelMap
          channel={current}
          selection={{ kind: 'producer', index: 0 }}
          onSelect={vi.fn()}
          inspectorId="lark-inspector"
        />
        <NodeInspector
          channel={current}
          selection={{ kind: 'producer', index: 0 }}
          id="lark-inspector"
        >
          <span>Settings</span>
        </NodeInspector>
      </>,
    );
    const direct = await screen.findByRole('button', {
      name: `Producer: ${platform} (Direct ${platform} app)`,
    });
    expect(direct.querySelector(`[data-identity="${region}"] svg`)).not.toBeNull();
    expect(
      screen.getByRole('button', { name: `Producer: ${platform} (NyxID relay)` }),
    ).toBeVisible();
    const inspector = screen.getByRole('region', { name: platform });
    expect(within(inspector).getByText(identity.tenant_key)).toBeVisible();
    expect(
      within(inspector).queryByText(channel.endpoints.nyxid_callback_url),
    ).not.toBeInTheDocument();
    fireEvent.click(within(inspector).getByRole('button', { name: 'Copy Lark / Feishu webhook' }));
    await waitFor(() =>
      expect(copy).toHaveBeenCalledWith(channel.endpoints.native_lark_webhook_url),
    );
    expect(fetcher).not.toHaveBeenCalled();
  },
);

it('retains uncertain native rotation bytes across selection changes and retries the same request', async () => {
  const mixed = {
    ...channel,
    definition: {
      ...channel.definition,
      sources: [source, { ...defaultSource('telegram'), bot_id: '321' }],
    },
  };
  const fetcher = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Connection interrupted'))
    .mockImplementation(async () => response(mixed));
  vi.stubGlobal('fetch', fetcher);
  const view = (section: EditorSection) => (
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor channel={mixed} section={section} onDone={vi.fn()} denied={vi.fn()} />
    </RelayCapabilitiesContext>
  );
  const { rerender } = render(view({ kind: 'producer', index: 1 }));
  enter('Telegram bot token', 'unsaved-telegram-private');
  rerender(view({ kind: 'producer', index: 0 }));
  enterBundle();
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await screen.findByRole('button', { name: 'Retry same request' });
  expect(screen.getByLabelText('Encrypt Key')).toHaveValue(bundle.encrypt_key);
  rerender(view({ kind: 'producer', index: 1 }));
  expect(screen.getByLabelText('Telegram bot token')).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await waitFor(() => expect(screen.getByLabelText('Telegram bot token')).toBeEnabled());
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(fetcher.mock.calls[1]![1].body).toBe(fetcher.mock.calls[0]![1].body);
  expect(fetcher.mock.calls[1]![1].headers).toEqual(fetcher.mock.calls[0]![1].headers);
  expect(screen.getByLabelText('Telegram bot token')).toHaveValue('unsaved-telegram-private');
  rerender(view({ kind: 'producer', index: 0 }));
  expect(screen.getByLabelText('Encrypt Key')).toHaveValue('');
});

it('offers explicit native history consent and permits revocation while authoring is disabled', () => {
  const onChange = vi.fn();
  const enabled = {
    ...available,
    authoring: { ...available.authoring!, native_lark_conversation_history_enabled: true },
  };
  const view = render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={source} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  expect(
    screen.queryByRole('checkbox', { name: 'Allow original conversation history' }),
  ).not.toBeInTheDocument();
  view.rerender(
    <RelayCapabilitiesContext value={enabled}>
      <SourceFields value={source} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('checkbox', { name: 'Allow original conversation history' }));
  const consented = { ...source, conversation_history: true };
  expect(onChange).toHaveBeenLastCalledWith(consented);
  expect(screen.getByText(/Thread reads may need several empty scan pages/)).toBeInTheDocument();
  expect(() =>
    validateRelayAuthoring(available, { sources: [consented] }, { sources: [source] }),
  ).toThrow('history consent');
  expect(() =>
    validateRelayAuthoring(available, { sources: [consented] }, { sources: [consented] }),
  ).not.toThrow();
  const paused = {
    ...available,
    authoring: { ...available.authoring!, native_lark_writes_enabled: false },
  };
  view.rerender(
    <RelayCapabilitiesContext value={paused}>
      <SourceFields value={consented} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('checkbox', { name: 'Allow original conversation history' }));
  expect(onChange).toHaveBeenLastCalledWith(source);
  expect(() =>
    validateRelayAuthoring(paused, { sources: [source] }, { sources: [consented] }),
  ).not.toThrow();
});
