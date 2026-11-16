import test from 'node:test';
import assert from 'node:assert/strict';
import {randomBytes} from 'node:crypto';
import {validateProfileFixture, profileWithSettings, profileTargetFixture, fillProfileSettings, assertProviderBinding,
  consumerProfileCase} from './consumer-profile.mjs';
import {channelDraft} from './channel-creation.mjs';
import {parentEgSetup} from './setup.mjs';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';

const ctx = {assert: (condition, code) => assert.ok(condition, code)};
function lease() {
  return {draft: {name: 'Synthetic channel', profileName: 'Synthetic profile', description: 'Disposable configuration',
    provider: 'local-provider', model: {access: 'managed', service: 'local-service', api_type: 'responses',
      model: 'synthetic-model', thinking_effort: 'high'}, permission: {action_mode: 'guarded', service_scope: ['local-service']},
    systemPrompt: 'Synthetic instructions', hmacKeyId: 'local-hmac', promptTemplate: '{{text}}', autoFinal: false, ackAfterMs: 1500},
  expected: {ownerSubject: 'synthetic-owner', allowedServices: ['local-service']}, existingAgentId: 'synthetic-agent',
  settings: {skills: [{source: 'ornn', id: 'synthetic-skill', version: '1.0.0'}],
    mcpServers: [{url: 'https://127.0.0.1:2345/mcp', auth: {type: 'none'}},
      {url: 'https://[::1]:2346/mcp', auth: {type: 'nyxid_service', slug: 'local-service'}}],
    environment: {CENSUS_MODE: 'synthetic', MAX_ITEMS: 5, ENABLED: true}, queueLifetimeSeconds: 45}};
}

test('existing-Agent setup enables required automatic replies without altering the profile-target choice', () => {
  const original = lease(), before = structuredClone(original);
  const profile = channelDraft(profileTargetFixture(original, 'profile'), {});
  const existing = channelDraft(profileTargetFixture(original, 'existing_agent'), {});
  assert.equal(profile.reply.auto_final, false);
  assert.equal(existing.reply.auto_final, true);
  assert.equal(existing.reply.ack_after_ms, original.draft.ackAfterMs);
  assert.deepEqual(existing.profile, profile.profile);
  assert.deepEqual(original, before);
});

test('Consumer profile fixtures require bounded supported settings and exclusively local MCP endpoints', () => {
  const valid = lease(); assert.equal(validateProfileFixture(valid), valid.settings);
  for (const change of [v => {v.existingAgentId = '';}, v => {v.settings.skills = [];},
    v => {v.settings.skills[0].version = '';}, v => {v.settings.mcpServers = [];},
    v => {v.settings.mcpServers[0].url = 'https://example.com/mcp';},
    v => {v.settings.mcpServers[0].url = 'http://127.0.0.1/mcp';},
    v => {v.settings.mcpServers[0].url = 'https://name@127.0.0.1/mcp';},
    v => {v.settings.environment = {};}, v => {v.settings.environment = {NESTED: {}};},
    v => {v.settings.queueLifetimeSeconds = 0;}, v => {v.expected.allowedServices = ['different'];}]) {
    const invalid = lease(); change(invalid);
    assert.throws(() => validateProfileFixture(invalid), error => error instanceof Blocked);
  }
  assert.equal(consumerProfileCase.evidence_scope, 'eg_only');
  assert.ok(consumerProfileCase.fixtures.includes('eg_browser'));
});

