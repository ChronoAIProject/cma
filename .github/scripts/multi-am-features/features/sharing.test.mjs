import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import test from 'node:test';
import {artifactCommittedWithoutAcknowledgement, artifactUnknownPublication, revokeWhileRedemptionRetries,
  shareAgentArtifact} from './sharing.mjs';

const fixtureTime = '2026-01-01T00:00:00Z';

function scenario({duplicateRedemption = false, divergingArtifact = false, aborts = 1,
  advancedRevision = true, lostRevision = false, publicationSettlesEarly = false,
  sampledTransaction = 'a'.repeat(64), publicationDeniedCode} = {}) {
  const agentId = 'fixture-agent';
  const calls = [];
  const link = {id: 'fixture-link', agent_id: agentId, mode: 'view', revision: 2};
  let redeemed = 0, revoked = false;
  const artifact = {id: 'fixture-artifact', agent_id: agentId, created_at: fixtureTime};
  let published, finishPublication;
  let preferences = {title: 'Original', revision: 3};
  const firstAdmission = new Map();
  const actions = [];
  const adapter = {ownerOf: async () => ({am: 'a', epoch: 2, agent_id: agentId, pod_uid: 'pod-a'}),
    artifactPublicationRace: async ({action, count, agentId: target}) => {
      actions.push(action);
      assert.equal(target, agentId);
      if (action === 'wait') {
        assert.equal(count, 8);
        assert.equal(typeof finishPublication, 'function');
        return {transaction_sha256: 'a'.repeat(64)};
      }
      if (action === 'sample') {
        setImmediate(finishPublication);
        return {transaction_sha256: sampledTransaction};
      }
      if (action === 'disarm') finishPublication?.();
      return {count: 8 + (action === 'arm' ? 0 : aborts)};
    }};
  const user = {request: async (am, method, route, options) => {
    calls.push([am, method, route]);
    if (route.endsWith('/preferences')) {
      if (method === 'GET') return {body: lostRevision && am === 'b' ? {...preferences, revision: 3} : preferences};
      assert.equal(method, 'PATCH');
      assert.equal(am, 'a');
      assert.equal(options.body.expected_revision, preferences.revision);
      if (publicationSettlesEarly) {
        finishPublication();
        await new Promise(resolve => setImmediate(resolve));
      }
      preferences = {title: options.body.title, revision: preferences.revision + Number(advancedRevision)};
      return {body: preferences};
    }
    if (method === 'POST' && route.endsWith('/artifacts')) {
      if (options.allowed.includes(409)) return {status: 409};
      const input = JSON.stringify(options.body);
      if (firstAdmission.has(options.key)) assert.equal(firstAdmission.get(options.key), input);
      else firstAdmission.set(options.key, input);
      const data = Buffer.from(options.body.content_base64, 'base64');
      const id = divergingArtifact && am === 'a' ? 'duplicate-artifact' : artifact.id;
      if (!published) await new Promise(resolve => { finishPublication = resolve; });
      published = {...artifact, id, name: options.body.name, media_type: options.body.media_type, size_bytes: data.length,
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
    if (method === 'POST' && route.endsWith('/artifacts')) {
      const status = revoked ? 404 : 403;
      assert.deepEqual(options.allowed, [status]);
      return {status, body: {code: revoked ? 'resource_not_found' : publicationDeniedCode ?? 'resource_forbidden'}};
    }
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
  assert.deepEqual(calls.slice(0, 8).map(([am, method, route]) => [am, method, route.split('/').at(-1)]),
    [['a', 'GET', 'preferences'], ['b', 'POST', 'artifacts'], ['a', 'PATCH', 'preferences'],
      ['a', 'GET', 'preferences'], ['a', 'GET', 'artifacts'], ['b', 'GET', 'preferences'], ['b', 'GET', 'artifacts'],
      ['a', 'POST', 'artifacts']]);
  assert.deepEqual(actions, ['arm', 'wait', 'sample', 'disarm']);
  assert.deepEqual(failures, []);
  assert(isRevoked());
});

test('a clean receipt still fails without one observed artifact transaction abort', async () => {
  for (const aborts of [0, 2]) {
    const {run, actions, calls} = scenario({aborts});
    await assert.rejects(run, /artifact_publication_abort_not_consumed_once/);
    assert.deepEqual(actions, ['arm', 'wait', 'sample', 'disarm']);
    assert.equal(calls.length, 3);
  }
});

test('an active viewer must receive the publication-specific access refusal', async () => {
  const {run, isRevoked} = scenario({publicationDeniedCode: 'account_deletion_in_progress'});
  await assert.rejects(run, /artifact_publication_refusal_untyped/);
  assert(isRevoked());
});

test('duplicate redemption fails the case and still revokes the owned fixture link', async () => {
  const {run, failures, isRevoked} = scenario({duplicateRedemption: true});
  await assert.rejects(run, /share_redemption_replayed_twice/);
  assert.deepEqual(failures, [['primary', 'share_redemption_replayed_twice']]);
  assert(isRevoked());
});

test('the retry and peer replay must return one artifact before any share is created', async () => {
  const {run, failures, calls, isRevoked} = scenario({divergingArtifact: true});
  await assert.rejects(run, /artifact_replay_changed_receipt/);
  assert.deepEqual(calls.filter(row => row[1] === 'POST' && row[2].endsWith('/artifacts')).map(row => row[0]), ['b', 'a']);
  assert.deepEqual(failures, []);
  assert.equal(isRevoked(), false);
});

test('artifact publication requires a changed owner revision before retry and both peer readbacks', async () => {
  for (const options of [{advancedRevision: false}, {publicationSettlesEarly: true}]) {
    const {run, actions} = scenario(options);
    await assert.rejects(run, /artifact_owner_revision_did_not_advance_before_retry/);
    assert.deepEqual(actions, ['arm', 'wait', 'disarm']);
  }
  for (const sampledTransaction of [null, 'b'.repeat(64)]) {
    const {run, actions} = scenario({sampledTransaction});
    await assert.rejects(run, /artifact_publication_transaction_changed_during_rename/);
    assert.deepEqual(actions, ['arm', 'wait', 'sample', 'disarm']);
  }
  await assert.rejects(scenario({lostRevision: true}).run(), /artifact_retry_lost_owner_revision/);
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

function committedPublication({duplicate = false, changedReceipt, wrongOriginal} = {}) {
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
      const data = Buffer.from(options.body.content_base64, 'base64');
      published = {id: 'artifact-first', name: options.body.name, agent_id: agentId, created_at: fixtureTime,
        media_type: options.body.media_type, size_bytes: data.length,
        sha256: createHash('sha256').update(data).digest('hex'), ...wrongOriginal};
    } else {
      assert.deepEqual(options, initial);
      if (changedReceipt) return {status: 201, body: {...published, ...changedReceipt}};
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
  for (const changedReceipt of [{id: 'altered-replay'}, {agent_id: 'other-agent'}, {name: 'other.txt'},
    {media_type: 'application/octet-stream'}, {size_bytes: 1}]) {
    const replay = committedPublication({changedReceipt});
    await assert.rejects(replay.run(), /artifact_acknowledgement_replay_changed_receipt/);
    assert.deepEqual(replay.calls, [['b', 'POST'], ['a', 'GET'], ['a', 'POST']]);
  }
  const original = committedPublication({wrongOriginal: {media_type: 'application/octet-stream'}});
  await assert.rejects(original.run(), /artifact_acknowledgement_commit_unbound/);
  assert.deepEqual(original.calls, [['b', 'POST']]);
});

function revocationRace({wrongDenial = false, count = 1} = {}) {
  const actions = [];
  const link = {id: 'fixture-link', agent_id: 'fixture-agent', revision: 3, accepted_count: 0};
  const denied = {status: 404, body: {code: wrongDenial ? 'fixture_failure' : 'resource_not_found'}};
  let resolveRedemption, revoked, revokeKey;
  const adapter = {
    ownerOf: async () => ({am: 'a', epoch: 2, agent_id: link.agent_id, pod_uid: 'pod-a'}),
    shareRedemptionPublicationAbort: async ({action, count: prior}) => {
      actions.push(action);
      if (action === 'wait') {
        assert.equal(prior, 9);
        assert.equal(typeof resolveRedemption, 'function');
        return {entered: true};
      }
      return {count: action === 'arm' ? 9 : 9 + count};
    },
  };
  const user = {
    request: async () => ({body: link}),
    rawRequest: async (am, method, path, {key}) => {
      actions.push(`${am}_${method}`);
      assert.match(path, /fixture-link\?expected_revision=3$/);
      if (!revoked) {
        revokeKey = key;
        revoked = {...link, revoked_at: 'fixture-time', revision: 4};
        setImmediate(() => resolveRedemption(denied));
      }
      assert.equal(key, revokeKey);
      return {body: revoked};
    },
  };
  const outsider = {rawRequest: async (am, method) => {
    actions.push(`${am}_outsider_${method}`);
    if (!resolveRedemption) return new Promise(resolve => { resolveRedemption = resolve; });
    assert(revoked);
    return denied;
  }};
  return {actions, run: () => revokeWhileRedemptionRetries({adapter, user, outsider,
    record: {agentId: link.agent_id}})};
}

test('the held redemption is refused after revocation and both Managers keep the revoked receipt', async () => {
  const fixture = revocationRace();
  await fixture.run();
  assert.deepEqual(fixture.actions, ['arm', 'b_outsider_POST', 'wait', 'a_DELETE', 'disarm',
    'b_outsider_GET', 'b_outsider_POST', 'a_outsider_GET', 'a_outsider_POST', 'b_DELETE']);
});

test('revocation during redemption needs the typed denial and one observed transaction abort', async () => {
  for (const [input, failure] of [[{wrongDenial: true}, /share_redemption_retry_wrong_outcome/],
    [{count: 2}, /share_redemption_fault_not_consumed_once/]]) {
    const fixture = revocationRace(input);
    await assert.rejects(fixture.run(), failure);
    assert.deepEqual(fixture.actions, ['arm', 'b_outsider_POST', 'wait', 'a_DELETE', 'disarm']);
  }
});
