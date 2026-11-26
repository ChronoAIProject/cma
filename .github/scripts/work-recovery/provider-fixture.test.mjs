import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';
import { startProviderFixture } from './provider-fixture.mjs';
import { fixtureIdentity, scopePlan, validateMint } from './provider-protocol.mjs';
import { barrier, call, fixture, mintBody } from './provider-test-support.mjs';

const expiry = JSON.parse(readFileSync(new URL('./fixtures/mint-expiry.json', import.meta.url), 'utf8'));
const fixedNow = Date.parse('2039-01-01T00:00:00.000Z');
const invalidExpiries = [null, 2208988800, '', 'not-a-date', '2040-99-01T00:00:00Z',
  '2040-01-01T00:00:00.Z', '2040-01-01T00:00:00.1234Z', '2040-01-01T00:00:00+00:00',
  '2038-12-31T23:59:59.999Z', '2039-01-01T00:00:00Z'];

for (const [milliseconds, expires_at] of expiry.cases) {
  test(`mint expiry accepts the production timestamp at ${milliseconds} milliseconds`, t => {
    t.mock.method(Date, 'now', () => fixedNow);
    const body = { ...mintBody(), expires_at };
    assert.equal(validateMint(body), body);
    assert.equal(Date.parse(expires_at), expiry.base_unix_seconds * 1000 + milliseconds);
  });
}

test('mint expiry keeps strict UTC millisecond format and future-only boundaries', t => {
  t.mock.method(Date, 'now', () => fixedNow);
  for (const expires_at of invalidExpiries) {
    assert.throws(() => validateMint({ ...mintBody(), expires_at }), /unsupported_mint_request/);
  }
  for (const fraction of ['001', '01', '1']) {
    const body = { ...mintBody(), expires_at: `2039-01-01T00:00:00.${fraction}Z` };
    assert.equal(validateMint(body), body);
  }
});

test('HTTP fixture accepts production timestamp precision with one effect per POST', async t => {
  t.mock.method(Date, 'now', () => fixedNow);
  const server = await fixture(t);
  for (const [, expires_at] of expiry.cases) {
    const response = await call(server, 'POST', '/api/v1/api-keys', { ...mintBody(), expires_at }).result;
    assert.equal(response.status, 200, expires_at);
  }
  const ledger = server.snapshot();
  for (const count of ['received_mint_posts', 'accepted_calls', 'physical_commits', 'distinct_effects']) {
    assert.equal(ledger[count], expiry.cases.length);
  }
  assert.equal(ledger.refusals, 0);
  assert.equal(ledger.replays, 0);
});

test('preparation barrier holds the actual GET without sending a mint', async t => {
  const server = await fixture(t);
  server.configure({ preparation: 'hold' });
  const response = call(server, 'GET', '/api/v1/user-services');
  const request = await barrier(server, 'preparation_held');
  assert.equal(server.snapshot().received_mint_posts, 0);
  server.release(request);
  const result = await response.result;
  assert.equal(result.status, 200);
  assert.deepEqual(JSON.parse(result.text), [{ id: fixtureIdentity.service,
    slug: fixtureIdentity.slug, is_active: true }]);
  assert.equal(server.snapshot().physical_commits, 0);
});

test('scope preparation binds the selected service; extra authority is refused', async t => {
  const server = await fixture(t);
  const selected = { selected_service_ids: [fixtureIdentity.service] };
  const response = await call(server, 'POST', '/api/v1/api-keys/scope-plan', selected).result;
  const plan = JSON.parse(response.text);
  assert.equal(response.status, 200);
  assert.equal(plan.normalized_grant_digest, scopePlan(selected).normalized_grant_digest);
  assert.deepEqual(plan.allowed_node_ids, [fixtureIdentity.node]);
  for (const body of [{ ...selected, target_org_id: 'another-owner' },
    { selected_service_ids: [] }, { selected_service_ids: ['unconfigured'] }]) {
    assert.equal((await call(server, 'POST', '/api/v1/api-keys/scope-plan', body).result).status, 400);
  }
  assert.equal(server.snapshot().received_mint_posts, 0);
});

