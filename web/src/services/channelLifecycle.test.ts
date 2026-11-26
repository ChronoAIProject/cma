import { expect, it } from 'vitest';
import { classifyChannelLifecycle } from './channelLifecycle';
import { channelEvidenceState, channelIsWritable, mergeChannelEvidence } from './channelEvidence';
import type { ChannelListItem } from './channelTypes';

const record = (creation: unknown, version = 7, addressed = true): ChannelListItem => ({
  record_id: 'record',
  version,
  owner: 'owner',
  provider: { slug: 'cma', level: 'l3' },
  definition: { name: 'Channel', sources: [], creation },
  ...(addressed
    ? { channel_id: 'original', endpoints: { custom_event_url: '', nyxid_callback_url: '' } }
    : {}),
});

it('recognizes only an absent property as legacy', () => {
  expect(classifyChannelLifecycle({})).toEqual({ kind: 'legacy' });
  expect(classifyChannelLifecycle({ creation: undefined })).toEqual({ kind: 'unconfirmed' });
});
it.each([null, [], 'ready', false, {}, { ready: null }, { ready: 'true' }, { ready: 1 }])(
  'does not grant readiness to malformed metadata %j',
  (creation) => {
    expect(classifyChannelLifecycle({ creation })).toEqual({ kind: 'unconfirmed' });
  },
);
it.each([true, false])('accepts additive fields with ready=%s', (ready) => {
  expect(classifyChannelLifecycle({ creation: { ready, future: 'value' } }).kind).toBe(
    ready ? 'complete' : 'incomplete',
  );
});
it.each([true, false])('separates address availability from creation evidence: %s', (addressed) => {
  const ready = mergeChannelEvidence(undefined, record({ ready: true }, 7, addressed));
  const incomplete = mergeChannelEvidence(undefined, record({ ready: false }, 7, addressed));
  expect(channelEvidenceState(incomplete).kind).toBe('incomplete');
  expect(channelIsWritable(incomplete, false)).toBe(false);
  expect(channelIsWritable(ready, false)).toBe(addressed);
  expect(channelIsWritable(ready, true)).toBe(false);
});
it('retains a known address for display without authorizing an unaddressed newer response', () => {
  const previous = mergeChannelEvidence(undefined, record({ ready: true }));
  const next = mergeChannelEvidence(previous, record({ ready: true }, 8, false));
  expect(next.address?.channel_id).toBe('original');
  expect(channelIsWritable(next, false)).toBe(false);
});
it('ignores older readiness even after explicit refresh', () => {
  const current = mergeChannelEvidence(undefined, record({ ready: false }, 8));
  expect(mergeChannelEvidence(current, record({ ready: true }), true)).toBe(current);
});
it.each([true, false])(
  'quarantines contradictory same-version responses in either order: %s',
  (ready) => {
    const current = mergeChannelEvidence(undefined, record({ ready }));
    const conflict = mergeChannelEvidence(current, record({ ready: !ready }), true);
    expect(channelEvidenceState(conflict).kind).toBe('unconfirmed');
    expect(channelIsWritable(conflict, false)).toBe(false);
    expect(channelIsWritable(mergeChannelEvidence(conflict, record({ ready: true })), false)).toBe(
      false,
    );
    expect(
      channelIsWritable(mergeChannelEvidence(conflict, record({ ready: true }), true), false),
    ).toBe(true);
  },
);
it.each([
  { record_id: 'foreign' },
  { channel_id: 'replacement' },
  { owner: 'other' },
  { version: NaN },
])('rejects changed identity or invalid version %j', (change) => {
  const current = mergeChannelEvidence(undefined, record({ ready: false }));
  const next = mergeChannelEvidence(current, { ...record({ ready: true }), ...change }, true);
  expect(channelEvidenceState(next).kind).toBe('unconfirmed');
  expect(next.channel.record_id).toBe('record');
  expect(next.address?.channel_id).toBe('original');
  expect(channelIsWritable(next, false)).toBe(false);
});
