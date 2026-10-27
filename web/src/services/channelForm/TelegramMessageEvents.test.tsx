import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
import { SourceFields } from './SourceFields';
import { validateRelayAuthoring } from './relayAuthoring';
const source = { type: 'telegram', bot_id: '77', admission: { type: 'all' } };
const available: RelayDiscovery = {
  status: 'ready',
  authoring: {
    native_telegram_message_events_enabled: true,
    dynamic_sources_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    multi_consumer_writes_enabled: true,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  },
};
it('requires confirmed support, preserves saved opt-in, and allows opt-out', () => {
  const onChange = vi.fn();
  const view = render(<SourceFields value={source} onChange={onChange} />);
  expect(screen.getByLabelText('Receive message edits and reactions')).toBeDisabled();
  view.rerender(<SourceFields value={{ ...source, message_events: true }} onChange={onChange} />);
  expect(screen.getByLabelText('Receive message edits and reactions')).toBeChecked();
  fireEvent.click(screen.getByLabelText('Receive message edits and reactions'));
  expect(onChange).toHaveBeenCalledWith(source);
  const help = screen.getByText(/Anonymous reactions and aggregate reaction counts/);
  expect(help).toHaveTextContent('keep the actor unknown and require open sender admission.');
  expect(help).toHaveTextContent('Telegram does not provide general message deletion events.');
  const opted = { sources: [{ ...source, message_events: true }] };
  expect(() => validateRelayAuthoring({ status: 'legacy' }, opted)).toThrow(/disabled/);
  expect(() => validateRelayAuthoring({ status: 'legacy' }, opted, opted)).not.toThrow();
  expect(() =>
    validateRelayAuthoring({ status: 'legacy' }, { sources: [source] }, opted),
  ).not.toThrow();
});
it('authors the source opt-in when capability discovery confirms durable enrollment', () => {
  const onChange = vi.fn();
  render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={source} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByLabelText('Receive message edits and reactions'));
  expect(onChange).toHaveBeenCalledWith({ ...source, message_events: true });
  expect(() =>
    validateRelayAuthoring(available, { sources: [{ ...source, message_events: true }] }),
  ).not.toThrow();
});

it('keeps channel posts separate and clears them when retained message events are disabled', () => {
  const onChange = vi.fn();
  const view = render(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={source} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  expect(screen.getByLabelText('Receive channel posts and edits')).toBeDisabled();
  view.rerender(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={{ ...source, message_events: true }} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByLabelText('Receive channel posts and edits'));
  expect(onChange).toHaveBeenLastCalledWith({
    ...source,
    message_events: true,
    channel_posts: true,
  });
  view.rerender(
    <RelayCapabilitiesContext value={available}>
      <SourceFields
        value={{ ...source, message_events: true, channel_posts: true }}
        onChange={onChange}
      />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByLabelText('Receive message edits and reactions'));
  expect(onChange).toHaveBeenLastCalledWith(source);
  expect(() =>
    validateRelayAuthoring(available, { sources: [{ ...source, channel_posts: true }] }),
  ).toThrow(/require message events/);
  const original = { sources: [{ ...source, message_events: true, channel_posts: true }] };
  const disabled = { sources: [{ ...source, message_events: true }] };
  expect(() => validateRelayAuthoring({ status: 'legacy' }, disabled, original)).not.toThrow();
  expect(() => validateRelayAuthoring({ status: 'legacy' }, original, disabled)).toThrow(
    /disabled/,
  );
  expect(() =>
    validateRelayAuthoring(
      { status: 'legacy' },
      { sources: [{ ...disabled.sources[0], bot_id: '88' }] },
      original,
    ),
  ).toThrow(/disabled/);
});
