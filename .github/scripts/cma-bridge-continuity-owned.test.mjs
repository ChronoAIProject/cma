import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm, stat, chmod, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { validateOwnedConfig, loadOwnedConfig, validateOwnedFixture, retainedCommand } from './cma-bridge-continuity-owned-config.mjs';
import { nativeIdentity, compareRetained } from './cma-bridge-continuity-native.mjs';
import { capture } from './cma-bridge-continuity-observer.mjs';
import { config as observerConfig, fixture } from './cma-bridge-continuity-watch-fixture.mjs';

globalThis.fetch = () => { throw new Error('offline_validation_forbids_network'); };
const config = () => ({
  ...observerConfig, schema: 'cma-owned-continuity-config-v1', run: 'owned-fixture',
  expectedSubject: 'fixture-owner', markers: { trigger: 'OWNED-TRIGGER', manual: 'OWNED-MANUAL' },
  fixtureJournal: '/private/fixture/journal.json', fixtureResult: '/private/fixture/result.json',
  outputDirectory: '/private/fixture/continuity', limits: { turnSeconds: 300, pollSeconds: 2 },
});
const resources = { agentIds: ['agt_trigger', 'agt_manual'], sandboxId: 'sbx_owned', workspaceId: 'wks_owned' };
const journal = () => ({ schema: 'cma-public-canary-journal-v1', run: 'owned-fixture',
  origin: observerConfig.origin, owner: 'fixture-owner', commands: {}, resources: structuredClone(resources), result: { status: 'passed' } });

test('portable configuration preserves HTTPS, exact cluster scope, private paths and bounded polling', () => {
  assert.equal(validateOwnedConfig(config()).origin, 'https://fixture.example');
  for (const mutate of [
    value => { value.origin = 'http://fixture.example'; },
    value => { value.origin = 'https://credential@fixture.example'; },
    value => { value.origin += '/api'; },
    value => { value.namespace = '--all-namespaces'; },
    value => { value.tokenFile = 'relative-token'; },
    value => { value.fixtureJournal = '../journal.json'; },
    value => { value.outputDirectory = '.'; },
    value => { value.run = '../another-run'; },
    value => { value.markers.manual = value.markers.trigger; },
    value => { value.markers.manual = 'marker; exit'; },
    value => { value.limits.pollSeconds = 0; },
    value => { value.limits.turnSeconds = 100000; },
  ]) { const value = config(); mutate(value); assert.throws(() => validateOwnedConfig(value)); }
});

test('owned fixture refuses failures, changed owners, scopes, duplicate Agents and cleanup intent', () => {
  const result = { status: 'passed', resources };
  validateOwnedFixture(config(), journal(), result);
  for (const mutate of [
    value => { value.owner = 'another-owner'; },
    value => { value.run = 'another-run'; },
    value => { value.origin = 'https://another.invalid'; },
    value => { value.result.status = 'failed'; },
    value => { value.resources.agentIds[1] = value.resources.agentIds[0]; },
    value => { value.resources.sandboxId = 'sbx_other'; },
    value => { value.commands['pause-sandbox'] = { state: 'pending' }; },
    value => { value.commands['retire-trigger'] = { state: 'accepted' }; },
  ]) { const value = journal(); mutate(value); assert.throws(() => validateOwnedFixture(config(), value, result)); }
  assert.throws(() => validateOwnedFixture(config(), journal(), { ...result, status: 'failed' }));
});

test('output is a private external directory and refuses public or symbolic destinations', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'cma-owned-continuity-'));
  try {
    const path = join(directory, 'config.json');
    const value = { ...config(), outputDirectory: join(directory, 'evidence') };
    await writeFile(path, JSON.stringify(value), { mode: 0o600 });
    await loadOwnedConfig(path);
    assert.equal((await stat(value.outputDirectory)).mode & 0o077, 0);
    await chmod(value.outputDirectory, 0o755);
    await assert.rejects(loadOwnedConfig(path), /private_output_directory_required/);
    await chmod(value.outputDirectory, 0o700);
    await symlink(value.outputDirectory, join(directory, 'linked'));
    await writeFile(path, JSON.stringify({ ...value, outputDirectory: join(directory, 'linked') }));
    await assert.rejects(loadOwnedConfig(path), /private_output_directory_required/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('only an identical accepted request can be recovered without sending another command', () => {
  const accepted = { state: 'accepted', path: '/api/v2/responses', body: '{}', key: 'retained', value: { commandId: 'cmd_owned' } };
  assert.deepEqual(retainedCommand(accepted, accepted.path, accepted.body, accepted.key), accepted.value);
  for (const state of ['pending', 'unsettled']) {
    assert.throws(() => retainedCommand({ ...accepted, state }, accepted.path, accepted.body, accepted.key), /explicit_recovery/);
  }
  for (const field of ['path', 'body', 'key']) {
    assert.throws(() => retainedCommand({ ...accepted, [field]: 'changed' }, accepted.path, accepted.body, accepted.key), /retained_command_changed/);
  }
});

const opened = (conversation = 'native-fixture', generation = 1) => ({ kind: { type: 'conversation_opened', conversation_id: conversation, runtime_generation: generation } });
test('reopening the same native conversation preserves identity but new conversations or retired generations fail', () => {
  const identity = nativeIdentity([opened(), opened()]);
  assert.equal(identity.openings, 2);
  assert.ok(!JSON.stringify(identity).includes('native-fixture'));
  for (const events of [[], [opened(), opened('other')], [opened(), opened('native-fixture', 2)], [opened(), { kind: { type: 'runtime_generation_retired' } }]]) {
    assert.throws(() => nativeIdentity(events));
  }
});

test('retained comparison protects completed history and allows only appended active-turn messages', async () => {
  const reads = fixture();
  const snapshot = await capture(observerConfig, { get: (path, authenticated) => reads.get(path, authenticated, {}), pods: () => reads.pods({}) });
  const before = { snapshot, agents: [{ agent: { worktree: { id: 'wkt_fixture' } }, native: nativeIdentity([opened()]),
    transcript: { turns: [{ turn_sequence: 1, started_at: '2026-09-16T00:00:00Z', owner_message: 'owned prompt',
      outcome: { type: 'active' }, messages: [{ role: 'assistant', content: 'retained prefix' }] }] } }] };
  const after = structuredClone(before);
  after.agents[0].transcript.turns[0].messages.push({ role: 'assistant', content: 'new output' });
  after.agents[0].transcript.turns[0].outcome = { type: 'completed' };
  assert.equal(compareRetained(before, after, true).status, 'passed');
  assert.throws(() => compareRetained(before, after), /transcript_prefix/);
  for (const mutate of [
    value => { value.agents[0].native = nativeIdentity([opened('other')]); },
    value => { value.agents[0].agent.worktree.id = 'wkt_replaced'; },
    value => { value.agents[0].transcript.turns[0].owner_message = 'replacement prompt'; },
    value => { value.agents[0].transcript.turns[0].messages[0].content = 'replacement output'; },
    value => { value.snapshot.runtimes[0].containers[0].restarts++; },
  ]) { const value = structuredClone(after); mutate(value); assert.throws(() => compareRetained(before, value, true)); }
});
