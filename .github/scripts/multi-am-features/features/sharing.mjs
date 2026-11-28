import {createHash, randomBytes, randomUUID} from 'node:crypto';
import {once} from 'node:events';
import {createServer} from 'node:http';
import {isDeepStrictEqual as equal} from 'node:util';
import {assertOwner, check, id, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

async function competingRequests(...requests) {
  const results = await Promise.allSettled(requests);
  const failure = results.find(result => result.status === 'rejected');
  if (failure) throw failure.reason;
  return results.map(result => result.value.body);
}

function matchesArtifact(artifact, agentId, input, data) {
  return id(artifact?.id) && artifact.agent_id === agentId && artifact.name === input.name
    && artifact.media_type === input.media_type && artifact.size_bytes === data.length
    && artifact.sha256 === createHash('sha256').update(data).digest('hex')
    && Number.isFinite(Date.parse(artifact.created_at));
}

export async function shareAgentArtifact({adapter, user, outsider, record, recordFailure, recordState = async () => {}}) {
  const {agentId} = record;
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(owner.am);
  const agentPath = `/api/v1/agents/${encodeURIComponent(agentId)}`;
  const access = `/api/v1/resource-access/agent/${encodeURIComponent(agentId)}`;
  const links = agentPath + '/share-links';
  const artifacts = agentPath + '/artifacts';
  const data = Buffer.from('Synthetic share and artifact regression\n');
  const artifactInput = {name: `fixture-${randomUUID()}.txt`, media_type: 'text/plain',
    content_base64: data.toString('base64')};
  const artifactIntent = {body: artifactInput, key: randomUUID(), allowed: [201]};
  const preferences = agentPath + '/preferences';
  const {body: before} = await user.rawRequest(owner.am, 'GET', preferences);
  check(Number.isSafeInteger(before?.revision), 'artifact_race_baseline_unbound');
  const title = `Artifact retry ${randomUUID()}`;
  const fault = (action, count) => adapter.artifactPublicationRace({action, count, agentId});
  let pending, armed, disarmed, advanced, artifact, holding;
  let settled = false;
  await withRestoration(async () => {
    armed = await fault('arm');
    check(Number.isSafeInteger(armed?.count), 'artifact_race_fault_count_missing');
    pending = user.rawRequest(ingress, 'POST', artifacts, artifactIntent)
      .then(response => ({response}), error => ({error})).finally(() => { settled = true; });
    holding = await poll(() => fault('wait', armed.count), value => /^[a-f0-9]{64}$/.test(value?.transaction_sha256),
      {timeout: 12000, interval: 100, code: 'artifact_publication_transaction_not_held'});
    const rename = await user.rawRequest(owner.am, 'PATCH', preferences, {
      key: randomUUID(), body: {title, expected_revision: before.revision},
    });
    advanced = rename.body;
    if (advanced?.title === title) record.writerTitle = {title, revision: advanced.revision};
    check(advanced?.title === title && advanced.revision === before.revision + 1 && !settled,
      'artifact_owner_revision_did_not_advance_before_retry');
    const afterRename = await fault('sample');
    check(afterRename?.transaction_sha256 === holding.transaction_sha256,
      'artifact_publication_transaction_changed_during_rename');
    const published = await pending;
    if (published.error) throw published.error;
    artifact = published.response.body;
  }, async () => {
    try { disarmed = await fault('disarm'); }
    finally { await pending; }
  }, recordFailure);
  check(Number.isSafeInteger(armed?.count) && disarmed?.count === armed.count + 1,
    'artifact_publication_abort_not_consumed_once');
  check(matchesArtifact(artifact, agentId, artifactInput, data), 'artifact_admission_identity');
  const readArtifact = async am => {
    const {body: listing} = await user.request(am, 'GET', artifacts);
    check(Array.isArray(listing?.items) && listing.next === undefined, 'artifact_retry_listing_unbound');
    const matches = listing.items.filter(row => row.id === artifact.id || row.name === artifactInput.name);
    check(matches.length === 1 && equal(matches[0], artifact), 'artifact_effect_missing_or_duplicated');
  };
  for (const am of [owner.am, ingress]) {
    const {body: current} = await user.rawRequest(am, 'GET', preferences);
    check(current?.revision === advanced.revision && current.title === title,
      'artifact_retry_lost_owner_revision');
    await readArtifact(am);
  }
  const artifactPath = artifacts + '/' + encodeURIComponent(artifact.id);
  const {body: replay} = await user.request(owner.am, 'POST', artifacts, artifactIntent);
  check(equal(replay, artifact), 'artifact_replay_changed_receipt');
  await user.request(owner.am, 'POST', artifacts,
    {...artifactIntent, body: {...artifactInput, name: 'changed.txt'}, allowed: [409]});
  await outsider.request(ingress, 'GET', access, {allowed: [404]});

  const token = randomBytes(32).toString('hex');
  const {body: link} = await user.request(ingress, 'POST', links, {
    body: {audience: {kind: 'public'}, mode: 'view', token}, key: randomUUID(),
  });
  check(typeof link.id === 'string' && link.agent_id === agentId, 'share_link_admission_identity');
  const redeem = '/api/v1/agent-share-links/' + encodeURIComponent(link.id) + '/redeem';
  const publicationDenied = async (am, status, code) => {
    const {body} = await outsider.request(am, 'POST', artifacts,
      {body: artifactInput, key: randomUUID(), allowed: [status]});
    check(body?.code === code, 'artifact_publication_refusal_untyped');
  };
  await withRestoration(async () => {
    check(link.mode === 'view' && !JSON.stringify(link).includes(token), 'share_link_scope_or_token');
    await outsider.request(owner.am, 'POST', redeem, {
      body: {token: randomBytes(32).toString('hex')}, allowed: [404],
    });
    const [accepted, repeated] = await competingRequests(
      outsider.rawRequest(owner.am, 'POST', redeem, {body: {token}}),
      outsider.rawRequest(ingress, 'POST', redeem, {body: {token}}),
    );
    check(accepted.id === link.id && repeated.id === link.id && accepted.accepted_count === 1 &&
      repeated.accepted_count === 1 && repeated.revision === accepted.revision, 'share_redemption_replayed_twice');
    for (const am of [owner.am, ingress]) {
      const {body: rights} = await outsider.request(am, 'GET', access);
      check(rights.capabilities.includes('view_files') && !rights.capabilities.includes('manage_sharing'),
        'share_view_capability_mismatch');
      const {body: seen} = await outsider.request(am, 'GET', artifactPath);
      check(seen.id === artifact.id && seen.sha256 === artifact.sha256, 'share_artifact_peer_readback');
    }
    await publicationDenied(ingress, 403, 'resource_forbidden');
  }, async () => {
    const {body: listing} = await user.request(owner.am, 'GET', links);
    const current = listing.items.find(row => row.id === link.id);
    check(Number.isSafeInteger(current?.revision), 'share_link_cleanup_revision_missing');
    await user.request(ingress, 'DELETE', links + '/' + encodeURIComponent(link.id) +
      '?expected_revision=' + current.revision, {key: randomUUID()});
    for (const am of [owner.am, ingress]) {
      await outsider.request(am, 'GET', access, {allowed: [404]});
      await outsider.request(am, 'GET', artifactPath, {allowed: [404]});
    }
    await outsider.request(owner.am, 'POST', redeem, {body: {token}, allowed: [404]});
    await publicationDenied(owner.am, 404, 'resource_not_found');
  }, recordFailure);
  await readArtifact(ingress);
  await recordState('artifact_publication_retry', {known_aborts: 1, artifacts: 1,
    owner: owner.am, ingress, before_revision: before.revision, confirmed_revision: advanced.revision,
    transaction_sha256: holding.transaction_sha256});
}

export async function artifactUnknownPublication({adapter, user, record, recordFailure, recordState = async () => {}}) {
  const {agentId} = record;
  const path = `/api/v1/agents/${encodeURIComponent(agentId)}/artifacts`;
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(owner.am);
  const data = Buffer.from('Synthetic unacknowledged artifact publication\n');
  const intent = {key: randomUUID(), body: {name: `fixture-${randomUUID()}.txt`, media_type: 'text/plain',
    content_base64: data.toString('base64')}};
  const matching = async am => {
    const {body} = await user.rawRequest(am, 'GET', path);
    check(Array.isArray(body?.items) && body.next === undefined, 'artifact_unknown_listing_unbound');
    return body.items.filter(row => row.name === intent.body.name);
  };
  let armed, disarmed;
  await withRestoration(async () => {
    armed = await adapter.artifactPublicationAbort({arm: true, outcome: 'unknown'});
    const response = await user.rawRequest(ingress, 'POST', path, {...intent, allowed: [503]});
    check(response.status === 503 && response.body?.code === 'resource_access_unavailable',
      'artifact_unknown_mislabeled_or_retried');
  }, async () => {
    disarmed = await adapter.artifactPublicationAbort({arm: false});
  }, recordFailure);
  check(Number.isSafeInteger(armed?.count) && disarmed?.count === armed.count + 1,
    'artifact_unknown_fault_not_consumed_once');
  for (const am of [ingress, owner.am]) {
    check((await matching(am)).length === 0, 'artifact_unknown_published_without_acknowledgement');
  }
  const {body: recovered} = await user.rawRequest(owner.am, 'POST', path, {...intent, allowed: [201]});
  const {body: replay} = await user.rawRequest(ingress, 'POST', path, {...intent, allowed: [201]});
  const digest = createHash('sha256').update(data).digest('hex');
  check(typeof recovered?.id === 'string' && recovered.agent_id === agentId && recovered.sha256 === digest
    && replay?.id === recovered.id && replay.created_at === recovered.created_at && replay.sha256 === digest,
  'artifact_unknown_same_key_receipt_unbound');
  for (const am of [ingress, owner.am]) {
    const rows = await matching(am);
    check(rows.length === 1 && rows[0].id === recovered.id && rows[0].sha256 === digest,
      'artifact_unknown_recovery_effect_missing_or_duplicated');
  }
  await recordState('artifact_unknown_publication', {unacknowledged_writes: 1, recovered_artifacts: 1});
}

export async function artifactCommittedWithoutAcknowledgement({adapter, user, record, recordFailure,
  recordState = async () => {}}) {
  const {agentId} = record;
  const path = `/api/v1/agents/${encodeURIComponent(agentId)}/artifacts`;
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(owner.am);
  const data = Buffer.from('Synthetic artifact with a disconnected caller\n');
  const intent = {key: randomUUID(), body: {name: `fixture-${randomUUID()}.txt`, media_type: 'text/plain',
    content_base64: data.toString('base64')}, allowed: [201]};
  let original;
  // The loopback relay observes the upstream response and cuts the caller's
  // connection before delivering the acknowledgement. It forwards one write.
  const relay = createServer((request, response) => {
    if (original || request.method !== 'POST' || request.url !== '/') {
      response.writeHead(400).end();
      return;
    }
    original = user.rawRequest(ingress, 'POST', path, intent);
    const disconnect = () => response.destroy();
    void original.then(disconnect, disconnect);
  });
  let committed;
  await withRestoration(async () => {
    relay.listen(0, '127.0.0.1');
    await once(relay, 'listening');
    let callerError;
    try {
      await fetch(`http://127.0.0.1:${relay.address().port}/`, {method: 'POST', signal: AbortSignal.timeout(35000)});
    } catch (error) { callerError = error; }
    check(original, 'artifact_acknowledgement_write_not_dispatched');
    const response = await original;
    check(callerError?.cause?.code === 'UND_ERR_SOCKET' && response.status === 201,
      'artifact_acknowledgement_disconnect_unproven');
    committed = response.body;
    check(matchesArtifact(committed, agentId, intent.body, data),
      'artifact_acknowledgement_commit_unbound');
  }, async () => {
    if (!relay.listening) return;
    await new Promise((resolve, reject) => relay.close(error => error ? reject(error) : resolve()));
  }, recordFailure);
  const readOnce = async am => {
    const {body} = await user.rawRequest(am, 'GET', path);
    check(Array.isArray(body?.items) && body.next === undefined, 'artifact_acknowledgement_listing_unbound');
    const rows = body.items.filter(row => row.name === intent.body.name || row.id === committed.id);
    check(rows.length === 1 && equal(rows[0], committed),
      'artifact_acknowledgement_effect_missing_or_duplicated');
  };
  await readOnce(owner.am);
  const {body: recovered} = await user.rawRequest(owner.am, 'POST', path, intent);
  check(equal(recovered, committed),
    'artifact_acknowledgement_replay_changed_receipt');
  await readOnce(ingress);
  await recordState('artifact_committed_without_acknowledgement', {upstream_commits_observed: 1, recovered_artifacts: 1});
}

export async function revokeWhileRedemptionRetries({adapter, user, outsider, record, recordFailure,
  recordState = async () => {}}) {
  const {agentId} = record;
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(owner.am);
  const links = `/api/v1/agents/${encodeURIComponent(agentId)}/share-links`;
  const access = `/api/v1/resource-access/agent/${encodeURIComponent(agentId)}`;
  const token = randomBytes(32).toString('hex');
  const {body: link} = await user.request(ingress, 'POST', links, {
    body: {audience: {kind: 'public'}, mode: 'view', token}, key: randomUUID(),
  });
  check(typeof link?.id === 'string' && Number.isSafeInteger(link.revision), 'share_revocation_link_unbound');
  const path = `${links}/${encodeURIComponent(link.id)}`;
  const redeemPath = `/api/v1/agent-share-links/${encodeURIComponent(link.id)}/redeem`;
  const revoke = (am, revision, key) => user.rawRequest(am, 'DELETE', `${path}?expected_revision=${revision}`, {key});
  const fault = (action, count) => adapter.shareRedemptionPublicationAbort({action, count});
  let revoked = false;
  const revokeKey = randomUUID();
  await withRestoration(async () => {
    check(link.agent_id === agentId && link.accepted_count === 0 && !link.revoked_at, 'share_revocation_baseline_unbound');
    let pending, settled = false, armed, disarmed;
    await withRestoration(async () => {
      armed = await fault('arm');
      check(Number.isSafeInteger(armed?.count), 'share_revocation_fault_count_missing');
      pending = outsider.rawRequest(ingress, 'POST', redeemPath, {body: {token}, allowed: [404]})
        .then(response => ({response}), error => ({error})).finally(() => { settled = true; });
      await poll(() => fault('wait', armed.count), value => value.entered === true,
        {timeout: 12000, interval: 100, code: 'share_redemption_transaction_not_held'});
      const {body: result} = await revoke(owner.am, link.revision, revokeKey);
      revoked = result?.id === link.id && Boolean(result.revoked_at);
      check(revoked && result.revision === link.revision + 1 && result.accepted_count === 0 && !settled,
        'share_revocation_did_not_win_before_retry');
      const denial = await pending;
      if (denial.error) throw denial.error;
      check(denial.response?.status === 404 && denial.response.body?.code === 'resource_not_found',
        'share_redemption_retry_wrong_outcome');
    }, async () => {
      try { disarmed = await fault('disarm'); }
      finally { await pending; }
    }, recordFailure);
    check(disarmed?.count === armed.count + 1, 'share_redemption_fault_not_consumed_once');
    for (const am of [ingress, owner.am]) {
      await outsider.rawRequest(am, 'GET', access, {allowed: [404]});
      await outsider.rawRequest(am, 'POST', redeemPath, {body: {token}, allowed: [404]});
    }
    const {body: replay} = await revoke(ingress, link.revision, revokeKey);
    check(replay?.id === link.id && replay.revision === link.revision + 1 && replay.accepted_count === 0
      && replay.revoked_at, 'share_revocation_replay_diverged');
    await recordState('share_revoked_before_redemption_retry', {known_aborts: 1, accepted_recipients: 0});
  }, async () => {
    if (revoked) return;
    const {body: listing} = await user.request(owner.am, 'GET', links);
    const current = listing.items.find(row => row.id === link.id);
    if (current && !current.revoked_at) await revoke(owner.am, current.revision, randomUUID());
  }, recordFailure);
}
