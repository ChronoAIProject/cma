import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { saveWatch, validateWatchConfig, watch } from './cma-bridge-continuity-watch.mjs';
import { config, drive, event, fakeClock, fixture, unavailable } from './cma-bridge-continuity-watch-fixture.mjs';

async function observe(frames, options = {}) {
  const timer = fakeClock();
  const reads = fixture(frames);
  const result = await drive(watch(options.config ?? config, reads, { timer, signal: options.signal }), timer);
  assert.equal(timer.pending(), 0);
  return { result, reads, timer };
}

test('an intervening API outage and recovery remain visible without stopping observation', async () => {
  const { result } = await observe([{}, { error: unavailable() }, { events: [event(1), event(2)] }]);
  assert.equal(result.status, 'completed');
  assert.equal(result.observedSamples, 3);
  assert.equal(result.errorSamples, 1);
  assert.equal(result.samples[1].code, 'public_read_failed_503');
  assert.equal(result.samples[2].comparison.status, 'passed');
  assert.equal(result.samples[2].snapshot.agents[0].events.cursor, 2);
  assert.ok(result.findings.some((entry) => entry.code === 'public_read_failed_503'));
  assert.ok(result.gaps.some((entry) => entry.code === 'incomplete_sample' && entry.sample === 1));
  assert.ok(result.gaps.some((entry) => entry.code === 'between_samples'));
  assert.ok(result.unproved.includes('continuous_availability_between_samples'));
  assert.ok(!JSON.stringify(result).includes('private event content'));
});

test('history and runtime regressions cannot disappear when later samples recover', async () => {
  for (const [frame, expected] of [
    [{ events: [event(1, 'changed')] }, 'history_prefix_changed'],
    [{ events: [] }, 'history_prefix_changed'],
    [{ workspace: 'wks_replaced' }, 'agent_identity_changed'],
    [{ uid: 'replaced-uid' }, 'runtime_replaced_or_missing'],
    [{ restarts: 1 }, 'runtime_changed'],
  ]) {
    const { result } = await observe([{}, frame, {}, {}]);
    assert.ok(result.findings.some((entry) => entry.sample === 1 && entry.code === expected), expected);
    assert.equal(result.assessment, 'findings_recorded');
  }
});

test('each sample rereads the immutable prefix and records advancing durable cursors', async () => {
  const { result, reads } = await observe([
    { pages: { 0: { items: [event(1)], next: 1 }, 1: { items: [event(2)] } } },
    { pages: { 0: { items: [event(1)], next: 1 }, 1: { items: [event(2), event(3)] } } },
  ]);
  assert.deepEqual(result.samples.map((sample) => sample.snapshot.agents[0].events.cursor), [2, 3, 3, 3]);
  assert.ok(result.samples.slice(1).every((sample) => sample.comparison.status === 'passed'));
  assert.equal(reads.calls.filter((call) => call.path?.endsWith('after=0&limit=100')).length, 4);
  assert.equal(reads.calls.filter((call) => call.path?.endsWith('after=1&limit=100')).length, 4);
  assert.ok(result.samples[0].probes.some((probe) => probe.kind === 'public_events' && probe.agentId === 'agt_fixture' && probe.after === 1));
});

test('gaps, duplicate events and stalled pagination produce retained errors', async () => {
  for (const [frame, expected] of [
    [{ events: [event(1), event(3)] }, 'event_sequence_gap_or_duplicate'],
    [{ events: [event(1), event(1)] }, 'event_sequence_gap_or_duplicate'],
    [{ pages: { 0: { items: [event(1)], next: 0 } } }, 'event_cursor_did_not_advance'],
  ]) {
    const { result } = await observe([{}, frame, {}]);
    assert.equal(result.samples[1].code, expected);
    assert.equal(result.samples[2].comparison.status, 'passed');
  }
});

