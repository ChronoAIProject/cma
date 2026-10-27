import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
import { LarkMessageEffectsFields } from './LarkMessageEffectsFields';
import { validateRelayAuthoring } from './relayAuthoring';

const source = {
  type: 'nyxid_relay',
  platform: 'lark',
  bot_id: 'ou_source',
  admission: { type: 'all' },
};
const effects = {
  connection: { user_service_id: 'one', catalog_service_id: 'catalog', app_id: 'cli_source' },
  delete: true,
  reaction: false,
};
const available: RelayDiscovery = {
  status: 'ready',
  authoring: {
    lark_message_effects_writes_enabled: true,
    lark_resource_writes_enabled: true,
    dynamic_sources_writes_enabled: false,
    multi_consumer_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [
      {
        id: 'lark',
        label: 'Lark',
        verification_modes: [{ id: 'native_event', writes_enabled: true }],
      },
    ],
  },
};
it('requires its separate authoring capability and preserves resource read authority', () => {
  const onChange = vi.fn();
  const value = { ...source, lark_resources: effects.connection };
  const view = render(<LarkMessageEffectsFields value={value} onChange={onChange} />);
  expect(screen.queryByLabelText('Allow message effects')).not.toBeInTheDocument();
  view.rerender(
    <RelayCapabilitiesContext value={available}>
      <LarkMessageEffectsFields value={value} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByLabelText('Allow message effects'));
  expect(onChange.mock.lastCall?.[0]).toMatchObject({
    lark_resources: effects.connection,
    lark_message_effects: { delete: true, reaction: true },
  });
});
it('allows removal but refuses changing saved writes when capability is unavailable', () => {
  const value = { ...source, lark_message_effects: effects };
  const onChange = vi.fn();
  render(<LarkMessageEffectsFields value={value} onChange={onChange} />);
  expect(screen.getByLabelText('Delete sent messages')).toBeDisabled();
  fireEvent.click(screen.getByLabelText('Allow message effects'));
  expect(onChange).toHaveBeenCalledWith(source);
  expect(() =>
    validateRelayAuthoring({ status: 'ready' }, { sources: [value] }, { sources: [value] }),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring(
      { status: 'ready' },
      { sources: [{ ...value, lark_message_effects: { ...effects, reaction: true } }] },
      { sources: [value] },
    ),
  ).toThrow('message effects');
});
it('requires an operation and an explicit connection before storing a new binding', () => {
  expect(() =>
    validateRelayAuthoring(available, {
      sources: [{ ...source, lark_message_effects: { ...effects, delete: false } }],
    }),
  ).toThrow('at least one');
  expect(() =>
    validateRelayAuthoring(available, {
      sources: [{ ...source, lark_message_effects: { ...effects, connection: {} } }],
    }),
  ).toThrow('connection');
});
