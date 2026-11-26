import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('./authenticated-mongo-readiness.js', import.meta.url), 'utf8');
const expected = {_id: 'cma-authenticated-contract', members: [{_id: 0, host: '127.0.0.1:27017'}]};
class Exit extends Error {
  constructor(code) { super('probe exited'); this.code = code; }
}

function fixture(overrides = {}) {
  const state = {
    finalProcess: true, authenticated: true, config: null, primary: false,
    replicationOptions: {replSet: 'cma-authenticated-contract'},
    initializationCalls: 0, throwNotInitialized: false, loseInitializeReply: false,
    ...overrides,
  };
  const db = {
    async auth(username, password) {
      assert.equal(username, 'fixture-admin');
      assert.equal(password, 'fixture-password');
    },
    async runCommand(command) {
      assert.equal(command.connectionStatus, 1);
      return {authInfo: {authenticatedUsers: state.authenticated
        ? [{user: 'fixture-admin', db: 'admin'}] : []}};
    },
    async adminCommand(command) {
      if (command.getCmdLineOpts) {
        return {ok: 1, parsed: state.finalProcess
          ? {replication: state.replicationOptions} : {}};
      }
      if (command.replSetGetConfig) {
        if (state.config) return {ok: 1, config: state.config};
        if (state.throwNotInitialized) throw Object.assign(new Error('NotYetInitialized'), {code: 94});
        return {ok: 0, code: 94};
      }
      assert.deepEqual(JSON.parse(JSON.stringify(command)), {replSetInitiate: expected});
      assert.equal(state.config, null, 'never initialize an existing configuration again');
      state.initializationCalls++;
      state.config = structuredClone(expected);
      if (state.loseInitializeReply) throw new Error('connection lost after initialization');
      return {ok: 1};
    },
    async hello() { return {isWritablePrimary: state.primary, setName: 'cma-authenticated-contract'}; },
  };
  return {state, async probe() {
    try {
      await vm.runInNewContext(source, {
        db, process: {env: {MONGO_INITDB_ROOT_USERNAME: 'fixture-admin',
          MONGO_INITDB_ROOT_PASSWORD: 'fixture-password'}},
        quit(code) { throw new Exit(code); },
      }, {timeout: 1000});
      assert.fail('probe must report an explicit exit status');
    } catch (error) {
      if (error instanceof Exit) return error.code;
      throw error;
    }
  }};
}

test('temporary authenticated bootstrap cannot satisfy readiness or initialize a set', async () => {
  const f = fixture({finalProcess: false, primary: true});
  assert.equal(await f.probe(), 1);
  assert.equal(f.state.initializationCalls, 0);
  assert.equal(f.state.config, null);
});

test('final process initializes once and becomes ready only after primary election', async () => {
  for (const throwNotInitialized of [false, true]) {
    const f = fixture({throwNotInitialized});
    assert.equal(await f.probe(), 1);
    assert.equal(f.state.initializationCalls, 1);
    f.state.primary = true;
    assert.equal(await f.probe(), 0);
    assert.equal(f.state.initializationCalls, 1);
  }
});

test('CLI and config replica options are accepted, but missing or conflicting names never initialize', async () => {
  for (const replicationOptions of [
    {replSet: 'cma-authenticated-contract'},
    {replSetName: 'cma-authenticated-contract'},
    {replSet: 'cma-authenticated-contract', replSetName: 'cma-authenticated-contract'},
  ]) {
    const f = fixture({replicationOptions, primary: true});
    assert.equal(await f.probe(), 0);
    assert.equal(f.state.initializationCalls, 1);
  }
  for (const replicationOptions of [
    {}, {replSet: 'foreign'}, {replSetName: 'foreign'},
    {replSet: 'cma-authenticated-contract', replSetName: 'foreign'},
    {replSet: 'foreign', replSetName: 'cma-authenticated-contract'},
    {replSet: 'cma-authenticated-contract/foreign:27017'},
  ]) {
    const f = fixture({replicationOptions, primary: true});
    assert.equal(await f.probe(), 1);
    assert.equal(f.state.initializationCalls, 0);
    assert.equal(f.state.config, null);
  }
});

test('lost initialization response is reconciled without a second initialization', async () => {
  const f = fixture({loseInitializeReply: true});
  await assert.rejects(() => f.probe(), /connection lost/);
  f.state.primary = true;
  assert.equal(await f.probe(), 0);
  assert.equal(f.state.initializationCalls, 1);
});

test('failed authentication and foreign configurations never become ready or get rewritten', async () => {
  const unauthorized = fixture({authenticated: false, primary: true});
  assert.equal(await unauthorized.probe(), 1);
  assert.equal(unauthorized.state.initializationCalls, 0);
  for (const config of [
    {...expected, _id: 'foreign'},
    {...expected, members: [{_id: 0, host: 'foreign:27017'}]},
    {...expected, members: [{_id: 1, host: '127.0.0.1:27017'}]},
    {...expected, members: [...expected.members, {_id: 1, host: 'foreign:27017'}]},
  ]) {
    const f = fixture({config, primary: true});
    assert.equal(await f.probe(), 1);
    assert.deepEqual(f.state.config, config);
    assert.equal(f.state.initializationCalls, 0);
  }
});