test('a sample timeout cancels its read, records skipped slots and resumes without overlap', async () => {
  const settings = { ...config, watch: { durationMs: 2000, intervalMs: 250, sampleTimeoutMs: 750 } };
  const { result, reads } = await observe([{ hang: '/' }, {}], { config: settings });
  assert.equal(result.samples[0].code, 'sample_timeout');
  assert.equal(reads.state.aborted, 1);
  assert.equal(reads.state.active, 0);
  assert.equal(reads.state.maximumActive, 1);
  assert.equal(result.samples[1].startedOffsetMs, 750);
  assert.ok(result.gaps.some((entry) => entry.code === 'sample_overran_interval' && entry.skippedSlots === 2));
  assert.ok(result.observedSamples > 0);
});

test('total deadline aborts either active HTTP or inventory and permits no later reads', async () => {
  for (const hang of ['/', 'pods']) {
    const settings = { ...config, watch: { ...config.watch, sampleTimeoutMs: 1000 } };
    const { result, reads, timer } = await observe([{ hang }], { config: settings });
    assert.equal(result.status, 'completed');
    assert.equal(result.elapsedMs, 1000);
    assert.equal(result.samples.length, 1);
    assert.equal(result.samples[0].code, 'watch_deadline');
    assert.equal(reads.state.aborted, 1);
    assert.equal(reads.state.active, 0);
    const count = reads.calls.length;
    timer.advance(2000);
    assert.equal(reads.calls.length, count);
  }
});

test('external cancellation aborts active reads and pending interval waits', async () => {
  for (const frames of [[{ hang: '/' }], [{}]]) {
    const timer = fakeClock();
    const reads = fixture(frames);
    const stop = new AbortController();
    timer.setTimeout(() => stop.abort(new Error('private cancellation detail')), 100);
    const result = await drive(watch(config, reads, { timer, signal: stop.signal }), timer);
    assert.equal(result.status, 'cancelled');
    assert.equal(result.elapsedMs, 100);
    assert.equal(reads.calls.filter((call) => call.path === '/').length, 1);
    assert.equal(reads.state.active, 0);
    assert.equal(timer.pending(), 0);
    assert.ok(!JSON.stringify(result).includes('private cancellation detail'));
  }
  const stop = new AbortController();
  stop.abort();
  const { result, reads } = await observe([{}], { signal: stop.signal });
  assert.equal(result.status, 'cancelled');
  assert.equal(reads.calls.length, 0);
});

test('untrusted exceptions never expose even a simple token-shaped message', async () => {
  const { result } = await observe([{ error: new Error('saved_secret_token') }]);
  assert.ok(result.samples.every((sample) => sample.code === 'continuity_observation_failed'));
  assert.ok(!JSON.stringify(result).includes('saved_secret_token'));
});

test('watch bounds reject unsafe or unbounded schedules before any reads', () => {
  for (const watch of [undefined, {}, { ...config.watch, durationMs: 0 }, { ...config.watch, durationMs: 3_600_001 }, { ...config.watch, intervalMs: 0 }, { ...config.watch, sampleTimeoutMs: 1001 }, { ...config.watch, durationMs: 3_600_000 }]) {
    assert.throws(() => validateWatchConfig({ ...config, watch }));
  }
});

test('receipts are private and exclusive; existing files or symlinks trigger no reads', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'continuity-watch-'));
  try {
    const path = join(directory, 'receipt.json');
    const result = await saveWatch(config, path, { makeTransport: async () => { throw new Error('private transport detail'); } });
    assert.equal(result.status, 'failed');
    assert.equal(result.code, 'continuity_observation_failed');
    assert.equal((await stat(path)).mode & 0o777, 0o600);
    const bytes = await readFile(path, 'utf8');
    assert.ok(!bytes.includes('private transport detail'));
    const linked = join(directory, 'linked.json');
    await symlink(path, linked);
    for (const output of [path, linked]) {
      await assert.rejects(saveWatch(config, output, { makeTransport: () => assert.fail('existing receipt must prevent transport initialization') }), { code: 'EEXIST' });
    }
    assert.equal(await readFile(path, 'utf8'), bytes);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
