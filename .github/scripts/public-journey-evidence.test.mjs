import assert from 'node:assert/strict';
import test from 'node:test';
import { validateCampaign, requiredCases } from './staging-authority.mjs';
import { currentJourneyCases } from './public-journey-evidence.mjs';
import { campaignFixture } from './public-journey-evidence.fixture.mjs';

for (const version of ['v1', 'v2']) test(`audits exact ${version} Launch routes for both journeys`, () => {
  const { results, journal } = campaignFixture(requiredCases);
  for (const row of results.cases) {
    const entries = row.proof?.publications ?? (row.proof?.bootstrap ? [row.proof.bootstrap] : []);
    for (const entry of entries)
      entry.commands[0].path = `/api/${version}/triggers/public/${entry.publicId}/launches`;
  }
  validateCampaign(results, journal);
});

for (const [name, change] of [
  ['unsupported version', (command, id) => { command.path = `/api/v3/triggers/public/${id}/launches`; }],
  ['foreign public id', (command) => { command.path = '/api/v2/triggers/public/foreign/launches'; }],
  ['extra suffix', (command) => { command.path += '/extra'; }],
  ['query', (command) => { command.path += '?ignored=true'; }],
  ['read method', (command) => { command.method = 'GET'; }],
  ['missing key hash', (command) => { delete command.keyHash; }],
]) test(`bound Launch route still rejects ${name}`, () => {
  const { results, journal } = campaignFixture(requiredCases);
  const entry = results.cases.find((row) => row.case === currentJourneyCases.j5).proof.publications[0];
  entry.commands[0].path = `/api/v2/triggers/public/${entry.publicId}/launches`;
  change(entry.commands[0], entry.publicId);
  assert.throws(() => validateCampaign(results, journal));
});

test('current campaign has 33 distinct cases including independent historical compatibility', () => {
  assert.equal(requiredCases.length, 33);
  assert.equal(new Set(requiredCases).size, 33);
  const { results, journal } = campaignFixture(requiredCases);
  validateCampaign(results, journal);
  for (const name of Object.values(currentJourneyCases)) {
    assert.throws(() => validateCampaign({ ...results, cases: results.cases.filter((row) => row.case !== name) }, journal));
  }
  const old = structuredClone(results);
  old.cases.find((row) => row.case === currentJourneyCases.j6).case = 'J6 G-S1 G-P1 trusted Sandbox, two Agents, stop/delete, pause/revive and rotation preserve work';
  assert.throws(() => validateCampaign(old, journal));
});

for (const [name, change] of [
  ['missing proof', (p) => { delete p.schema; }],
  ['legacy path', (p) => { p.scope = 'legacy_profile'; }],
  ['local execution', (p) => { p.execution = 'synthetic_local'; }],
  ['wrong run', (p) => { p.run = 'another-run'; }],
  ['shared publishers', (p) => { p.publications[1].publisher = p.publications[0].publisher; }],
  ['owner in visitor slot', (p) => { p.visitor = p.publications[0].publisher; }],
  ['wrong session owner', (p) => { p.publications[0].observedPublisher = 'foreign'; }],
  ['changed image', (p) => { p.publications[0].observedImage.version = 'changed'; }],
  ['legacy target', (p) => { p.publications[0].submittedProfileIds.push('legacy-profile'); }],
  ['wrong submitted hash', (p) => { p.publications[0].submittedTargetHash = '0'.repeat(64); }],
  ['changed generated profile', (p) => { p.publications[0].revisionProfiles.agent.id = 'other'; }],
  ['empty generated profile', (p) => { p.publications[0].publishedProfiles.agent.id = ''; }],
  ['invalid generated revision', (p) => { p.publications[0].publishedProfiles.agent.revision = 0; }],
  ['profile authoring command', (p) => { p.publications[0].authoringCommands.push({ ...p.publications[0].authoringCommands[0], path: '/api/v1/agent-profiles' }); }],
  ['Marketplace command', (p) => { p.publications[0].authoringCommands.push({ ...p.publications[0].authoringCommands[0], path: '/api/v1/marketplace/fork' }); }],
  ['missing authoring command', (p) => { p.publications[0].authoringCommands.pop(); }],
  ['duplicate authoring command', (p) => { p.publications[0].authoringCommands.push(p.publications[0].authoringCommands[0]); }],
  ['wrong target hash', (p) => { p.publications[0].revisionTargetHash = '0'.repeat(64); }],
  ['wrong revision', (p) => { p.publications[0].launch.revisionDigest = '0'.repeat(64); }],
  ['wrong placement', (p) => { p.publications[0].launch.placementId = 'foreign'; }],
  ['wrong visitor owner', (p) => { p.publications[0].launch.owner = 'owner'; }],
  ['duplicate commands', (p) => { p.publications[0].commands.push(p.publications[0].commands[0]); }],
  ['legacy command', (p) => { p.publications[0].commands[0].path = '/api/v1/sandboxes'; }],
  ['duplicate total', (p) => { p.publications[0].stats.after.totals.launches_total++; }],
  ['missing placement count', (p) => { p.publications[0].stats.after.placements[0].agents_ready_total = 0; }],
  ['unsafe extra detail', (p) => { p.publications[0].rawResponse = 'unreviewed-content'; }],
]) test(`J5 audit rejects ${name}`, () => {
  const { results, journal } = campaignFixture(requiredCases);
  change(results.cases.find((row) => row.case === currentJourneyCases.j5).proof);
  assert.throws(() => validateCampaign(results, journal));
});

for (const [name, change] of [
  ['parent mismatch', (p) => { p.agents[0].sandboxId = 'foreign'; }],
  ['shared Worktree', (p) => { p.agents[0].worktreeId = p.agents[1].worktreeId; }],
  ['retry changes key', (p) => { p.retry.commands[1].keyHash = '0'.repeat(64); }],
  ['retry changes body', (p) => { p.retry.commands[1].bodyHash = '0'.repeat(64); }],
  ['extra task', (p) => { p.workspaceAgentIds.push('unexpected-agent'); }],
  ['wrong managed image', (p) => { p.expectedManagedImageHash = '0'.repeat(64); }],
  ['missing lifecycle phase', (p) => { p.after.pop(); }],
  ['lost file', (p) => { p.after[0].snapshot.homes[0].fileHash = '0'.repeat(64); }],
  ['shared home', (p) => { p.before.homes[0].homeDigest = p.before.homes[1].homeDigest; }],
  ['rotation unchanged', (p) => { p.after[2].snapshot.imageReferenceHash = p.before.imageReferenceHash; }],
  ['deleted sibling', (p) => { p.survivingAgentId = p.deletedAgentId; }],
]) test(`J6 audit rejects ${name}`, () => {
  const { results, journal } = campaignFixture(requiredCases);
  change(results.cases.find((row) => row.case === currentJourneyCases.j6).proof);
  assert.throws(() => validateCampaign(results, journal));
});

test('passed proofs cannot substitute for owned resource and terminal cleanup records', () => {
  const { results, journal } = campaignFixture(requiredCases);
  for (const removed of journal.filter((row) => row.action === 'created')) {
    const without = journal.filter((row) => row.kind !== removed.kind || row.id !== removed.id);
    assert.throws(() => validateCampaign(results, without));
  }
});
