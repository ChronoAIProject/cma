import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { RelayDiscovery } from '../RelayCapabilities';
import { ChannelEditor, type Channel } from '../ChannelEditor';
import {
  fitPartition,
  partitionIssue,
  replyIssue,
  schemaIssue,
  validateAuthoringRules,
} from './authoringRules';
import { channelCredentials, defaultSource, type Fields } from './model';

afterEach(() => vi.unstubAllGlobals());

const advertised = (dynamic: boolean) =>
  ({
    status: 'ready',
    authoring: { dynamic_sources_writes_enabled: dynamic, platforms: [] },
  }) as unknown as RelayDiscovery;
const legacy = advertised(false);
const extended = advertised(true);
const unknown: RelayDiscovery = { status: 'failed' };
const custom = defaultSource('custom');
const relay = defaultSource('nyxid_relay');

function newChannel() {
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor ownerSubject="owner" onDone={vi.fn()} denied={vi.fn()} />);
  return fetcher;
}
function choose(label: string, option: string) {
  fireEvent.click(screen.getByRole('button', { name: label }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: option }));
}

it('refuses partition shapes with the exact pointer and message the v1 gateway returns', () => {
  expect(partitionIssue(legacy, { version: 1, key: 'conversation_and_sender' }, [custom])).toEqual({
    path: '/partition/key',
    message: 'custom sources require fixed, event_subject, or per_event',
  });
  expect(partitionIssue(extended, { version: 1, key: 'conversation' }, [custom])).toEqual({
    path: '/partition/key',
    message: 'custom sources require fixed, event_subject, or per_event',
  });
  expect(partitionIssue(legacy, { version: 1, key: 'event_subject' }, [relay])).toEqual({
    path: '/partition/key',
    message: 'requires a custom source',
  });
  expect(
    partitionIssue(legacy, { version: 1, key: 'conversation', custom_key: 'fixed' }, [
      relay,
      custom,
    ])?.path,
  ).toBe('/partition/custom_key');
  for (const [discovery, partition, sources] of [
    [legacy, { version: 1, key: 'event_subject' }, [custom]],
    [legacy, { version: 1, key: 'fixed' }, [relay, custom]],
    [legacy, { version: 1, key: 'conversation_and_sender' }, [relay]],
    [extended, { version: 1, key: 'conversation', custom_key: 'per_event' }, [relay, custom]],
    // Native sources are outside the write gate's legacy check.
    [legacy, { version: 1, key: 'conversation' }, [defaultSource('discord')]],
  ] as const)
    expect(partitionIssue(discovery, partition, sources)).toBeUndefined();
});

it('fits a valid grouping to new producers and leaves valid groupings alone', () => {
  const draft = (sources: Fields[], partition: Fields = { version: 1, key: 'conversation' }) => ({
    sources,
    partition,
  });
  expect(fitPartition(legacy, draft([custom])).partition).toEqual({
    version: 1,
    key: 'event_subject',
  });
  expect(fitPartition(legacy, draft([relay, custom])).partition).toEqual({
    version: 1,
    key: 'fixed',
  });
  expect(fitPartition(extended, draft([relay, custom])).partition).toEqual({
    version: 1,
    key: 'conversation',
    custom_key: 'event_subject',
  });
  expect(fitPartition(legacy, draft([relay], { version: 1, key: 'per_event' })).partition).toEqual({
    version: 1,
    key: 'conversation_and_sender',
  });
  const valid = draft([relay]);
  expect(fitPartition(legacy, valid)).toBe(valid);
});

it('defaults a new Custom event channel to a grouping the gateway accepts', async () => {
  const fetcher = newChannel();
  choose('Producer 1 connection type', 'Custom event');
  const grouping = screen.getByRole('button', { name: 'Group incoming messages by' });
  expect(grouping).toHaveTextContent('Each event subject');
  expect(screen.queryByRole('button', { name: 'Group custom events by' })).not.toBeInTheDocument();
  fireEvent.click(grouping);
  for (const name of [/Each sender in each conversation/, /^Each conversation/, /Each thread/])
    expect(screen.getByRole('menuitemradio', { name })).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getByRole('menuitemradio', { name: /Each event subject/ })).not.toHaveAttribute(
    'aria-disabled',
    'true',
  );
  expect(fetcher).not.toHaveBeenCalled();
});

