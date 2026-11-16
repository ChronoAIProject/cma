import test from 'node:test';
import assert from 'node:assert/strict';
import {publicSetups} from './public-setups.mjs';
import {periodFields, requirePeriod, invalidPlatformPeriods} from './spend-period-data.mjs';
import {periodModel} from './spend-period-model.mjs';
import {spendPeriod} from './spend-browser.mjs';

const wholePeriod = () => ({...periodModel().f.period,
  period_start: '2026-11-17T00:00:00Z', period_end: '2026-11-18T00:00:00Z'});

async function saveDirect(period, options = {}) {
  const h = periodModel({existing: true, ...options}), responses = [];
  h.page.on('response', r => responses.push(r));
  await h.page.goto();
  for (const [key, value] of Object.entries(period))
    await h.page.getByLabel(periodFields[key]).fill(String(value));
  await h.page.getByRole('button', {name: 'Save settings'}).click();
  return {h, responses};
}

test('public positive period uses the backend twenty-character UTC format', async () => {
  const h = periodModel();
  const f = await publicSetups.spend_period(h.ctx);
  for (const key of ['period_start', 'period_end'])
    assert.match(f.period[key], /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/);
  assert.equal(Date.parse(f.period.period_end) - Date.parse(f.period.period_start), 86400000);
  assert.deepEqual(h.calls.map(c => c.method), ['GET']);
  await f.cleanup();
});

test('period fixture accepts backend whole seconds and refuses fractional identity', () => {
  const period = wholePeriod();
  assert.doesNotThrow(() => requirePeriod(period));
  assert.throws(() => requirePeriod({...period, period_start: '2026-11-17T00:00:00.000Z'}), /admin_period_time_fixture/);
});

test('frontend-valid fractional period reaches modeled backend refusal without persistence', async () => {
  const period = wholePeriod();
  period.period_start = '2026-11-17T00:00:00.000Z';
  const {h, responses} = await saveDirect(period);
  assert.equal(h.actions.find(a => a.op === 'click').invalid, false);
  assert.equal(responses.length, 1);
  assert.equal(responses[0].status(), 422);
  assert.deepEqual(responses[0].request().postDataJSON().settings.model_budget.platform.period_start, period.period_start);
  assert.deepEqual(h.settings(), h.original);
});

test('actual public recipe completes all fifteen negatives and the exact modeled save/restoration', async () => {
  const h = periodModel({existing: true}); h.ctx.am = 'b';
  const f = await publicSetups.spend_period(h.ctx); h.f.period = f.period;
  await spendPeriod.run(h.ctx); assert.deepEqual(await h.cleanup(), []);
  const saved = h.observations.find(o => o.label === 'admin_period_save').value;
  assert.equal(saved.status, 200);
  for (const [key, value] of Object.entries(f.period))
    assert.equal(saved.body.settings.model_budget.platform[key], value);
  assert.equal(h.actions.filter(a => a.op === 'click' && a.invalid).length, 15);
  assert.equal(h.receipts.filter(r => r.method === 'PUT').length, 1);
  assert.deepEqual(new Set(h.calls.map(c => c.am)), new Set(['a', 'b']));
  assert.deepEqual(h.settings(), h.original);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.ok(h.actions.find(a => a.op === 'restore').closed);
  await f.cleanup();
});

for (const [start, end] of [
  ['1970-01-01T00:00:00Z', '1970-01-01T00:00:01Z'],
  ['2000-02-29T23:59:59Z', '2000-03-01T00:00:00Z'],
  ['2024-02-29T12:30:45Z', '2024-03-01T12:30:45Z'],
  ['9999-12-31T23:59:58Z', '9999-12-31T23:59:59Z'],
]) test('backend-compatible calendar bounds survive unchanged: ' + start, async () => {
  const period = {...wholePeriod(), period_start: start, period_end: end};
  assert.doesNotThrow(() => requirePeriod(period));
  const {h, responses} = await saveDirect(period);
  assert.equal(responses[0].status(), 200);
  assert.equal(h.settings().model_budget.platform.period_start, start);
  assert.equal(h.settings().model_budget.platform.period_end, end);
});

