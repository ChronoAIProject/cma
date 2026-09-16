import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { compareSnapshots, digest, eventPrefix } from './cma-bridge-continuity-observer.mjs';
import { nativeIdentity, compareRetained } from './cma-bridge-continuity-native.mjs';
import { loadOwnedFixture, validateOwnedFixture } from './cma-bridge-continuity-owned-config.mjs';
import { resolveRecoveryFixture } from './cma-bridge-continuity-recovery.mjs';
import { scope } from './cma-bridge-continuity-browser-contract.mjs';

globalThis.fetch = () => { throw new Error('offline_tests_forbid_network'); };
const bytes = value => Buffer.from(JSON.stringify(value, null, 2) + '\n');
const hash = value => createHash('sha256').update(Buffer.isBuffer(value) || typeof value === 'string' ? value : JSON.stringify(value)).digest('hex');
const at = '2026-09-16T12:00:00Z';
const event = (seq, kind) => ({ seq, appended_at: at, kind });
const opening = (seq, generation = 1, conversation = 'retained-native') => event(seq,
  { type: 'conversation_opened', conversation_id: conversation, runtime_generation: generation });

function fixture() {
  const resources = { agentIds: ['agt_trigger', 'agt_manual'], sandboxId: 'sbx_owned', workspaceId: 'wks_owned' };
  const ids = [...resources.agentIds, 'agt_bootstrap'];
  const originalEvents = [opening(1), event(2, { type: 'message', text: 'retained turn' })];
  const events = [...originalEvents, event(3, { type: 'runtime_generation_retired', runtime_generation: 1 }), opening(4, 2)];
  const native = { ...nativeIdentity(originalEvents), runtimeGeneration: 2, openings: 2,
    recordedOpenings: [1, 4].map(seq => ({ sequence: seq, at, conversationSha256: hash('retained-native'), runtimeGeneration: seq === 1 ? 1 : 2 })),
    recordedRetirements: [{ sequence: 3, at, ...events[2].kind }] };
  const firstTurn = { outcome: { type: 'completed' }, owner_message: 'retain marker', messages: [{ role: 'assistant', content: 'MARKER' }] };
  const before = { schema: 'cma-bridge-continuity-observation-v1', status: 'observed', observedAt: at,
    scope: { agentIds: resources.agentIds }, agents: resources.agentIds.map((id, i) => ({ id, sandboxId: resources.sandboxId,
      workspaceId: resources.workspaceId, worktreeId: `wkt_${i}`, profileHash: 'a'.repeat(64), createdAt: at, status: 'ready', events: eventPrefix(originalEvents) })),
    runtimes: [{ name: 'prior-pod', uid: 'prior-runtime', sandboxId: resources.sandboxId, generation: '1', contract: '3',
      containers: [{ name: 'runtime', image: 'fixture-image', id: 'fixture-container', restarts: 0 }] }] };
  const original = { schema: 'cma-public-canary-journal-v1', run: 'owned-recovery', origin: 'https://fixture.example', owner: 'fixture-owner',
    commit: '1'.repeat(40), resources, bootstrapAgentId: ids[2], commands: { 'continue-0': {
      path: `/api/v1/agents/${ids[0]}/input`, body: JSON.stringify({ text: 'repeat marker' }), key: 'owned-recovery-continue-0',
      state: 'unsettled', attempts: 1, safeCode: 'public_http_403_forbidden' } },
    baseline: { before, nativeBefore: [nativeIdentity(originalEvents), nativeIdentity(originalEvents)], firstTurns: [firstTurn, firstTurn] } };
  const runtime = structuredClone(before);
  runtime.runtimes[0].uid = 'resumed-runtime'; runtime.runtimes[0].generation = '2';
  runtime.agents.forEach(agent => { agent.events = eventPrefix(events); });
  const after = { runtime, native: [native, native], public: ids.map((id, i) => ({ agent: { id, status: 'ready',
    sandbox: { id: resources.sandboxId }, workspace: { id: resources.workspaceId }, worktree: { id: `wkt_${i}` } },
    transcript: { agent_id: id, turns: i < 2 ? [firstTurn, { ...firstTurn, owner_message: 'repeat marker' }] : [] },
    provenance: { agent_id: id, creation_origin: ['trigger', 'manual', 'legacy_unknown'][i], details: { workspace_profile_index: 0 } } })) };
  const sandboxEvent = (seq, kind) => ({ ...event(seq, kind), sandbox_id: resources.sandboxId });
  const observation = { status: 'recorded_pause', applicationCommit: '2'.repeat(40),
    pause: { latestSnapshotEpoch: 1, completedAt: at }, sandboxEvents: [sandboxEvent(1, { type: 'paused' })] };
  const fresh = { ...observation, nonadmission: { originalCommandAbsent: true } };
  const resumption = { status: 'resumed_after_recorded_pause', checkpointEpoch: 1, pausedAt: at, revivedAt: at,
    priorRuntime: before.runtimes[0], resumedRuntime: runtime.runtimes[0], priorNative: original.baseline.nativeBefore, resumedNative: after.native,
    originalRuntimeComparisonPreserved: true, applicationRolloutSameRuntimeContinuity: 'not_established_by_this_paused_canary' };
  const lifecycle = { resumption, sandbox: { id: resources.sandboxId, owner: original.owner, status: 'active', lifecycle: { latest_snapshot: { epoch: 1 } } },
    events: [...observation.sandboxEvents, sandboxEvent(2, { type: 'status_changed', from: 'reviving', to: 'active' }), sandboxEvent(3, { type: 'revived' })] };
  const label = 'continue-0-after-nonadmission-1';
  const followup = { path: original.commands['continue-0'].path, body: original.commands['continue-0'].body, key: `${original.run}-${label}`, status: 202 };
  const originalBytes = bytes(original);
  const root = { status: 'confirmed_nonadmission', applicationCommit: fresh.applicationCommit, originalJournalSha256: hash(originalBytes),
    originalUnsettledCommandSha256: hash(original.commands['continue-0']), observationSha256: hash(observation), freshKeyAuthorized: followup.key,
    originalExecutionCannotAppend: true, originalKeyWillNeverBeResubmitted: true, runtimeMode: 'recorded_pause_resumption',
    allowImplicitRevivalFromMessage: true, pauseObservationSha256: hash(observation.pause) };
  const files = Object.fromEntries(Object.entries({ 'observation.json': observation, 'root-reconciliation.json': root,
    'api-receipt.json': { commit: fresh.applicationCommit }, 'web-receipt.json': { commit: fresh.applicationCommit }, 'after-rollout.json': {},
    'fresh-preflight.json': fresh, 'after.json': after, 'resumption-lifecycle.json': lifecycle }).map(([name, value]) => [name, bytes(value)]));
  const directive = { schema: 'cma-confirmed-nonadmission-followup-directive-v1', recoveryId: 'retained-recovery', originalJournalSha256: hash(originalBytes),
    startedApplicationCommit: original.commit, resumedApplicationCommit: fresh.applicationCommit, runtimeMode: root.runtimeMode,
    originalKeyWillNeverBeResubmitted: true, newResourcesPermitted: 0, plannedFinalTurns: [2, 2, 0], followup,
    helperSha256: { '/private/recovery.mjs': 'a'.repeat(64) }, evidenceSha256: Object.fromEntries(
      ['api-receipt.json', 'web-receipt.json', 'after-rollout.json', 'root-reconciliation.json', 'observation.json'].map(name => [name, hash(files[name])])) };
  files['directive.json'] = bytes(directive);
  const result = { status: 'passed', resources, commit: fresh.applicationCommit, startedApplicationCommit: original.commit,
    resumedApplicationCommit: fresh.applicationCommit, turnApplicationCommits: Array(2).fill([original.commit, fresh.applicationCommit]),
    recovery: { id: directive.recoveryId, originalJournalSha256: hash(originalBytes), directiveSha256: hash(files['directive.json']),
      originalForbiddenPreserved: true, exactOriginalKeyRetried: false, originalKeyDisposition: 'not_resubmitted', replacementFollowupKey: followup.key,
      helperSha256: directive.helperSha256, deploymentEvidenceSha256: directive.evidenceSha256, freshNonadmissionSha256: hash(fresh),
      finalEvidenceSha256: { 'after.json': hash(files['after.json']), 'resumption-lifecycle.json': hash(files['resumption-lifecycle.json']) } },
    originalFailedRunPreserved: true, allOwnedAgentIds: ids, totalOwnedAgents: 3, completedTurns: [2, 2], origins: ['trigger', 'manual'],
    sameSandbox: true, separateWorktrees: true, retainedTranscriptPrefix: true,
    bootstrapAgent: { id: ids[2], creationOrigin: 'legacy_unknown', workspaceProfileIndex: 0, completedTurns: 0, untouched: true },
    nativeIdentity: after.native, runtimeMode: root.runtimeMode, resumption, continuity: compareSnapshots(before, runtime) };
  const journal = { schema: 'cma-nonadmission-followup-journal-v1', recoveryId: directive.recoveryId, originalJournalSha256: hash(originalBytes),
    directiveHash: result.recovery.directiveSha256, startedApplicationCommit: original.commit, resumedApplicationCommit: fresh.applicationCommit,
    resources, bootstrapAgentId: ids[2], freshNonadmissionSha256: hash(fresh), original403: { command: original.commands['continue-0'],
      disposition: 'not_resubmitted', replacementFollowupLabel: label },
    commands: { [label]: { ...followup, state: 'accepted', attempts: 1 }, 'continue-1': { path: '/api/v2/responses',
      body: JSON.stringify({ conversation: ids[1], input: 'repeat marker', stream: true }), key: `${original.run}-continue-1`, state: 'accepted', status: 200, attempts: 1 } }, result };
  const sealResults = () => { files['journal.json'] = bytes(journal); files['result.json'] = bytes(result); };
  sealResults();
  return { original, result, journal, files, evidence: { originalBytes, files }, sealResults, events, after,
    config: { run: original.run, expectedSubject: original.owner, origin: original.origin, markers: { manual: 'MANUAL' } } };
}

