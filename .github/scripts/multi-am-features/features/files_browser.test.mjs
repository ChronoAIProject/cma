import assert from 'node:assert/strict';
import test from 'node:test';
import {removeFixtureDirectory} from './files_browser.mjs';

function fixture({malformed = false} = {}) {
  const path = '/api/v1/agents/fixture-agent/files';
  const directory = 'fixture-folder';
  const calls = [];
  let present = true;
  const api = {request: async (am, method, route, options) => {
    assert.equal(options.direct, true);
    calls.push([am, method]);
    if (method === 'DELETE') {
      assert.equal(route, `${path}?path=${directory}&recursive=true`);
      assert.equal(am, 'b');
      present = false;
      return {status: 200, body: {path: directory, deleted: true}};
    }
    assert.equal(route, `${path}/tree?path=`);
    const entries = am === 'a' && present ? [{path: directory, name: directory, kind: 'directory'}] : [];
    return {body: {path: '', entries: malformed && am === 'b' && !present ? [{path: 42}] : entries}};
  }};
  return {calls, present: () => present, cleanup: acknowledged =>
    removeFixtureDirectory({api, path, directory, ingress: 'a', acknowledged})};
}

test('acknowledged browser creation attempts deletion before checking either listing', async () => {
  const {calls, present, cleanup} = fixture();
  await cleanup(true);
  assert.deepEqual(calls, [['b', 'DELETE'], ['a', 'GET'], ['b', 'GET']]);
  assert.equal(present(), false);
});

test('an ambiguous create is removed even when the peer omits the directory', async () => {
  const {calls, present, cleanup} = fixture();
  await cleanup(false);
  assert.deepEqual(calls, [['a', 'GET'], ['b', 'GET'], ['b', 'DELETE'], ['a', 'GET'], ['b', 'GET']]);
  assert.equal(present(), false);
});

test('a malformed successful listing cannot prove directory absence', async () => {
  const {calls, cleanup} = fixture({malformed: true});
  await assert.rejects(cleanup(true), /browser_files_root_listing_invalid/);
  assert.deepEqual(calls, [['b', 'DELETE'], ['a', 'GET'], ['b', 'GET']]);
});