for (const value of [
  '2026-11-17T00:00:00.000Z', '2026-11-17T00:00:00.001Z',
  '2026-11-17T00:00:00+00:00', '2026-11-17T08:00:00+08:00',
  '2026-11-17t00:00:00z', '2026-11-17T00:00:00', '2026-11-17',
  '2026-02-30T00:00:00Z', '2100-02-29T00:00:00Z', '1969-12-31T23:59:59Z',
  '2026-00-17T00:00:00Z', '2026-13-17T00:00:00Z', '2026-11-00T00:00:00Z',
  '2026-11-17T24:00:00Z', '2026-11-17T00:60:00Z', '2026-11-17T00:00:60Z',
  ' 2026-11-17T00:00:00Z', '2026-11-17T00:00:00Z\n', '+010000-01-01T00:00:00Z',
  null, 1794873600000,
]) test('invalid backend timestamp cannot enter either positive fixture bound: ' + JSON.stringify(value), () => {
  // Wide ordered bounds isolate timestamp validity from start/end ordering.
  const period = {...wholePeriod(), period_start: '1970-01-01T00:00:00Z', period_end: '9999-12-31T23:59:59Z'};
  for (const key of ['period_start', 'period_end'])
    assert.throws(() => requirePeriod({...period, [key]: value}), /admin_period_time_fixture/);
});

for (const value of ['2026-11-17T00:00:00.000Z', '2026-11-17T00:00:00.001Z',
  '2026-11-17T00:00:00+00:00', '2026-11-17T08:00:00+08:00', '1969-12-31T23:59:59Z',
]) test('broader frontend-valid timestamp is independently refused by backend model: ' + value, async () => {
  for (const key of value.startsWith('1969') ? ['period_start'] : ['period_start', 'period_end']) {
    const period = {...wholePeriod(), period_start: '1970-01-01T00:00:00Z', period_end: '9999-12-31T23:59:59Z', [key]: value};
    const {h, responses} = await saveDirect(period);
    assert.equal(h.actions.find(a => a.op === 'click').invalid, false);
    assert.equal(responses.length, 1); assert.equal(responses[0].status(), 422);
    assert.deepEqual(h.settings(), h.original);
    const snapshot = await h.ctx.request({method: 'GET', path: '/api/v1/admin/settings', am: 'b'});
    assert.equal(snapshot.body.revision, 1);
  }
});

for (const value of ['2026-02-30T00:00:00Z', '2100-02-29T00:00:00Z',
  '2026-00-17T00:00:00Z', '2026-13-17T00:00:00Z', '2026-11-00T00:00:00Z',
  '2026-11-17T24:00:00Z', '2026-11-17T00:60:00Z', '2026-11-17T00:00:60Z',
]) test('modeled backend cannot persist an invalid calendar/time even if dispatched: ' + value, async () => {
  const {h, responses} = await saveDirect({...wholePeriod(), period_start: value}, {invalidDispatch: true});
  assert.equal(responses.length, 1); assert.equal(responses[0].status(), 422);
  assert.deepEqual(h.settings(), h.original);
});

test('equal/reversed whole-second periods stay invalid independently of timestamp syntax', async () => {
  for (const end of ['2026-11-17T00:00:00Z', '2026-11-16T23:59:59Z']) {
    const period = {...wholePeriod(), period_end: end};
    assert.throws(() => requirePeriod(period), /admin_period_time_fixture/);
    const {h, responses} = await saveDirect(period, {invalidDispatch: true});
    assert.equal(responses[0].status(), 422); assert.deepEqual(h.settings(), h.original);
  }
  const negatives = invalidPlatformPeriods(wholePeriod());
  assert.equal(negatives.length, 15);
  const reversed = negatives.find(n => n.field === 'period_end' && Date.parse(n.value.period_end) < Date.parse(n.value.period_start));
  assert.equal(reversed.value.period_end, '2026-11-16T23:59:59Z');
  assert.equal(reversed.error, 'Period end must be after period start.');
});
