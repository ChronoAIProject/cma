import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import test from 'node:test';
import {artifactCommittedWithoutAcknowledgement, artifactUnknownPublication, shareAgentArtifact} from './sharing.mjs';

function scenario({duplicateRedemption = false, divergingArtifact = false, aborts = 1} = {}) {
  const agentId = 'fixture-agent';
  const calls = [];
  const link = {id: 'fixture-link', agent_id: agentId, mode: 'view', revision: 2};
  let redeemed = 0, revoked = false;
  const artifact = {id: 'fixture-artifact', agent_id: agentId, created_at: 'fixture-time'};
  let published;
  const firstAdmission = new Map();
  const actions = [];
  const adapter = {ownerOf: async () => ({am: 'a', epoch: 2, agent_id: agentId, pod_uid: 'pod-a'}),
    artifactPublicationAbort: async ({arm}) => {
      actions.push(arm);
      return {count: 8 + (arm ? 0 : aborts)};
    }};
  const user = {request: async (am, method, route, options) => {
    calls.push([am, method, route]);
    if (method === 'POST' && route.endsWith('/artifacts')) {
      if (options.allowed.includes(409)) return {status: 409};
      const input = JSON.stringify(options.body);
      if (firstAdmission.has(options.key)) assert.equal(firstAdmission.get(options.key), input);
      else firstAdmission.set(options.key, input);
      const data = Buffer.from(options.body.content_base64, 'base64');
      const id = divergingArtifact && am === 'a' ? 'duplicate-artifact' : artifact.id;
      published = {...artifact, id, name: options.body.name, size_bytes: data.length,
        sha256: createHash('sha256').update(data).digest('hex')};
      return {body: published};
    }
    if (method === 'POST' && route.endsWith('/share-links')) return {body: link};
    if (method === 'GET' && route.endsWith('/share-links')) return {body: {items: [link]}};
    if (method === 'GET' && route.endsWith('/artifacts')) return {body: {items: [published]}};
    assert.equal(method, 'DELETE');
    assert.match(route, /fixture-link\?expected_revision=2$/);
    revoked = true;
    return {body: {...link, revoked_at: 'fixture-time'}};
  }};
  user.rawRequest = user.request;
  const outsider = {request: async (am, method, route, options) => {
    if (options?.allowed?.includes(404)) {
      if (redeemed === 2 && method === 'GET') assert(revoked);
      return {status: 404};
    }
    if (method === 'GET' && route.includes('/resource-access/')) {
      assert(!revoked);
      return {body: {capabilities: ['view_files']}};
    }
    if (method === 'GET' && route.endsWith('/' + artifact.id)) {
      assert(!revoked);
      return {body: published};
    }
    assert.equal(method, 'POST');
    assert.match(route, /fixture-link\/redeem$/);
    redeemed += 1;
    return {body: {...link, accepted_count: duplicateRedemption ? redeemed : 1}};
  }};
  outsider.rawRequest = outsider.request;
  const failures = [];
  const run = () => shareAgentArtifact({adapter, user, outsider, record: {agentId},
    recordFailure: async (stage, error) => failures.push([stage, error.message])});
  return {run, failures, calls, actions, isRevoked: () => revoked};
}

test('both ingresses share one artifact receipt and redeem the same link only once', async () => {
  const {run, failures, calls, actions, isRevoked} = scenario();
  await run();
  assert.deepEqual(calls.slice(0, 2).map(row => row[0]), ['b', 'a']);
  assert.deepEqual(actions, [true, false]);
  assert.deepEqual(failures, []);
  assert(isRevoked());
});

test('a clean receipt still fails without one observed artifact transaction abort', async () => {
  for (const aborts of [0, 2]) {
    const {run, actions, calls} = scenario({aborts});
    await assert.rejects(run, /artifact_publication_abort_not_consumed_once/);
    assert.deepEqual(actions, [true, false]);
    assert(calls.every(([, , route]) => route.endsWith('/artifacts')));
  }
});

test('duplicate redemption fails the case and still revokes the owned fixture link', async () => {
  const {run, failures, calls, isRevoked} = scenario({duplicateRedemption: true});
  await assert.rejects(run, /share_redemption_replayed_twice/);
  assert.deepEqual(failures, [['primary', 'share_redemption_replayed_twice']]);
  assert.deepEqual(calls.slice(0, 2).map(row => row[0]), ['b', 'a']);
  assert(isRevoked());
});

test('competing uploads must return one artifact before any share is created', async () => {
  const {run, failures, calls, isRevoked} = scenario({divergingArtifact: true});
  await assert.rejects(run, /artifact_concurrent_receipt_diverged/);
  assert.deepEqual(calls.map(row => row[0]), ['b', 'a']);
  assert.deepEqual(failures, []);
  assert.equal(isRevoked(), false);
});

