import test from 'node:test';
import assert from 'node:assert/strict';
import { Turn } from '../staging/turn.mjs';
import { TRANSCRIPT_PAGES, TRANSCRIPT_BYTES } from '../staging/transcript.mjs';
import { transcriptPage } from './staging-transcript-fixture.mjs';

function probe({ mutate = () => {}, through = 75 } = {}) {
  const calls = []; const rows = [];
  const run = { config: { runId: 'synthetic-run' }, time: { now: () => 0 },
    remaining: (deadline) => { assert.ok(deadline > 0); return deadline; } };
  const fixture = { run, spec: { index: 1 }, agent: 'agt_synthetic', row: (row) => rows.push(row) };
  const turn = new Turn(fixture, 'synthetic-offer');
  const turns = [{ turn_sequence: 2, started_at: '2026-01-01T00:00:00Z', owner_message: turn.prompt,
    messages: [{ sequence: 1, role: 'user', content: turn.prompt },
      { sequence: through - 1, role: 'assistant', content: turn.answer }], outcome: { type: 'completed', at: 'synthetic-time' } }];
  fixture.get = async (path) => {
    calls.push(path);
    const value = transcriptPage(path, turns, through);
    await mutate(value, calls.length, turns, run);
    return { value };
  };
  return { turn, turns, calls, rows };
}

test('complete split turn assembles all pages before terminal/nonce success without a stream binding', async () => {
  const { turn, calls } = probe();
  assert.equal(await turn.durable(1000), true); assert.equal(turn.settled, true);
  assert.deepEqual(calls, ['/api/v1/agents/agt_synthetic/transcript?limit=32',
    '/api/v1/agents/agt_synthetic/transcript?turn=2&after=33&through=75&limit=32',
    '/api/v1/agents/agt_synthetic/transcript?turn=2&after=65&through=75&limit=32']);
});

test('settlement searches later turns when the input acknowledgement and stream binding were lost', async () => {
  const { turn, turns, calls } = probe({ through: 90 });
  turns.unshift({ turn_sequence: 1, started_at: 'synthetic-time', owner_message: 'previous prompt', messages: [],
    outcome: { type: 'completed', at: 'synthetic-time' } });
  turn.state = 'uncertain';
  await turn.settle(1000);
  assert.equal(turn.settled, true); assert.equal(calls.length, 4); assert.equal(turn.passed, undefined);
});

for (const [name, mutate] of [
  ['stale continuation', (v, n) => { if (n === 2) v.next.after = 33; }],
  ['changed high water', (v, n) => { if (n === 2) v.through_seq++; }],
  ['changed continuation high water', (v) => { v.next.through++; }],
  ['wrong Agent', (v, n) => { if (n === 2) v.agent_id = 'agt_foreign'; }],
  ['wrong turn', (v, n) => { if (n === 2) v.turns[0].turn_sequence++; }],
  ['changed header', (v, n) => { if (n === 2) v.turns[0].owner_message = 'foreign'; }],
  ['wrong nonce on final page', (v, n) => { if (n === 3) {
    v.turns[0].agent_messages[0] = 'OK foreign'; v.turns[0].messages[0].content = 'OK foreign';
  } }],
  ['interrupted pagination', (_v, n) => { if (n === 2) throw new Error('synthetic connection lost'); }],
]) test(`${name} cannot settle an incomplete transcript`, async () => {
  const { turn } = probe({ mutate });
  await assert.rejects(turn.durable(1000)); assert.equal(turn.settled, false);
});

test('an early answer and terminal header cannot hide a duplicate assistant in a later source page', async () => {
  const { turn, turns } = probe();
  turns[0].messages.splice(1, 0, { sequence: 3, role: 'assistant', content: turn.answer });
  await assert.rejects(turn.durable(1000)); assert.equal(turn.settled, false);
});

test('a duplicate nonce in a later turn cannot be treated as one completed turn', async () => {
  const { turn, turns } = probe({ through: 80 });
  turns.push({ ...turns[0], turn_sequence: 79, messages: [] });
  await assert.rejects(turn.durable(1000)); assert.equal(turn.settled, false);
});

test('transcript traversal has finite page, aggregate wire-byte and total-time budgets', async () => {
  const many = probe({ through: (TRANSCRIPT_PAGES + 1) * 32 });
  await assert.rejects(many.turn.durable(1000), /transcript_page_limit/);
  assert.equal(many.calls.length, TRANSCRIPT_PAGES); assert.equal(many.turn.settled, false);
  const large = probe(); const get = large.turn.fixture.get;
  large.turn.fixture.get = async (path) => ({ ...await get(path), bytes: TRANSCRIPT_BYTES / 2 + 1 });
  await assert.rejects(large.turn.durable(1000), /transcript_byte_limit/);
  assert.equal(large.calls.length, 2); assert.equal(large.turn.settled, false);
  let now = 0;
  const slow = probe({ mutate: () => { now += 600; } });
  slow.turn.run.remaining = (deadline) => { if (now >= deadline) throw new Error('operation_deadline'); return deadline - now; };
  await assert.rejects(slow.turn.durable(1000), /operation_deadline/);
  assert.equal(slow.calls.length, 2); assert.equal(slow.turn.settled, false);
});

test('an interrupted snapshot is discarded; a later settlement starts from a fresh first page', async () => {
  let interrupted = false;
  const { turn, calls } = probe({ mutate: (_v, n) => {
    if (n === 2 && !interrupted) { interrupted = true; throw new Error('synthetic interruption'); }
  } });
  await assert.rejects(turn.durable(1000)); assert.equal(turn.settled, false);
  turn.state = 'uncertain'; await turn.settle(1000);
  assert.equal(turn.settled, true); assert.equal(calls[2], calls[0]); assert.equal(calls.length, 5);
});