it('does not promise automatic answers once the last reply destination is removed', async () => {
  const fetcher = newChannel();
  fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'No route' } });
  fireEvent.change(screen.getByLabelText('Provider agent key'), { target: { value: 'agent' } });
  choose('Producer 1 connection type', 'Custom event');
  fireEvent.change(screen.getByLabelText('HMAC key ID'), { target: { value: 'events' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add HMAC credential' }));
  fireEvent.change(screen.getByLabelText('HMAC 1 key ID'), { target: { value: 'events' } });
  fireEvent.change(screen.getByLabelText('HMAC 1 secret'), {
    target: { value: btoa('synthetic event material') },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Remove destination 1' }));
  const review = within(screen.getByRole('group', { name: 'Review' }));
  expect(review.queryByText('Final answers sent automatically')).not.toBeInTheDocument();
  expect(review.getByText(/Final answers have no reply destination/)).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Add a reply destination or turn off Send final answers automatically',
  );
  expect(fetcher).not.toHaveBeenCalled();
});

it('offers each single-use reply destination once', () => {
  newChannel();
  fireEvent.click(screen.getByRole('button', { name: 'Add reply destination' }));
  expect(screen.getByRole('button', { name: 'Destination 2 type' })).toHaveTextContent(
    'Synchronous response',
  );
  fireEvent.click(screen.getByRole('button', { name: 'Destination 2 type' }));
  const original = screen.getByRole('menuitemradio', { name: /Original conversation/ });
  expect(original).toHaveAttribute('aria-disabled', 'true');
  expect(original).toHaveTextContent('Already used by destination 1.');
});

it('refuses repeated and missing reply destinations, but keeps a saved empty list', () => {
  const webhook = { type: 'result_webhook', url: 'https://example.test/a', hmac_key_id: 'k' };
  expect(replyIssue({ auto_final: true, sinks: [{ type: 'source' }, { type: 'source' }] })).toEqual(
    expect.objectContaining({ path: '/reply/sinks/1' }),
  );
  expect(
    replyIssue({ sinks: [webhook, { ...webhook, url: ' https://example.test/a ' }] })?.path,
  ).toBe('/reply/sinks/1');
  expect(replyIssue({ sinks: [webhook, { ...webhook, url: 'https://example.test/b' }] })).toBe(
    undefined,
  );
  const automatic = { auto_final: true, sinks: [] };
  const manual = { auto_final: false, sinks: [] };
  expect(replyIssue(automatic)?.path).toBe('/reply/sinks');
  expect(replyIssue(manual)).toBeUndefined();
  expect(replyIssue(automatic, manual)?.path).toBe('/reply/sinks');
  expect(replyIssue(automatic, { auto_final: true, sinks: [{ type: 'source' }] })?.path).toBe(
    '/reply/sinks',
  );
  expect(replyIssue({ ...automatic, ack_after_ms: 10 }, automatic)).toBeUndefined();
  // Repeats a saved channel already had deliver nothing extra and are not re-judged.
  const repeated = { auto_final: true, sinks: [{ type: 'source' }, { type: 'source' }] };
  expect(replyIssue({ ...repeated, ack_after_ms: 10 }, repeated)).toBeUndefined();
});

it('drops HMAC replacement controls and credentials when a Custom source uses an agent key', () => {
  newChannel();
  choose('Producer 1 connection type', 'Custom event');
  expect(screen.queryByLabelText('Replace HMAC credentials')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Add HMAC credential' })).toBeVisible();
  choose('Authentication method', 'NyxID agent key');
  expect(screen.queryByLabelText('Replace HMAC credentials')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Add HMAC credential' })).not.toBeInTheDocument();
  const draft = { sources: [{ type: 'custom', auth: { type: 'nyxid_key', key_hashes: [] } }] };
  expect(
    channelCredentials(
      { agent_key: 'agent', replace_hmac: true, hmac_secrets: [{ key_id: 'k', secret: 's' }] },
      draft,
      true,
    ),
  ).toEqual({ agent_key: 'agent' });
});

it('does not offer Custom event schemas the v1 gateway refuses', () => {
  newChannel();
  choose('Producer 1 connection type', 'Custom event');
  expect(screen.queryByRole('button', { name: 'Add schema validation' })).not.toBeInTheDocument();
  expect(schemaIssue([custom])).toBeUndefined();
  expect(schemaIssue([relay, { ...custom, schema: { type: 'object' } }])).toEqual(
    expect.objectContaining({ path: '/sources/1/schema' }),
  );
});

it('refuses a retained Custom schema until it is removed', async () => {
  const channel = {
    channel_id: 'chan',
    record_id: 'rec',
    version: 3,
    provider: { slug: 'cma_codex' },
    endpoints: {},
    definition: {
      schema_version: 1,
      name: 'Events',
      provider: { slug: 'cma_codex' },
      profile: {},
      sources: [
        { type: 'custom', auth: { type: 'hmac', key_id: 'events' }, schema: { type: 'object' } },
      ],
      partition: { version: 1, key: 'event_subject' },
      reply: { auto_final: true, ack_after_ms: 0, sinks: [{ type: 'sync' }] },
    },
  } as unknown as Channel;
  const fetcher = vi
    .fn()
    .mockResolvedValue(new Response(JSON.stringify({ ...channel, version: 4 }), { status: 200 }));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ChannelEditor
      channel={channel}
      section={{ kind: 'producer', index: 0 }}
      onDone={vi.fn()}
      denied={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Event schema validation is not supported by this gateway',
  );
  expect(fetcher).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Remove schema validation' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(JSON.parse(fetcher.mock.calls[0]![1].body).sources[0]).not.toHaveProperty('schema');
});

it('re-judges only changed settings when a saved channel is edited', () => {
  const saved = {
    sources: [relay],
    partition: { version: 1, key: 'conversation' },
    reply: { auto_final: true, sinks: [] },
  };
  expect(() => validateAuthoringRules(legacy, { ...saved, name: 'Renamed' }, saved)).not.toThrow();
  expect(() =>
    validateAuthoringRules(legacy, { ...saved, sources: [relay, custom] }, saved),
  ).toThrow('/partition/key: custom sources require fixed, event_subject, or per_event');
});

it('leaves custom-event grouping to the gateway until it advertises its capabilities', () => {
  const partition = { version: 1, key: 'conversation', custom_key: 'fixed' };
  expect(partitionIssue(unknown, partition, [relay, custom])).toBeUndefined();
  expect(partitionIssue(legacy, partition, [relay, custom])?.path).toBe('/partition/custom_key');
  // Reader rules still apply: a Custom-only channel cannot use chat grouping.
  expect(partitionIssue(unknown, { version: 1, key: 'conversation' }, [custom])?.path).toBe(
    '/partition/key',
  );
});

it("keeps the owner's grouping when only the separate custom grouping is invalid", () => {
  const draft = {
    sources: [relay, custom],
    partition: { version: 1, key: 'fixed', custom_key: 'event_subject' },
  };
  expect(fitPartition(extended, draft).partition).toEqual({ version: 1, key: 'fixed' });
});

it('requires an explicit destination again after removing the default chat reply', async () => {
  const fetcher = newChannel();
  for (const [label, value] of [
    ['Channel name', 'Chat replies'],
    ['NyxID issuer', 'https://example.test'],
    ['Signing key ID', 'relay-key'],
    ['Route IDs 1', 'relay-route'],
    ['NyxID channel key', 'fixture-channel-key'],
    ['Provider agent key', 'fixture-agent-key'],
  ] as const) {
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
  }
  fireEvent.click(screen.getByRole('button', { name: 'Remove destination 1' }));
  const review = within(screen.getByRole('group', { name: 'Review' }));
  expect(review.getByText(/Final answers have no reply destination/)).toBeVisible();
  expect(
    review.queryByText(/sent automatically to the original conversation/),
  ).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Add a reply destination or turn off Send final answers automatically',
  );
  expect(fetcher).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Add reply destination' }));
  expect(screen.getByRole('button', { name: 'Destination 1 type' })).toHaveTextContent(
    'Original conversation',
  );
  expect(review.getByText('Final answers sent automatically')).toBeVisible();
  fetcher.mockResolvedValue(new Response(null, { status: 400 }));
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(JSON.parse(fetcher.mock.calls[0]![1].body).reply).toMatchObject({
    auto_final: true,
    sinks: [{ type: 'source' }],
  });
});
