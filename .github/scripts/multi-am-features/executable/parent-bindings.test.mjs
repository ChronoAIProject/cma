import test from 'node:test';
import assert from 'node:assert/strict';
import {disposableCustody, bindParentFixtures} from './parent-bindings.mjs';

const config = {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true, run_id: 'campaign'};

test('document custody records intent before resource and never claims final disposal', async () => {
  const events = [], database = disposableCustody(config, async event => events.push(event));
  const lease = await database.reserve({purpose: 'documents', caseId: 'C275:document'});
  await lease.retainIntent({method: 'POST', path: '/api/v1/documents', key: 'original'});
  await lease.retainDocument('acknowledged-id');
  const release = await lease.release(); await lease.release();
  assert.deepEqual(events.map(event => event.kind), ['disposable_reservation', 'disposable_intent',
    'disposable_document', 'disposable_reservation_closed']);
  assert.equal(release.final_cleanup, 'PENDING_PARENT_DATABASE_DISPOSAL');
  await assert.rejects(lease.retainDocument('late'), /released/);
});

test('failed custody sink cannot silently accept resource tracking', async () => {
  const database = disposableCustody(config, async () => {throw Error('disk_unavailable');});
  await assert.rejects(database.reserve({purpose: 'documents', caseId: 'C275:document'}), /disk_unavailable/);
});

test('parent bindings require exact actor subjects and preserve supplied authentic fixtures', async () => {
  const browser = {}, services = {record: async () => {}, executable: {fixtures: {browser}}};
  const shared = await bindParentFixtures({...config, owner_id: 'owner',
    actors: {recipient: {subject: 'recipient'}, outsider: {subject: 'outsider'}}},
  {services, adapter: {}}, {hasActor: name => name === 'recipient'});
  assert.equal(shared.fixtures.browser, browser);
  assert.deepEqual(shared.primitives.identities, {user: {actor: 'user', subject: 'owner'},
    recipient: {actor: 'recipient', subject: 'recipient'}});
  assert.equal(shared.fixtures['admin-fixtures'], undefined);
});