test('independent concurrent duplicate POSTs produce two physical effects', async t => {
  const server = await fixture(t);
  const body = mintBody();
  const results = await Promise.all([call(server, 'POST', '/api/v1/api-keys', body).result,
    call(server, 'POST', '/api/v1/api-keys', body).result]);
  assert.deepEqual(results.map(result => result.status), [200, 200]);
  const keys = results.map(result => JSON.parse(result.text));
  assert.notEqual(keys[0].id, keys[1].id);
  assert.notEqual(keys[0].full_key, keys[1].full_key);
  const ledger = server.snapshot();
  assert.equal(ledger.received_mint_posts, 2);
  assert.equal(ledger.accepted_calls, 2);
  assert.equal(ledger.physical_commits, 2);
  assert.equal(ledger.distinct_effects, 2);
  assert.equal(ledger.replays, 0);
  assert.equal(ledger.refusals, 0);
  const events = ledger.events.filter(event => event.kind === 'mint_accepted');
  assert.equal(events[0].operation_hash, events[1].operation_hash);
  assert.notEqual(events[0].effect_hash, events[1].effect_hash);
  const retained = readFileSync(join(server.root, 'provider.jsonl'), 'utf8');
  assert(!retained.includes(server.bearer));
  for (const key of keys) assert(!retained.includes(key.full_key));
  assert(!JSON.stringify(ledger).includes(body.name));
  assert(!JSON.stringify(ledger).includes(keys[0].id));
});

test('accepted result can be lost while metadata and durable effect count survive', async t => {
  const server = await fixture(t);
  server.configure({ mint: 'hold_response' });
  const body = mintBody();
  const response = call(server, 'POST', '/api/v1/api-keys', body);
  const request = await barrier(server, 'response_held');
  assert.equal(server.snapshot().physical_commits, 1);
  response.abort();
  assert.equal((await response.result).transportError, true);
  server.release(request, 'drop');
  const listing = JSON.parse((await call(server, 'GET', '/api/v1/api-keys').result).text);
  assert.equal(listing.keys.length, 1);
  assert.equal(listing.keys[0].name, body.name);
  assert.deepEqual(Object.keys(listing.keys[0]).sort(), ['id', 'name']);
  await server.close();
  const reopened = await startProviderFixture({ root: server.root, bearer: server.bearer });
  try {
    assert.equal(reopened.snapshot().accepted_calls, 1);
    const after = JSON.parse((await call({ ...reopened, bearer: server.bearer },
      'GET', '/api/v1/api-keys').result).text);
    assert.deepEqual(after, listing);
    assert.equal(reopened.snapshot().received_mint_posts, 1);
  } finally { await reopened.close(); }
});

test('absent listing after client death cannot rule out late provider acceptance', async t => {
  const server = await fixture(t);
  server.configure({ mint: 'hold_acceptance' });
  const response = call(server, 'POST', '/api/v1/api-keys', mintBody());
  const request = await barrier(server, 'acceptance_held');
  response.abort();
  await response.result;
  const before = JSON.parse((await call(server, 'GET', '/api/v1/api-keys').result).text);
  assert.deepEqual(before.keys, []);
  assert.equal(server.snapshot().physical_commits, 0);
  server.release(request);
  const after = JSON.parse((await call(server, 'GET', '/api/v1/api-keys').result).text);
  assert.equal(after.keys.length, 1);
  assert.equal(server.snapshot().physical_commits, 1);
  assert(server.snapshot().events.some(event => event.kind === 'response_dropped'));
});

test('empty, malformed and failed observations preserve accepted effects', async t => {
  const server = await fixture(t);
  await call(server, 'POST', '/api/v1/api-keys', mintBody()).result;
  for (const mode of ['empty', 'malformed', 'unavailable']) {
    server.configure({ listing: mode });
    const response = await call(server, 'GET', '/api/v1/api-keys').result;
    assert.equal(response.status, mode === 'unavailable' ? 503 : 200);
    if (mode === 'empty') assert.deepEqual(JSON.parse(response.text), { keys: [] });
    if (mode === 'malformed') assert.deepEqual(JSON.parse(response.text), { keys: null });
    assert.equal(server.snapshot().physical_commits, 1);
  }
  server.configure({ listing: 'metadata' });
  assert.equal(JSON.parse((await call(server, 'GET', '/api/v1/api-keys').result).text).keys.length, 1);
});

