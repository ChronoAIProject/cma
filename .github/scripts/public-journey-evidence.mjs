import assert from 'node:assert/strict';

export const currentJourneyCases = Object.freeze({
  j5: 'J5 independent image and inline publishers bind visitor embeds, revisions and exact counts',
  j6: 'J6 image and inline Workspace tasks preserve identity through retry, pause, rotation and deletion',
  compatibility: 'COMPAT-TC025 historical public profile requires explicit trust before Sandbox allocation',
});
const keys = (value, expected) => assert.deepEqual(Object.keys(value).sort(), [...expected].sort());
const id = (value) => assert.match(value, /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,255}$/);
const digest = (value) => assert.match(value, /^[a-f0-9]{64}$/);
const distinct = (values) => { values.forEach(id); assert.equal(new Set(values).size, values.length); };
const sameSet = (actual, expected) => assert.deepEqual([...actual].sort(), [...expected].sort());

function owned(journal, run, kind, resource, subject) {
  id(resource); id(subject);
  assert.ok(journal.some((row) => row.action === 'created' && row.run === run &&
    row.kind === kind && row.id === resource && row.subject === subject), 'Missing owned journey resource');
}
function command(value, path) {
  keys(value, ['method', 'path', 'bodyHash', 'keyHash']);
  assert.equal(value.method, 'POST');
  assert.equal(value.path, path);
  digest(value.bodyHash); digest(value.keyHash);
}
function stats(value, placement) {
  keys(value, ['totals', 'placements']);
  keys(value.totals, ['launches_total', 'agents_ready_total']);
  for (const row of value.placements) keys(row, ['placement_id', 'launches_total', 'agents_ready_total']);
  assert.ok(Array.isArray(value.placements));
  const rows = value.placements.filter((row) => row.placement_id === placement);
  assert.equal(rows.length, 1);
  for (const row of [value.totals, rows[0]])
    for (const key of ['launches_total', 'agents_ready_total'])
      assert.ok(Number.isSafeInteger(row[key]) && row[key] >= 0);
  return [value.totals, rows[0]];
}
function publication(entry, proof, journal, visitor, managed) {
  keys(entry, ['publisher', 'observedPublisher', 'targetKind', 'submittedProfileIds', 'submittedTargetHash', 'publishedProfiles', 'revisionProfiles', 'authoringCommands', 'requestedTargetHash', 'publishedTargetHash', 'revisionTargetHash', 'expectedImage', 'observedImage', 'triggerId', 'publicId', 'placementId', 'revision', 'revisionDigest', 'launch', 'commands', 'stats']);
  keys(entry.stats, ['before', 'after']);
  id(entry.publisher);
  assert.equal(entry.observedPublisher, entry.publisher);
  assert.equal(entry.targetKind, 'inline');
  assert.deepEqual(entry.submittedProfileIds, []);
  keys(entry.publishedProfiles, ['sandbox', 'workspace', 'agent']);
  for (const reference of Object.values(entry.publishedProfiles)) {
    keys(reference, ['id', 'revision']); id(reference.id);
    assert.ok(Number.isSafeInteger(reference.revision) && reference.revision > 0);
  }
  assert.deepEqual(entry.revisionProfiles, entry.publishedProfiles);
  const required = ['/api/v1/triggers', `/api/v1/triggers/${entry.triggerId}/placements`, `/api/v1/triggers/${entry.triggerId}/publish`];
  for (const path of required) {
    const matches = entry.authoringCommands.filter((row) => row.path === path);
    assert.equal(matches.length, 1); command(matches[0], path);
  }
  for (const row of entry.authoringCommands) {
    if (required.includes(row.path)) continue;
    keys(row, ['method', 'path', 'bodyHash', 'keyHash']);
    assert.equal(row.method, 'PUT'); assert.equal(row.path, `/api/v1/triggers/${entry.triggerId}`);
    digest(row.bodyHash); digest(row.keyHash);
  }
  for (const key of ['requestedTargetHash', 'submittedTargetHash', 'publishedTargetHash', 'revisionTargetHash'])
    digest(entry[key]);
  assert.equal(entry.submittedTargetHash, entry.requestedTargetHash);
  assert.equal(entry.publishedTargetHash, entry.requestedTargetHash);
  assert.equal(entry.revisionTargetHash, entry.requestedTargetHash);
  if (managed) {
    assert.deepEqual(entry.expectedImage, { kind: 'managed_default' });
  } else {
    keys(entry.expectedImage, ['kind', 'id', 'version', 'referenceHash']);
    assert.equal(entry.expectedImage.kind, 'catalogue');
    id(entry.expectedImage.id); id(entry.expectedImage.version); digest(entry.expectedImage.referenceHash);
  }
  assert.deepEqual(entry.observedImage, entry.expectedImage);
  id(entry.triggerId); id(entry.publicId); id(entry.placementId);
  assert.equal(entry.revision, 1);
  digest(entry.revisionDigest);
  owned(journal, proof.run, 'triggers', entry.triggerId, entry.publisher);
  const launch = entry.launch;
  keys(launch, ['id', 'triggerId', 'publicId', 'placementId', 'revisionDigest', 'sandboxId', 'agentId', 'owner']);
  assert.equal(launch.triggerId, entry.triggerId);
  assert.equal(launch.publicId, entry.publicId);
  assert.equal(launch.placementId, entry.placementId);
  assert.equal(launch.revisionDigest, entry.revisionDigest);
  assert.equal(launch.owner, visitor);
  owned(journal, proof.run, 'launches', launch.id, visitor);
  owned(journal, proof.run, 'sandboxes', launch.sandboxId, visitor);
  owned(journal, proof.run, 'agents', launch.agentId, visitor);
  assert.equal(entry.commands.length, 1);
  const launchPath = entry.commands[0].path;
  assert.ok(['v1', 'v2'].some((version) =>
    launchPath === `/api/${version}/triggers/public/${entry.publicId}/launches`));
  command(entry.commands[0], launchPath);
  const before = stats(entry.stats.before, entry.placementId);
  const after = stats(entry.stats.after, entry.placementId);
  for (let index = 0; index < before.length; index++)
    for (const key of ['launches_total', 'agents_ready_total'])
      assert.equal(after[index][key], before[index][key] + 1, 'One explicit intent must count exactly once');
}
function runtime(value, agents) {
  keys(value, ['isolatedHomes', 'installBeforeCredentials', 'softwarePresent', 'dirtyWorktreePresent', 'transcriptsPreserved', 'runtimeVersion', 'imageReferenceHash', 'homes']);
  assert.equal(typeof value.installBeforeCredentials, 'boolean');
  for (const key of ['isolatedHomes', 'softwarePresent', 'dirtyWorktreePresent', 'transcriptsPreserved'])
    assert.equal(value[key], true);
  digest(value.runtimeVersion);
  digest(value.imageReferenceHash);
  assert.equal(value.homes.length, agents.length);
  sameSet(value.homes.map((row) => row.agentId), agents);
  for (const row of value.homes) { keys(row, ['agentId', 'homeDigest', 'fileHash']); digest(row.homeDigest); digest(row.fileHash); }
  assert.equal(new Set(value.homes.map((row) => row.homeDigest)).size, agents.length);
}

