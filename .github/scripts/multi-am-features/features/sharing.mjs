import {createHash, randomBytes, randomUUID} from 'node:crypto';
import {once} from 'node:events';
import {createServer} from 'node:http';
import {assertOwner, check, other} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

async function competingRequests(...requests) {
  const results = await Promise.allSettled(requests);
  const failure = results.find(result => result.status === 'rejected');
  if (failure) throw failure.reason;
  return results.map(result => result.value.body);
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
  let published, armed, disarmed;
  await withRestoration(async () => {
    armed = await adapter.artifactPublicationAbort({arm: true});
    // Bypass ordinary pacing so a peer request overlaps the transaction retry.
    published = await competingRequests(user.rawRequest(ingress, 'POST', artifacts, artifactIntent),
      user.rawRequest(owner.am, 'POST', artifacts, artifactIntent));
  }, async () => {
    disarmed = await adapter.artifactPublicationAbort({arm: false});
  }, recordFailure);
  check(Number.isSafeInteger(armed?.count) && disarmed?.count === armed.count + 1,
    'artifact_publication_abort_not_consumed_once');
  const [artifact, contender] = published;
  check(artifact.agent_id === agentId && typeof artifact.id === 'string' &&
    artifact.size_bytes === data.length && artifact.sha256 === createHash('sha256').update(data).digest('hex'),
  'artifact_admission_identity');
  check(contender.id === artifact.id && contender.created_at === artifact.created_at &&
    contender.sha256 === artifact.sha256, 'artifact_concurrent_receipt_diverged');
  const artifactPath = artifacts + '/' + encodeURIComponent(artifact.id);
  const {body: replay} = await user.request(owner.am, 'POST', artifacts, artifactIntent);
  check(replay.id === artifact.id && replay.created_at === artifact.created_at, 'artifact_replay_changed_receipt');
  await user.request(owner.am, 'POST', artifacts,
    {...artifactIntent, body: {...artifactInput, name: 'changed.txt'}, allowed: [409]});
  await outsider.request(ingress, 'GET', access, {allowed: [404]});

  const token = randomBytes(32).toString('hex');
  const {body: link} = await user.request(ingress, 'POST', links, {
    body: {audience: {kind: 'public'}, mode: 'view', token}, key: randomUUID(),
  });
  check(typeof link.id === 'string' && link.agent_id === agentId, 'share_link_admission_identity');
  const redeem = '/api/v1/agent-share-links/' + encodeURIComponent(link.id) + '/redeem';
  const publicationDenied = async am => outsider.request(am, 'POST', artifacts,
    {body: artifactInput, key: randomUUID(), allowed: [404]});
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
    await publicationDenied(ingress);
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
    await publicationDenied(owner.am);
  }, recordFailure);
  const {body: listing} = await user.request(owner.am, 'GET', artifacts);
  const matches = listing.items.filter(row => row.id === artifact.id || row.name === artifactInput.name);
  check(matches.length === 1 && matches[0].id === artifact.id && matches[0].sha256 === artifact.sha256,
    'artifact_effect_missing_or_duplicated');
  await recordState('artifact_publication_retry', {known_aborts: 1, artifacts: matches.length});
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
  const digest = createHash('sha256').update(data).digest('hex');
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
    check(typeof committed?.id === 'string' && committed.agent_id === agentId && committed.sha256 === digest,
      'artifact_acknowledgement_commit_unbound');
  }, async () => {
    if (!relay.listening) return;
    await new Promise((resolve, reject) => relay.close(error => error ? reject(error) : resolve()));
  }, recordFailure);
  const readOnce = async am => {
    const {body} = await user.rawRequest(am, 'GET', path);
    check(Array.isArray(body?.items) && body.next === undefined, 'artifact_acknowledgement_listing_unbound');
    const rows = body.items.filter(row => row.name === intent.body.name || row.id === committed.id);
    check(rows.length === 1 && rows[0].id === committed.id && rows[0].sha256 === digest,
      'artifact_acknowledgement_effect_missing_or_duplicated');
  };
  await readOnce(owner.am);
  const {body: recovered} = await user.rawRequest(owner.am, 'POST', path, intent);
  check(recovered?.id === committed.id && recovered.created_at === committed.created_at && recovered.sha256 === digest,
    'artifact_acknowledgement_replay_changed_receipt');
  await readOnce(ingress);
  await recordState('artifact_committed_without_acknowledgement', {upstream_commits_observed: 1, recovered_artifacts: 1});
}
