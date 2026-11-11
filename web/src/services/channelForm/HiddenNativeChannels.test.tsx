import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel } from '../ChannelEditor';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
import { NodeFields } from './NodeFields';
import { defaultSource } from './model';

const enabled: RelayDiscovery = {
  status: 'ready',
  authoring: {
    native_discord_writes_enabled: true,
    native_discord_auto_activation_enabled: true,
    native_whatsapp_writes_enabled: true,
    native_whatsapp_auto_activation_enabled: true,
    native_lark_writes_enabled: true,
    native_lark_intake_enabled: true,
    multi_consumer_writes_enabled: true,
    dynamic_sources_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  },
};
const choices = ['Direct Discord bot', 'Direct WhatsApp Business', 'Direct Lark / Feishu app'];
const cases = [
  ['native_discord_writes_enabled', choices[0]],
  ['native_discord_auto_activation_enabled', choices[0]],
  ['multi_consumer_writes_enabled', choices[0]],
  ['native_whatsapp_writes_enabled', choices[1]],
  ['native_whatsapp_auto_activation_enabled', choices[1]],
  ['native_lark_writes_enabled', choices[2]],
  ['native_lark_intake_enabled', choices[2]],
] as const;
function picker(discovery: RelayDiscovery, create = true) {
  const change = vi.fn();
  render(
    <RelayCapabilitiesContext value={discovery}>
      <NodeFields
        draft={{ sources: [defaultSource('custom')] }}
        onChange={change}
        credentials={{}}
        onCredentials={vi.fn()}
        section={{ kind: 'producer', index: 0 }}
        create={create}
        sourceCount={0}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Producer 1 connection type' }));
  return change;
}
afterEach(() => vi.unstubAllGlobals());
it.each(cases)('hides native setup when %s is false', (flag, label) => {
  const request = vi.fn();
  vi.stubGlobal('fetch', request);
  const change = picker({ ...enabled, authoring: { ...enabled.authoring!, [flag]: false } });
  expect(screen.queryByRole('menuitemradio', { name: label })).not.toBeInTheDocument();
  expect(screen.getByRole('menuitemradio', { name: 'Custom event' })).toBeInTheDocument();
  expect(change).not.toHaveBeenCalled();
  expect(request).not.toHaveBeenCalled();
});
it.each(['loading', 'legacy', 'failed'] as const)(
  'hides unconfirmed native setup for %s discovery',
  (status) => {
    picker({ status });
    for (const name of choices)
      expect(screen.queryByRole('menuitemradio', { name })).not.toBeInTheDocument();
  },
);
it('also hides disabled choices when attaching a new producer to an existing channel', () => {
  picker(
    { status: 'ready', authoring: { ...enabled.authoring!, native_lark_intake_enabled: false } },
    false,
  );
  expect(screen.queryByRole('menuitemradio', { name: choices[2] })).not.toBeInTheDocument();
});
it('allows explicitly enabled setup', () => {
  const change = picker(enabled);
  for (const name of choices)
    expect(screen.getByRole('menuitemradio', { name })).not.toHaveAttribute(
      'aria-disabled',
      'true',
    );
  fireEvent.click(screen.getByRole('menuitemradio', { name: choices[0] }));
  expect(change).toHaveBeenCalledWith({ sources: [defaultSource('discord')] });
});
it('retains the existing producer but blocks changed authority before sending when paused', () => {
  const request = vi.fn();
  vi.stubGlobal('fetch', request);
  const channel: Channel = {
    channel_id: 'fixture-channel',
    record_id: 'fixture-record',
    version: 1,
    provider: { slug: 'cma_codex', level: 'l3' },
    definition: { name: 'Retained', sources: [{ ...defaultSource('discord'), bot_id: '77' }] },
    endpoints: { custom_event_url: '', nyxid_callback_url: '' },
  };
  render(
    <RelayCapabilitiesContext value={{ status: 'legacy' }}>
      <ChannelEditor
        channel={channel}
        section={{ kind: 'producer', index: 0 }}
        onDone={vi.fn()}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>,
  );
  expect(screen.getByLabelText('Discord bot user ID')).toHaveValue('77');
  fireEvent.change(screen.getByLabelText('Discord bot user ID'), { target: { value: '88' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  expect(screen.getByRole('alert')).toHaveTextContent('not enabled direct Discord authoring');
  expect(screen.getByLabelText('Discord bot user ID')).toHaveValue('88');
  expect(request).not.toHaveBeenCalled();
});
