import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
import { validateRelayAuthoring } from './relayAuthoring';
import { TelegramMemberJoinFields } from './TelegramMemberJoinFields';

const source = {
  type: 'telegram',
  bot_id: '77',
  message_events: true,
  admission: {
    type: 'scoped',
    senders: { type: 'open' },
    chats: { type: 'allowlist', ids: ['-100', '-101', '42'] },
    groups: 'all',
  },
};
const available: RelayDiscovery = {
  status: 'ready',
  authoring: {
    native_telegram_message_events_enabled: true,
    native_telegram_member_joins_enabled: true,
    dynamic_sources_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    multi_consumer_writes_enabled: true,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  },
};
const legacy: RelayDiscovery = { status: 'legacy' };
const opted = { ...source, member_join_chat_ids: ['-100'] };

it('offers only explicitly allowed groups after discovery and allows clearing a saved opt-in', () => {
  const onChange = vi.fn();
  const view = render(<TelegramMemberJoinFields value={source} onChange={onChange} />);
  expect(screen.queryByLabelText('Welcome new members in -100')).toBeNull();

  view.rerender(
    <RelayCapabilitiesContext value={available}>
      <TelegramMemberJoinFields value={source} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  expect(screen.queryByLabelText('Welcome new members in 42')).toBeNull();
  fireEvent.click(screen.getByLabelText('Welcome new members in -100'));
  expect(onChange).toHaveBeenLastCalledWith(opted);

  view.rerender(<TelegramMemberJoinFields value={opted} onChange={onChange} />);
  expect(screen.queryByLabelText('Welcome new members in -101')).toBeNull();
  expect(screen.getByLabelText('Welcome new members in -100')).toBeChecked();
  fireEvent.click(screen.getByLabelText('Welcome new members in -100'));
  expect(onChange).toHaveBeenLastCalledWith({ ...source, member_join_chat_ids: [] });
});

it('retains a saved opt-in, refuses gated expansion, and checks group admission', () => {
  expect(() => validateRelayAuthoring(available, { sources: [opted] })).not.toThrow();
  expect(() =>
    validateRelayAuthoring(legacy, { sources: [opted] }, { sources: [opted] }),
  ).not.toThrow();
  expect(() => validateRelayAuthoring(legacy, { sources: [opted] }, { sources: [source] })).toThrow(
    /disabled/,
  );
  for (const ids of [['-100', '-100'], ['-200'], ['42']]) {
    expect(() =>
      validateRelayAuthoring(available, { sources: [{ ...source, member_join_chat_ids: ids }] }),
    ).toThrow(/exact group IDs/);
  }
});

it('gates a verified Telegram relay independently and retains only the same relay identity', () => {
  const relay = {
    ...source,
    type: 'nyxid_relay',
    platform: 'telegram',
    issuer: 'https://nyx.example',
    key_id: 'relay-key',
    route_ids: ['route-1'],
    member_join_chat_ids: ['-100'],
  };
  const relayAvailable: RelayDiscovery = {
    ...available,
    authoring: {
      ...available.authoring!,
      relay_telegram_member_joins_enabled: true,
      platforms: [
        {
          id: 'telegram',
          label: 'Telegram',
          verification_modes: [{ id: 'native_event', writes_enabled: true }],
        },
      ],
    },
  };
  expect(() => validateRelayAuthoring(available, { sources: [relay] })).toThrow(/disabled/);
  expect(() => validateRelayAuthoring(relayAvailable, { sources: [relay] })).not.toThrow();
  expect(() =>
    validateRelayAuthoring(legacy, { sources: [relay] }, { sources: [relay] }),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring(
      legacy,
      { sources: [{ ...relay, route_ids: ['new-route'] }] },
      { sources: [relay] },
    ),
  ).toThrow(/disabled/);
  expect(() =>
    validateRelayAuthoring(relayAvailable, { sources: [{ ...relay, bot_id: undefined }] }),
  ).toThrow(/bot ID/);
});
