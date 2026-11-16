import { describe, expect, it } from 'vitest';
import { ProtocolError } from '../src/index.js';
import { harness } from './responses-v2-fixtures.js';

const output = {
  id: 'retained_checkpoint',
  agent_id: 'agent',
  turn_sequence: 3,
  checkpoint_sequence: 5,
  failure_sequence: 6,
  failed_at: '2026-11-17T00:00:00Z',
  byte_length: 1024 * 1024,
  partial: true,
};
const multibyte = '界'.repeat((1024 * 1024 - 1) / 3) + 'x';

describe('retained partial output', () => {
  it.each([
    ['agent', multibyte, 174_762, 3],
    ['response', multibyte, 174_762, 3],
    ['agent', '\u0001'.repeat(1024 * 1024), 512 * 1024, 2],
    ['response', '\u0001'.repeat(1024 * 1024), 512 * 1024, 2],
  ] as const)(
    'retrieves every maximum-size byte through %s without changing replay or outcomes',
    async (surface, content, characters, requests) => {
      const t = harness();
      let offset = 0;
      let actual = '';
      let cursor: string | undefined;
      while (offset < output.byte_length) {
        const chunk = content.slice(actual.length, actual.length + characters);
        const nextOffset = offset + new TextEncoder().encode(chunk).length;
        const next = nextOffset < output.byte_length ? `retained_checkpoint:${nextOffset}` : null;
        t.fetch.mockResolvedValueOnce(
          Response.json({ output, offset, content: chunk, next, additive_future_field: true }),
        );
        const page =
          surface === 'agent'
            ? await t.client.agents.retainedOutput('agent', { cursor })
            : await t.client.responsesV2.retainedOutput('response', { cursor });
        const [url, init] = t.fetch.mock.calls.at(-1)!;
        expect(new URL(String(url)).searchParams.get('cursor')).toBe(cursor ?? null);
        expect(init?.method).toBe('GET');
        expect(init?.body).toBeUndefined();
        actual += page.content;
        cursor = page.next ?? undefined;
        offset = nextOffset;
      }
      expect(actual).toBe(content);
      expect(t.fetch).toHaveBeenCalledTimes(requests);
    },
  );
  it.each([
    { output: { ...output, agent_id: 'foreign' } },
    { output: { ...output, partial: false } },
    { output: { ...output, byte_length: 1024 * 1024 + 1 } },
    { output: { ...output, failure_sequence: 3 } },
    { content: 'x'.repeat(512 * 1024 + 1) },
    { offset: 1 },
    { next: null },
    { next: '' },
    { output: null },
  ])('rejects inconsistent partial text: %j', async (change) => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(
      Response.json({ output, offset: 0, content: 'first', next: 'next', ...change }),
    );
    await expect(t.client.agents.retainedOutput('agent')).rejects.toBeInstanceOf(ProtocolError);
  });
  it('reports absence without inventing a message and propagates a revoked continuation', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(
      Response.json({ output: null, offset: 0, content: '', next: null }),
    );
    expect((await t.client.agents.retainedOutput('agent')).output).toBeNull();
    t.fetch.mockResolvedValueOnce(
      Response.json(
        { code: 'agent_not_found', detail: 'No Agent matches this request.' },
        { status: 404 },
      ),
    );
    await expect(
      t.client.agents.retainedOutput('agent', { cursor: 'retained_checkpoint:65536' }),
    ).rejects.toMatchObject({ status: 404 });
  });
});
