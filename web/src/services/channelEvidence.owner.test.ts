import { expect, it } from 'vitest';
import { channelIsWritable, channelResponseMatches, mergeChannelEvidence } from './channelEvidence';
import type { Channel } from './channelTypes';
const record = (owner: unknown): Channel => ({
  record_id: 'record',
  channel_id: 'original',
  version: 7,
  provider: { slug: 'cma', level: 'l3' },
  definition: { name: 'Channel', sources: [], creation: { ready: true }, owner },
  endpoints: { custom_event_url: '', nyxid_callback_url: '' },
});
const original = record({ kind: 'human', subject: 'owner' });
it.each(['human', 'organization', 'service_account', 'agent_key'])(
  'matches kind and subject without comparing display metadata for %s',
  (kind) => {
    const previous = record({ kind, subject: 'owner', display_name: 'Old name' });
    const next = { ...record({ kind, subject: 'owner', display_name: 'New name' }), version: 8 };
    expect(channelResponseMatches(previous, next)).toBe(true);
    expect(
      channelIsWritable(
        mergeChannelEvidence(mergeChannelEvidence(undefined, previous), next),
        false,
      ),
    ).toBe(true);
  },
);
it.each([
  { kind: 'organization', subject: 'owner' },
  { kind: 'human', subject: 'other' },
  { kind: 'unsupported', subject: 'owner' },
  { subject: 'owner' },
  { kind: 'human', subject: '' },
  undefined,
  null,
  [],
  'owner',
  {},
])(
  'keeps the confirmed public owner constraint on mismatched or malformed evidence: %j',
  (owner) => {
    const previous = mergeChannelEvidence(undefined, original);
    const next = { ...record(owner), owner: 'owner', version: 8 };
    expect(channelResponseMatches(original, next)).toBe(false);
    const merged = mergeChannelEvidence(previous, next, true);
    expect(merged.channel).toBe(original);
    expect(channelIsWritable(merged, false)).toBe(false);
    expect(merged.unconfirmed).toBe(true);
  },
);
it('does not erase a known structured identity through missing or legacy-only owner metadata', () => {
  const next = { ...original, version: 8, definition: { ...original.definition } };
  delete next.definition.owner;
  expect(channelResponseMatches(original, next)).toBe(false);
  expect(channelResponseMatches(original, { ...next, owner: 'owner' })).toBe(false);
  expect(channelResponseMatches(original, { ...next, owner: { subject: 'owner' } })).toBe(false);
});
it('supports absent and subject-only legacy identities and can learn the public kind', () => {
  const legacy = { ...original, definition: { ...original.definition } };
  delete legacy.definition.owner;
  expect(channelResponseMatches(legacy, legacy)).toBe(true);
  expect(
    channelResponseMatches(
      { ...legacy, owner: 'owner' },
      { ...legacy, owner: { subject: 'owner', display_name: 'Owner' } },
    ),
  ).toBe(true);
  expect(channelResponseMatches({ ...legacy, owner: 'owner' }, original)).toBe(true);
  expect(channelResponseMatches({ ...legacy, owner: 'owner' }, legacy)).toBe(false);
});
