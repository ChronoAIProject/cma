import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
import { SourceFields } from './SourceFields';
import { validateRelayAuthoring } from './relayAuthoring';

const binding = {
  user_service_id: 'connection-one',
  catalog_service_id: 'catalog-one',
  app_id: 'cli_source',
};
const source = {
  type: 'nyxid_relay',
  platform: 'lark',
  issuer: 'https://nyx.example',
  key_id: 'key-one',
  route_ids: ['route-one'],
  bot_id: 'ou_receiver',
  admission: { type: 'all' },
};
const available: RelayDiscovery = {
  status: 'ready',
  authoring: {
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
      {
        id: 'feishu',
        label: 'Feishu',
        verification_modes: [{ id: 'native_event', writes_enabled: true }],
      },
    ],
  },
};

it('does not offer an opt-in without explicit capability confirmation', () => {
  render(<SourceFields value={source} onChange={vi.fn()} />);
  expect(screen.queryByText('Read media in rich-text posts')).not.toBeInTheDocument();
});
it('enables only explicit non-secret binding fields and retains other source settings', () => {
  const onChange = vi.fn();
  const view = render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={source} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByLabelText('Read media in rich-text posts'));
  expect(onChange).toHaveBeenLastCalledWith({
    ...source,
    lark_resources: { user_service_id: '', catalog_service_id: '', app_id: '' },
  });
  view.rerender(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={{ ...source, lark_resources: binding }} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.change(screen.getByLabelText('NyxID service connection ID'), {
    target: { value: 'connection-two' },
  });
  expect(onChange).toHaveBeenLastCalledWith({
    ...source,
    lark_resources: { ...binding, user_service_id: 'connection-two' },
  });
  expect(screen.getByLabelText('Receiving app ID')).toHaveAttribute('type', 'text');
  expect(screen.getByText(/independently confirmed open_id/)).toBeVisible();
});
it('retains saved bindings read-only on older gateways while allowing removal', () => {
  const onChange = vi.fn();
  render(<SourceFields value={{ ...source, lark_resources: binding }} onChange={onChange} />);
  expect(screen.getByLabelText('NyxID service connection ID')).toHaveValue('connection-one');
  expect(screen.getByLabelText('NyxID service connection ID')).toBeDisabled();
  expect(onChange).not.toHaveBeenCalled();
  fireEvent.click(screen.getByLabelText('Read media in rich-text posts'));
  expect(onChange).toHaveBeenCalledWith(source);
});
it('requires a new explicit binding after switching regions', () => {
  const onChange = vi.fn();
  render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={{ ...source, lark_resources: binding }} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Messaging platform' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'feishu' }));
  expect(onChange).toHaveBeenCalledWith({ ...source, platform: 'feishu' });
});
it('gates changed source bindings at submission while preserving saved records and removal', () => {
  const saved = { sources: [{ ...source, lark_resources: binding }] };
  const changed = { sources: [{ ...source, lark_resources: { ...binding, app_id: 'cli_other' } }] };
  expect(() => validateRelayAuthoring({ status: 'legacy' }, saved)).toThrow('embedded media');
  expect(() => validateRelayAuthoring({ status: 'legacy' }, changed, saved)).toThrow(
    'embedded media',
  );
  expect(() =>
    validateRelayAuthoring({ status: 'legacy' }, { ...saved, name: 'new' }, saved),
  ).not.toThrow();
  expect(() =>
    validateRelayAuthoring({ status: 'legacy' }, { sources: [source] }, saved),
  ).not.toThrow();
  expect(() => validateRelayAuthoring(available, changed, saved)).not.toThrow();
});
