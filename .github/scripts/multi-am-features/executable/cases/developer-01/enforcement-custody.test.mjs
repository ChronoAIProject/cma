import test from 'node:test';
import assert from 'node:assert/strict';
import {ownedIntent, perUserLimit} from './enforcement.mjs';

const intent = {key: 'create-1', method: 'POST', path: '/api/v1/sandboxes', body: {name: 'test'}};
const accepted = {status: 201, body: {id: 'sandbox-1'}, timing: {am: 'b', request_id: 'request-1'}};

function context(request) {
  const cleanups = [], captures = [], reservations = [];
  const ctx = {am: 'a', request, defer: (label, run) => cleanups.push({label, run})};
  const factory = {custody: async (...args) => {
    reservations.push(args);
    return {cleanup: async () => {}, capture: async response => captures.push(response)};
  }};
  return {ctx, factory, captures, cleanups, reservations};
}

test('owned creation captures the complete accepted receipt with effective ingress custody', async () => {
  const h = context(async () => accepted);
  assert.equal(await ownedIntent(h.ctx, h.factory, intent, {am: 'b'}), accepted);
  assert.deepEqual(h.captures, [accepted]);
  assert.deepEqual(h.reservations, [[intent, {actor: 'user', am: 'b'}]]);
  assert.equal(h.cleanups.length, 1);
});

test('accepted response survives attribution refusal without replay', async () => {
  const error = Object.assign(Error('request_attribution_missing'), {response: accepted});
  let requests = 0;
  const h = context(async () => {requests++; throw error;});
  await assert.rejects(ownedIntent(h.ctx, h.factory, intent), e => e === error);
  assert.deepEqual(h.captures, [accepted]); assert.equal(requests, 1);
});

test('primary and capture failures are retained together', async () => {
  const primary = Object.assign(Error('request_attribution_missing'), {response: accepted});
  const capture = Error('parent_receipt_write_failed');
  const h = context(async () => {throw primary;});
  h.factory.custody = async () => ({cleanup: async () => {}, capture: async () => {throw capture;}});
  await assert.rejects(ownedIntent(h.ctx, h.factory, intent), error => {
    assert.deepEqual(error.errors, [primary, capture]); return true;
  });
});

test('capture failure is not captured again or replayed', async () => {
  let captures = 0, requests = 0;
  const h = context(async () => {requests++; return accepted;});
  h.factory.custody = async () => ({cleanup: async () => {}, capture: async () => {
    captures++; throw Error('capture_failed');
  }});
  await assert.rejects(ownedIntent(h.ctx, h.factory, intent), /capture_failed/);
  assert.equal(captures, 1); assert.equal(requests, 1);
});

test('missing response stays uncertain and cleanup-only parent factories remain compatible', async () => {
  const h = context(async () => {throw Error('unknown_ack');});
  await assert.rejects(ownedIntent(h.ctx, h.factory, intent), /unknown_ack/);
  assert.deepEqual(h.captures, []);
  h.ctx.request = async () => accepted;
  h.factory.custody = async () => ({cleanup: async () => {}});
  assert.equal(await ownedIntent(h.ctx, h.factory, intent), accepted);
});

test('all concurrent creates settle before quota failure and cleanup can start', async () => {
  const h = context();
  let releaseLate, lateStarted, settled = false, lateFinished = false;
  const started = new Promise(resolve => {lateStarted = resolve;});
  const late = new Promise(resolve => {releaseLate = resolve;});
  const primary = Error('first_create_failed'), second = Error('second_create_failed');
  let settings = {limits: {max_sandboxes_per_user: 5}}, revision = 1;
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, emptyOwner: true,
    exclusiveSettings: true, cleanup: async () => {}, ...h.factory,
    intents: [0, 1, 2].map(i => ({...intent, key: 'create-' + i}))};
  h.ctx.fixture = () => ({take: async () => f});
  h.ctx.assert = (ok, code) => assert.ok(ok, code);
  h.ctx.step = async (_label, run) => run();
  h.ctx.request = async ({method, key, body}) => {
    if (method === 'PATCH') {revision++; settings = {...settings, limits: body.limits};}
    if (method !== 'POST') return {status: 200, body: {revision, settings}};
    if (key === 'create-0') throw primary;
    if (key === 'create-1') throw second;
    lateStarted(); await late; lateFinished = true; return accepted;
  };
  const run = perUserLimit.run(h.ctx).then(() => {settled = true;}, error => {settled = true; return error;});
  await started; await new Promise(resolve => setImmediate(resolve));
  const beforeRelease = settled;
  releaseLate();
  const error = await run;
  assert.equal(beforeRelease, false, 'case escaped while a create was still running');
  assert.equal(lateFinished, true);
  assert.deepEqual(error.errors, [primary, second]);
  assert.deepEqual(h.captures, [accepted]);
});