test('profile settings preserve owner/model/permissions and require full supported provider application', () => {
  const f = lease(), desired = channelDraft(f, {}), hashes = {agent: 'sha256:' + digest(randomBytes(32))};
  const original = structuredClone(desired.profile);
  desired.profile = profileWithSettings(desired.profile, f.settings);
  assert.deepEqual(original.context.skills, []);
  assert.deepEqual(desired.profile.model, original.model);
  assert.deepEqual(desired.profile.metadata.owner, original.metadata.owner);
  assert.deepEqual(desired.profile.permission.service_scope, original.permission.service_scope);
  const record = {provider: {binding_id: 'observed-binding'}, definition: {profile_digest: 'sha256:' + digest(desired.profile)}};
  const snapshot = {binding_id: 'observed-binding', bind_requests: 1, owner: f.expected.ownerSubject,
    agent_owner: f.expected.ownerSubject, authorized_services: f.expected.allowedServices,
    agent_key_hash: hashes.agent, profile: desired.profile, profile_digest: record.definition.profile_digest,
    target: {type: 'profile'}, agent_id: 'observed-agent', applied_profile: desired.profile, refused: []};
  assertProviderBinding(ctx, snapshot, record, desired, hashes);
  for (const change of [s => {s.owner = 'foreign-owner';}, s => {s.agent_owner = 'foreign-owner';},
    s => {s.authorized_services = [];}, s => {s.bind_requests = 2;},
    s => {s.profile_digest = 'different';}, s => {s.agent_key_hash = 'different';},
    s => {s.applied_profile.context.mcp_servers = [];}, s => {s.applied_profile.environment = {};},
    s => {s.applied_profile.permission.service_scope.push('unexpected-service');},
    s => {s.refused = [{field: 'context.mcp_servers', reason: 'unsupported'}];},
    s => {s.credentials = {};}, s => {delete s.applied_profile;}]) {
    const invalid = structuredClone(snapshot); change(invalid);
    assert.throws(() => assertProviderBinding(ctx, invalid, record, desired, hashes), /consumer_provider_/);
  }
  desired.provider.target = {type: 'existing_agent', agent_id: f.existingAgentId};
  snapshot.target = desired.provider.target;
  assert.throws(() => assertProviderBinding(ctx, snapshot, record, desired, hashes), /resolved_agent/);
  snapshot.agent_id = f.existingAgentId; assertProviderBinding(ctx, snapshot, record, desired, hashes);
});

test('advanced settings use actual typed form controls for skills, MCP auth and environment values', async () => {
  const calls = [], settings = lease().settings;
  const root = {getByLabel: (label, {exact}) => ({fill: async value => {
    assert.equal(exact, true); calls.push(['fill', label, value]);}}),
  getByRole: (role, {name, exact}) => {
    assert.equal(exact, true);
    if (role === 'group') {assert.equal(name, 'Environment values'); return root;}
    return {click: async () => calls.push(['click', role, name]),
      setChecked: async value => calls.push(['check', name, value])};
  }};
  await fillProfileSettings(root, settings);
  for (const [label, value] of [['Maximum wait (seconds)', '45'], ['Skill 1 ID', 'synthetic-skill'],
    ['Skill 1 version', '1.0.0'], ['Server 2 service', 'local-service'], ['MAX_ITEMS', '5']])
    assert.ok(calls.some(call => call[0] === 'fill' && call[1] === label && call[2] === value));
  assert.ok(calls.some(call => call[0] === 'check' && call[1] === 'ENABLED' && call[2] === true));
  assert.equal(calls.filter(call => call[0] === 'click' && call[2] === 'Add MCP server').length, 2);
  assert.equal(calls.filter(call => call[0] === 'click' && call[2] === 'Add field').length, 3);
});

test('Consumer profile setup needs custody, independent provider state and disclosure scanning', async () => {
  const base = {...lease(), local: true, synthetic: true, replicas: [{ready: true}, {ready: true}],
    fillProviderKey: async () => ({}), retainChannelIntent: async () => ({}), channelCreationRecords: async () => ({}),
    bindingSnapshot: async () => ({}), scanBrowserForCredentialValues: async () => ({}), close: async () => {}};
  for (const method of ['fillProviderKey', 'retainChannelIntent', 'channelCreationRecords', 'bindingSnapshot', 'scanBrowserForCredentialValues']) {
    const partial = {...base}; delete partial[method]; const cleanups = [];
    await assert.rejects(parentEgSetup({row: {}, defer: (_label, cleanup) => cleanups.push(cleanup)},
      {eg: {acquire: async () => partial}}, 'consumer_profile'), error => error instanceof Blocked);
    assert.equal(cleanups.length, 1); await cleanups[0]();
  }
});