export function validatePublicJourneyProof(title, proof, journal) {
  const common = ['schema', 'execution', 'scope', 'run'];
  keys(proof, [...common, ...(title === currentJourneyCases.j5 ? ['visitor', 'publications'] : ['owner', 'bootstrap', 'sandboxId', 'workspaceId', 'agents', 'workspaceAgentIds', 'expectedManagedImageHash', 'before', 'after', 'retry', 'deletedAgentId', 'survivingAgentId'])]);
  assert.equal(proof.schema, 'cma-public-journey-v1');
  assert.equal(proof.execution, 'configured_remote_origin');
  assert.ok(['current_image_inline', 'managed_default_inline'].includes(proof.scope));
  id(proof.run);
  if (title === currentJourneyCases.j5) {
    id(proof.visitor);
    assert.equal(proof.publications.length, 2);
    distinct(proof.publications.map((entry) => entry.publisher));
    assert.ok(!proof.publications.some((entry) => entry.publisher === proof.visitor));
    assert.equal(new Set(proof.publications.map((entry) => entry.requestedTargetHash)).size, 2);
    for (const entry of proof.publications) publication(entry, proof, journal, proof.visitor, proof.scope === 'managed_default_inline');
    for (const read of [
      (entry) => entry.triggerId, (entry) => entry.publicId,
      (entry) => entry.launch.id, (entry) => entry.launch.agentId, (entry) => entry.launch.sandboxId,
    ]) distinct(proof.publications.map(read));
    return;
  }
  assert.equal(title, currentJourneyCases.j6);
  id(proof.owner);
  publication(proof.bootstrap, proof, journal, proof.owner, true);
  assert.equal(proof.bootstrap.publisher, proof.owner);
  assert.equal(proof.sandboxId, proof.bootstrap.launch.sandboxId);
  owned(journal, proof.run, 'workspaces', proof.workspaceId, proof.owner);
  assert.equal(proof.agents.length, 2);
  const agents = proof.agents.map((row) => row.id);
  distinct(agents);
  distinct(proof.agents.map((row) => row.worktreeId));
  for (const row of proof.agents) {
    keys(row, ['id', 'workspaceId', 'sandboxId', 'worktreeId', 'command']);
    assert.equal(row.workspaceId, proof.workspaceId);
    assert.equal(row.sandboxId, proof.sandboxId);
    owned(journal, proof.run, 'agents', row.id, proof.owner);
    command(row.command, '/api/v1/agents');
  }
  keys(proof.retry, ['operationId', 'agentId', 'commands']);
  assert.equal(proof.retry.commands.length, 2);
  proof.retry.commands.forEach((row) => command(row, '/api/v1/agents'));
  assert.deepEqual(proof.retry.commands[0], proof.retry.commands[1]);
  owned(journal, proof.run, 'agent-lifecycle', proof.retry.operationId, proof.owner);
  owned(journal, proof.run, 'agents', proof.retry.agentId, proof.owner);
  distinct([...agents, proof.retry.agentId, proof.bootstrap.launch.agentId]);
  sameSet(proof.workspaceAgentIds, [...agents, proof.retry.agentId]);
  runtime(proof.before, agents);
  assert.equal(proof.before.installBeforeCredentials, true);
  digest(proof.expectedManagedImageHash);
  assert.equal(proof.before.imageReferenceHash, proof.expectedManagedImageHash);
  sameSet(proof.after.map((row) => row.fault), ['restart_pausing', 'restart_reviving', 'rotate_image']);
  for (const after of proof.after) {
    keys(after, ['fault', 'snapshot']);
    runtime(after.snapshot, agents);
    for (const row of proof.before.homes)
      assert.deepEqual(after.snapshot.homes.find((home) => home.agentId === row.agentId), row);
    if (after.fault === 'rotate_image') {
      assert.notEqual(after.snapshot.runtimeVersion, proof.before.runtimeVersion);
      assert.notEqual(after.snapshot.imageReferenceHash, proof.before.imageReferenceHash);
    }
  }
  assert.equal(proof.deletedAgentId, agents[0]);
  assert.equal(proof.survivingAgentId, agents[1]);
}