test('separate recovery loads for continuation and browser while preserving failed original bytes and runtime comparison', async () => {
  const f = fixture(); const dir = await mkdtemp(join(tmpdir(), 'cma-recovery-'));
  try {
    const recoveryDir = join(dir, 'recovery'); await mkdir(recoveryDir);
    await writeFile(join(dir, 'journal.json'), f.evidence.originalBytes);
    await writeFile(join(dir, 'result.json'), bytes(f.result));
    await Promise.all(Object.entries(f.files).map(([name, value]) => writeFile(join(recoveryDir, name), value)));
    const selected = await loadOwnedFixture({ ...f.config, fixtureJournal: join(dir, 'journal.json'), fixtureResult: join(dir, 'result.json'), fixtureRecoveryDirectory: recoveryDir });
    assert.equal(selected.journal.result, undefined);
    assert.equal(selected.journal.commands['continue-0'].state, 'unsettled');
    assert.equal(selected.result.continuity.status, 'failed');
    assert.equal(scope(f.config, selected.journal, selected.result, 'next-rollout', selected.recovery).agentId, 'agt_manual');
    assert.equal(nativeIdentity(f.events, selected.nativeBoundaries[0]).runtimeGeneration, 2);
    assert.deepEqual(await readFile(join(dir, 'journal.json')), f.evidence.originalBytes);
    await assert.rejects(loadOwnedFixture({ ...f.config, fixtureJournal: join(dir, 'journal.json'), fixtureResult: join(dir, 'result.json') }), /linked_recovery/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('missing recovery, modified original, linked evidence or substituted resource scope cannot promote a failed fixture', () => {
  for (const mutate of [
    f => { delete f.evidence.files['directive.json']; },
    f => { f.evidence.originalBytes = Buffer.concat([f.evidence.originalBytes, Buffer.from(' ')]); },
    f => { f.original.commands['continue-0'].state = 'accepted'; },
    f => { f.files['after.json'] = bytes({ ...f.after, native: [] }); },
    f => { f.files['observation.json'] = bytes({ status: 'observed' }); },
    f => { f.result.resources = { ...f.result.resources, sandboxId: 'sbx_replacement' }; f.sealResults(); },
    f => { f.result.recovery.exactOriginalKeyRetried = true; f.sealResults(); },
    f => { f.result.continuity.status = 'passed'; f.sealResults(); },
    f => { f.result.recovery.finalEvidenceSha256 = {}; f.sealResults(); },
  ]) {
    const f = fixture(); mutate(f);
    assert.throws(() => validateOwnedFixture(f.config, f.original, f.result, f.evidence));
  }
  const f = fixture();
  assert.throws(() => validateOwnedFixture(f.config, f.original, f.result), /linked_recovery/);
  assert.throws(() => validateOwnedFixture(f.config, f.original, { status: 'passed', resources: f.original.resources }), /successful_original/);
});

test('linked final evidence rejects changed native conversation, bootstrap turn, original transcript or lineage', () => {
  for (const mutate of [
    f => { f.after.native[0].conversationSha256 = 'b'.repeat(64); },
    f => { f.after.native[0].recordedOpenings.at(-1).conversationSha256 = 'b'.repeat(64); },
    f => { f.after.public[2].transcript.turns.push(f.after.public[0].transcript.turns[0]); },
    f => { f.after.public[0].transcript.turns[0] = { ...f.after.public[0].transcript.turns[0], owner_message: 'copied history' }; },
    f => { f.after.runtime.agents[0].worktreeId = 'wkt_other'; },
  ]) {
    const f = fixture(); mutate(f);
    f.files['after.json'] = bytes(f.after);
    f.result.recovery.finalEvidenceSha256['after.json'] = hash(f.files['after.json']);
    const lifecycle = JSON.parse(f.files['resumption-lifecycle.json']); lifecycle.resumption = f.result.resumption;
    f.files['resumption-lifecycle.json'] = bytes(lifecycle);
    f.result.recovery.finalEvidenceSha256['resumption-lifecycle.json'] = hash(f.files['resumption-lifecycle.json']);
    f.sealResults();
    assert.throws(() => resolveRecoveryFixture(f.original, f.result, f.evidence));
  }
});

test('only the accepted historical revival is allowed; new retirement, generation or prefix changes fail', () => {
  const f = fixture(); const boundary = resolveRecoveryFixture(f.original, f.result, f.evidence)[0];
  assert.throws(() => nativeIdentity(f.events), /runtime_generation_changed/);
  assert.equal(nativeIdentity([...f.events, opening(5, 2)], boundary).openings, 3);
  for (const events of [
    [...f.events, event(5, { type: 'runtime_generation_retired', runtime_generation: 2 })],
    [...f.events, opening(5, 3)], [...f.events, opening(5, 2, 'copied-conversation')],
    f.events.slice(0, -1), [f.events[0], event(2, { type: 'message', text: 'edited' }), ...f.events.slice(2)],
  ]) assert.throws(() => nativeIdentity(events, boundary));
  const counterfeit = structuredClone(boundary); counterfeit.identity.recordedOpenings[0].runtimeGeneration = 0;
  assert.throws(() => nativeIdentity(f.events, counterfeit), /accepted_resumption_native_identity_changed/);
});

test('later rollout comparisons still reject runtime replacement, new generation and changed retained history', () => {
  const f = fixture(); const boundaries = resolveRecoveryFixture(f.original, f.result, f.evidence);
  const before = { snapshot: f.after.runtime, agents: f.after.public.slice(0, 2).map((item, i) => ({ ...item, native: nativeIdentity(f.events, boundaries[i]) })) };
  assert.equal(compareRetained(before, structuredClone(before)).status, 'passed');
  for (const mutate of [
    value => { value.snapshot.runtimes[0].uid = 'unexpected-replacement'; },
    value => { value.agents[0].native.runtimeGeneration++; },
    value => { value.agents[0].transcript.turns[1].owner_message = 'changed'; },
    value => { value.snapshot.agents[0].events[1].sha256 = digest('changed'); },
  ]) { const after = structuredClone(before); mutate(after); assert.throws(() => compareRetained(before, after)); }
});
