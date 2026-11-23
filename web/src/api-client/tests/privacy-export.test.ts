// @vitest-environment node
import { createHash } from 'node:crypto';
import { expect, it } from 'vitest';
import { transport } from './resource-fixtures.js';

const payload = '{"content":"héllo"}\n';
const owner = 'subject-a';
const agent = 'agt_one';
const firstWindow = { turn: null, after: null, through: null, limit: 32 };
const finalWindow = { turn: 3, after: 4, through: 6, limit: 32 };
const finalCursor = { agent, ...finalWindow };

function page(
  content = payload,
  window: {
    turn: number | null;
    after: number | null;
    through: number | null;
    limit: number;
  } = firstWindow,
  next: typeof finalCursor | null = null,
) {
  return {
    owner_subject: owner,
    payload: content,
    manifest: {
      format: 'cma.retained-conversation-page',
      version: 1,
      agent_id: agent,
      through_seq: 6,
      window,
      records: 1,
      size_bytes: Buffer.byteLength(content),
      sha256: createHash('sha256').update(content).digest('hex'),
      coverage: {
        raw_history: false,
        completeness: 'not_certified',
        retention_window: 'not_reported_by_source',
        gaps: 'unknown',
        includes: ['recorded_user_messages'],
        excludes: ['raw_runtime_records'],
      },
      next,
    },
  };
}

it('builds a portable file from verified UTF-8 bytes through the owner route', async () => {
  const t = transport();
  t.answer(page());
  const blob = await t.client.privacyExports.retainedConversation(
    owner,
    agent,
    new AbortController().signal,
  );
  expect(JSON.parse(await blob.text())).toMatchObject({
    pages: [{ payload, manifest: page().manifest }],
  });
  expect(await blob.text()).not.toContain(owner);
  expect(new URL(t.calls[0]!.url).pathname).toBe(
    '/api/v1/account/privacy-export/agents/agt_one/transcript',
  );
  expect(t.calls[0]!.init).toMatchObject({ cache: 'no-store', redirect: 'error' });
});

it('downloads the first and final pages in order using the original source sequence', async () => {
  const t = transport();
  const first = page(payload, firstWindow, finalCursor);
  const final = page('{"content":"final"}\n', finalWindow);
  t.fetch.mockResolvedValueOnce(Response.json(first));
  t.answer(final);
  const blob = await t.client.privacyExports.retainedConversation(
    owner,
    agent,
    new AbortController().signal,
  );
  expect(JSON.parse(await blob.text()).pages).toEqual(
    [first, final].map(({ manifest, payload }) => ({ manifest, payload })),
  );
  expect(Object.fromEntries(new URL(t.calls[0]!.url).searchParams)).toEqual({
    agent,
    turn: '3',
    after: '4',
    through: '6',
    limit: '32',
  });
});

it.each([false, true])(
  'rejects a valid final page returned for an earlier window',
  async (hasFirst) => {
    const t = transport();
    if (hasFirst)
      t.fetch.mockResolvedValueOnce(Response.json(page(payload, firstWindow, finalCursor)));
    t.answer(page('{"content":"wrong order"}\n', { ...finalCursor, after: 5 }));
    await expect(
      t.client.privacyExports.retainedConversation(owner, agent, new AbortController().signal),
    ).rejects.toThrow(/requested page/);
    expect(t.fetch).toHaveBeenCalledTimes(hasFirst ? 2 : 1);
  },
);

it.each([{ owner_subject: 'wrong-subject' }, { payload: payload.replace('h', 'j') }])(
  'withholds data after a changed account or digest mismatch',
  async (override) => {
    const t = transport();
    t.answer({ ...page(), ...override });
    await expect(
      t.client.privacyExports.retainedConversation(owner, agent, new AbortController().signal),
    ).rejects.toThrow();
  },
);

it('withholds the inventory when the session owner changes', async () => {
  const t = transport();
  t.answer({
    owner_subject: 'wrong-subject',
    data_class: 'retained_conversation',
    agents: [],
    next: null,
  });
  await expect(t.client.privacyExports.inventory(owner)).rejects.toThrow(/did not match/);
});
