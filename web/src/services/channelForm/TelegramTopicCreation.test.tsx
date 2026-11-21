import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
import { SourceFields } from './SourceFields';
import { validateRelayAuthoring } from './relayAuthoring';

const source = { type: 'telegram', bot_id: '77', admission: { type: 'all' } };
const available: RelayDiscovery = {
  status: 'ready',
  authoring: {
    native_telegram_topic_creation_enabled: true,
    dynamic_sources_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    multi_consumer_writes_enabled: true,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  },
};
it('requires independent creation support and lets owners revoke it while authoring is disabled', () => {
  const onChange = vi.fn();
  const view = render(<SourceFields value={source} onChange={onChange} />);
  expect(screen.getByLabelText('Create new forum topics')).toBeDisabled();
  view.rerender(<SourceFields value={{ ...source, topic_creation: true }} onChange={onChange} />);
  expect(screen.getByLabelText('Create new forum topics')).toBeChecked();
  fireEvent.click(screen.getByLabelText('Create new forum topics'));
  expect(onChange).toHaveBeenLastCalledWith(source);
  view.rerender(
    <RelayCapabilitiesContext value={available}>
      <SourceFields value={source} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByLabelText('Create new forum topics'));
  expect(onChange).toHaveBeenLastCalledWith({ ...source, topic_creation: true });
  expect(screen.getByLabelText('Manage the original forum topic')).toBeDisabled();
  expect(screen.getByText(/Agents may create empty named topics/)).toHaveTextContent(
    'then rename, close or reopen the topics they created',
  );
});

it('preserves creation consent and requires its gate for changed identity or admission', () => {
  const opted = { sources: [{ ...source, topic_creation: true }] };
  expect(() => validateRelayAuthoring({ status: 'legacy' }, opted)).toThrow(/disabled/);
  expect(() => validateRelayAuthoring(available, opted)).not.toThrow();
  expect(() => validateRelayAuthoring({ status: 'legacy' }, opted, opted)).not.toThrow();
  expect(() =>
    validateRelayAuthoring({ status: 'legacy' }, { sources: [source] }, opted),
  ).not.toThrow();
  for (const changed of [
    { ...opted.sources[0], bot_id: '88' },
    { ...opted.sources[0], admission: { type: 'allowlist', sender_ids: ['51'] } },
  ]) {
    expect(() =>
      validateRelayAuthoring({ status: 'legacy' }, { sources: [changed] }, opted),
    ).toThrow(/disabled/);
  }
});

it('allows partial revocation without accidentally reauthorizing another saved Telegram grant', () => {
  const original = {
    sources: [
      {
        ...source,
        topic_creation: true,
        topic_lifecycle: true,
        message_events: true,
        channel_posts: true,
      },
    ],
  };
  for (const next of [
    { ...source, topic_lifecycle: true, message_events: true, channel_posts: true },
    { ...source, topic_creation: true },
    { ...source, topic_creation: true, topic_lifecycle: true },
  ]) {
    expect(() =>
      validateRelayAuthoring({ status: 'legacy' }, { sources: [next] }, original),
    ).not.toThrow();
  }
});
