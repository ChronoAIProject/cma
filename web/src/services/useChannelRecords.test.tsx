import { act, renderHook } from '@testing-library/react';
import { expect, it } from 'vitest';
import { channelIsWritable } from './channelEvidence';
import type { ChannelListItem } from './channelTypes';
import { useChannelRecords } from './useChannelRecords';

const record = (id: string, ready = true): ChannelListItem => ({
  record_id: id,
  channel_id: id,
  version: 7,
  provider: { slug: 'cma', level: 'l3' },
  definition: {
    name: id,
    sources: [],
    creation: { ready },
    owner: { kind: 'human', subject: 'owner' },
  },
  endpoints: { custom_event_url: '', nyxid_callback_url: '' },
});

function fixture() {
  return renderHook(({ page }) => useChannelRecords(page, undefined), {
    initialProps: { page: { items: [record('first'), record('second')] } },
  });
}

it('resolves a second-page conflict only when the refreshed page containing that record arrives', () => {
  const view = fixture();
  act(() => view.result.current.observe(record('second', false)));
  expect(view.result.current.records.get('second')!.unconfirmed).toBe(true);
  act(() => view.result.current.beginRefresh());
  view.rerender({ page: { items: [record('first')] } });
  expect(channelIsWritable(view.result.current.records.get('second')!, false)).toBe(false);
  view.rerender({ page: { items: [record('first'), record('second')] } });
  expect(view.result.current.records.get('second')!.unconfirmed).toBe(false);
  expect(channelIsWritable(view.result.current.records.get('second')!, false)).toBe(true);
});

it.each([false, true])(
  'keeps a conflict observed after refresh starts blocked, with an earlier conflict=%s',
  (earlierConflict) => {
    const view = fixture();
    if (earlierConflict) act(() => view.result.current.observe(record('second', false)));
    act(() => view.result.current.beginRefresh());
    view.rerender({ page: { items: [record('first')] } });
    act(() => view.result.current.observe(record('second', false)));
    view.rerender({ page: { items: [record('first'), record('second')] } });
    expect(view.result.current.records.get('second')!.unconfirmed).toBe(true);
    expect(channelIsWritable(view.result.current.records.get('second')!, false)).toBe(false);

    act(() => view.result.current.beginRefresh());
    view.rerender({ page: { items: [record('first')] } });
    expect(channelIsWritable(view.result.current.records.get('second')!, false)).toBe(false);
    view.rerender({ page: { items: [record('first'), record('second')] } });
    expect(view.result.current.records.get('second')!.unconfirmed).toBe(false);
    expect(channelIsWritable(view.result.current.records.get('second')!, false)).toBe(true);
  },
);