function unknownPublication({publishedBeforeRetry = false, wrongProblem = false, divergingReceipt = false} = {}) {
  const agentId = 'fixture-agent';
  const actions = [], posts = [], states = [];
  let artifact;
  const adapter = {
    ownerOf: async () => ({am: 'a', epoch: 2, agent_id: agentId, pod_uid: 'pod-a'}),
    artifactPublicationAbort: async ({arm, outcome}) => {
      actions.push([arm, outcome]);
      return {count: arm ? 5 : 6};
    },
  };
  const user = {rawRequest: async (am, method, path, options) => {
    assert.equal(path, '/api/v1/agents/fixture-agent/artifacts');
    if (method === 'GET') return {body: {items: artifact ? [artifact] : []}};
    assert.equal(method, 'POST');
    posts.push({am, ...options});
    const published = {id: 'artifact-first', name: options.body.name, agent_id: agentId,
      sha256: createHash('sha256').update(Buffer.from(options.body.content_base64, 'base64')).digest('hex'),
      created_at: 'fixture-time'};
    if (posts.length === 1) {
      if (publishedBeforeRetry) artifact = published;
      return {status: 503, body: {code: wrongProblem ? 'other_error' : 'resource_access_unavailable'}};
    }
    artifact ??= published;
    return {status: 201, body: posts.length === 3 && divergingReceipt ? {...artifact, id: 'second'} : artifact};
  }};
  return {actions, posts, states, run: () => artifactUnknownPublication({adapter, user, record: {agentId},
    recordState: async (name, state) => states.push([name, state])})};
}

test('unknown artifact publication stays unresolved until explicit replay of the exact original key across both ingresses', async () => {
  const scenario = unknownPublication();
  await scenario.run();
  assert.deepEqual(scenario.posts.map(row => [row.am, row.allowed[0]]), [['b', 503], ['a', 201], ['b', 201]]);
  assert(scenario.posts.every(row => row.key === scenario.posts[0].key && row.body === scenario.posts[0].body));
  assert.deepEqual(scenario.actions, [[true, 'unknown'], [false, undefined]]);
  assert.deepEqual(scenario.states, [['artifact_unknown_publication', {unacknowledged_writes: 1, recovered_artifacts: 1}]]);
});

test('unknown artifact publication rejects an extra effect or incorrect uncertainty classification', async () => {
  for (const [input, error, requests] of [
    [{publishedBeforeRetry: true}, /artifact_unknown_published_without_acknowledgement/, 1],
    [{wrongProblem: true}, /artifact_unknown_mislabeled_or_retried/, 1],
    [{divergingReceipt: true}, /artifact_unknown_same_key_receipt_unbound/, 3],
  ]) {
    const scenario = unknownPublication(input);
    await assert.rejects(scenario.run(), error);
    assert.equal(scenario.posts.length, requests);
    assert.deepEqual(scenario.actions, [[true, 'unknown'], [false, undefined]]);
  }
});

function committedPublication({duplicate = false, changedReceipt = false} = {}) {
  const agentId = 'fixture-agent';
  const calls = [];
  let initial, published;
  const adapter = {ownerOf: async () => ({am: 'a', epoch: 2, agent_id: agentId, pod_uid: 'pod-a'})};
  const user = {rawRequest: async (am, method, path, options) => {
    assert.equal(path, '/api/v1/agents/fixture-agent/artifacts');
    calls.push([am, method]);
    if (method === 'GET') {
      const items = duplicate ? [published, {...published, id: 'second'}] : [published];
      return {body: {items}};
    }
    assert.equal(method, 'POST');
    if (!initial) {
      initial = options;
      published = {id: 'artifact-first', name: options.body.name, agent_id: agentId, created_at: 'fixture-time',
        sha256: createHash('sha256').update(Buffer.from(options.body.content_base64, 'base64')).digest('hex')};
    } else {
      assert.deepEqual(options, initial);
      if (changedReceipt) return {status: 201, body: {...published, id: 'altered-replay'}};
    }
    return {status: 201, body: published};
  }};
  return {calls, run: () => artifactCommittedWithoutAcknowledgement({adapter, user, record: {agentId}})};
}

test('a caller disconnect after commit recovers the original receipt through the other Manager', async () => {
  const scenario = committedPublication();
  await scenario.run();
  assert.deepEqual(scenario.calls, [['b', 'POST'], ['a', 'GET'], ['a', 'POST'], ['b', 'GET']]);
});

test('a lost acknowledgement fails if the commit duplicates the effect or the replay changes its receipt', async () => {
  const duplicate = committedPublication({duplicate: true});
  await assert.rejects(duplicate.run(), /artifact_acknowledgement_effect_missing_or_duplicated/);
  assert.deepEqual(duplicate.calls, [['b', 'POST'], ['a', 'GET']]);
  const replay = committedPublication({changedReceipt: true});
  await assert.rejects(replay.run(), /artifact_acknowledgement_replay_changed_receipt/);
  assert.deepEqual(replay.calls, [['b', 'POST'], ['a', 'GET'], ['a', 'POST']]);
});