test('known-key revocation does not close a held late mint', async t => {
  const server = await fixture(t);
  const first = JSON.parse((await call(server, 'POST', '/api/v1/api-keys', mintBody()).result).text);
  server.configure({ mint: 'hold_acceptance' });
  const late = call(server, 'POST', '/api/v1/api-keys', mintBody());
  const request = await barrier(server, 'acceptance_held');
  assert.equal((await call(server, 'DELETE', `/api/v1/api-keys/${first.id}`).result).status, 204);
  assert.equal((await call(server, 'DELETE', `/api/v1/api-keys/${first.id}`).result).status, 404);
  assert.deepEqual(JSON.parse((await call(server, 'GET', '/api/v1/api-keys').result).text).keys, []);
  server.release(request);
  assert.equal((await late.result).status, 200);
  assert.equal(server.snapshot().physical_commits, 2);
});

test('invalid grants, malformed requests and wrong bearer never create an effect', async t => {
  t.mock.method(Date, 'now', () => fixedNow);
  const server = await fixture(t);
  const base = { ...mintBody(), expires_at: '2040-01-01T00:00:00.12Z' };
  const invalid = [null, [], '{', { ...base, name: 'not-a-fixture' },
    { ...base, scopes: 'admin' }, { ...base, allowed_service_ids: ['unconfigured'] },
    { ...base, allow_all_services: true }, { ...base, allow_auto_connected_services: true },
    { ...base, allow_all_nodes: true }, { ...base, allowed_node_ids: [] },
    { ...base, allowed_node_ids: ['unconfigured'] }, { ...base, scope_plan_digest: 'unbound' },
    ...invalidExpiries.map(expires_at => ({ ...base, expires_at })),
    { ...base, extra: true }];
  for (const body of invalid) {
    assert.equal((await call(server, 'POST', '/api/v1/api-keys', body).result).status, 400);
  }
  assert.equal((await call(server, 'POST', '/api/v1/api-keys', base,
    randomBytes(32).toString('hex')).result).status, 401);
  assert.equal(server.snapshot().received_mint_posts, invalid.length + 1);
  assert.equal(server.snapshot().refusals, invalid.length + 1);
  assert.equal(server.snapshot().physical_commits, 0);
});

test('controls reject invalid and repeated release without changing the ledger', async t => {
  const server = await fixture(t);
  for (const value of [null, [], { mint: 'deduplicate' }, { upstream: 'https://example.invalid' }]) {
    assert.throws(() => server.configure(value), /invalid_fixture_mode/);
  }
  server.configure({ mint: 'hold_response' });
  const response = call(server, 'POST', '/api/v1/api-keys', mintBody());
  const request = await barrier(server, 'response_held');
  assert.throws(() => server.release(request, 'retry'), /invalid_fixture_release/);
  server.release(request);
  assert.equal((await response.result).status, 200);
  assert.throws(() => server.release(request), /invalid_fixture_release/);
  assert.equal(server.snapshot().accepted_calls, 1);
});

test('closing fixture retains pending work and destroys held connections', async t => {
  const server = await fixture(t);
  server.configure({ mint: 'hold_acceptance' });
  const response = call(server, 'POST', '/api/v1/api-keys', mintBody());
  const request = await barrier(server, 'acceptance_held');
  const receipt = await server.close();
  assert.deepEqual(receipt.pending_at_close, [{ request, phase: 'acceptance_held' }]);
  assert.equal(receipt.physical_commits, 0);
  assert.equal((await response.result).transportError, true);
});

test('hold deadline drops response without erasing an accepted effect', async t => {
  const server = await fixture(t, { holdMs: 50 });
  server.configure({ mint: 'hold_response' });
  const response = call(server, 'POST', '/api/v1/api-keys', mintBody());
  await barrier(server, 'response_held');
  assert.equal((await response.result).transportError, true);
  const ledger = server.snapshot();
  assert.equal(ledger.physical_commits, 1);
  assert.equal(ledger.pending.length, 0);
  assert(ledger.events.some(event => event.kind === 'response_dropped'));
});
