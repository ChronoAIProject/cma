// Resolve separately recorded recovery evidence without rewriting the failed fixture.
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { compareSnapshots, digest } from './cma-bridge-continuity-observer.mjs';
import { stableNative } from './cma-bridge-continuity-native.mjs';

const check = (condition, code) => { if (!condition) throw new Error(code); };
const same = (a, b) => digest(a) === digest(b);
// Recovery hashes retain JSON insertion order; observer event hashes are canonical.
const hash = (value) => createHash('sha256').update(Buffer.isBuffer(value) ? value : JSON.stringify(value)).digest('hex');
const evidenceNames = ['api-receipt.json', 'web-receipt.json', 'after-rollout.json', 'root-reconciliation.json', 'observation.json'];

export async function readRecoveryEvidence(directory, originalBytes) {
  const names = ['directive.json', 'journal.json', 'result.json', 'after.json', 'fresh-preflight.json', ...evidenceNames];
  const files = Object.fromEntries(await Promise.all(names.map(async name => [name, await readFile(join(directory, name))])));
  if (JSON.parse(files['result.json']).runtimeMode === 'recorded_pause_resumption') {
    files['resumption-lifecycle.json'] = await readFile(join(directory, 'resumption-lifecycle.json'));
  }
  return { originalBytes, files };
}

export function resolveRecoveryFixture(original, result, evidence) {
  check(evidence?.originalBytes && evidence.files, 'linked_recovery_evidence_required');
  const { files } = evidence;
  const json = name => JSON.parse(files[name]);
  const recovery = result.recovery;
  check(recovery && same(JSON.parse(evidence.originalBytes), original)
    && hash(evidence.originalBytes) === recovery.originalJournalSha256, 'original_journal_digest_changed');
  const journal = json('journal.json');
  const directive = json('directive.json');
  check(journal.schema === 'cma-nonadmission-followup-journal-v1'
    && directive.schema === 'cma-confirmed-nonadmission-followup-directive-v1'
    && recovery.id === journal.recoveryId && recovery.id === directive.recoveryId
    && journal.originalJournalSha256 === recovery.originalJournalSha256
    && directive.originalJournalSha256 === recovery.originalJournalSha256
    && hash(files['directive.json']) === recovery.directiveSha256
    && journal.directiveHash === recovery.directiveSha256
    && same(journal.result, result) && same(json('result.json'), result)
    && same(journal.resources, original.resources), 'recovery_result_linkage_changed');
  check(result.commit === result.resumedApplicationCommit
    && /^[a-f0-9]{40}$/.test(result.commit)
    && result.startedApplicationCommit === original.commit
    && result.startedApplicationCommit !== result.resumedApplicationCommit
    && [journal, directive].every(value => value.startedApplicationCommit === result.startedApplicationCommit
      && value.resumedApplicationCommit === result.resumedApplicationCommit)
    && same(result.turnApplicationCommits, Array(2).fill([original.commit, result.commit])), 'recovery_application_attribution_changed');
  check(recovery.originalForbiddenPreserved === true && recovery.exactOriginalKeyRetried === false
    && recovery.originalKeyDisposition === 'not_resubmitted' && result.originalFailedRunPreserved === true
    && directive.originalKeyWillNeverBeResubmitted === true && directive.newResourcesPermitted === 0
    && same(directive.plannedFinalTurns, [2, 2, 0])
    && journal.original403?.disposition === 'not_resubmitted'
    && same(journal.original403.command, original.commands['continue-0'])
    && original.commands['continue-0']?.state === 'unsettled'
    && original.commands['continue-0'].safeCode === 'public_http_403_forbidden', 'original_refusal_not_preserved');
  const label = journal.original403.replacementFollowupLabel;
  const first = journal.commands?.[label];
  const manual = journal.commands?.['continue-1'];
  check(label === 'continue-0-after-nonadmission-1' && same(Object.keys(journal.commands).sort(), [label, 'continue-1'].sort())
    && first.state === 'accepted' && first.status === 202 && first.attempts === 1
    && first.path === original.commands['continue-0'].path && first.body === original.commands['continue-0'].body
    && first.key === `${original.run}-${label}` && first.key === recovery.replacementFollowupKey
    && first.key !== original.commands['continue-0'].key
    && same(directive.followup, { path: first.path, body: first.body, key: first.key, status: first.status })
    && manual?.state === 'accepted' && manual.status === 200 && manual.attempts === 1
    && manual.path === '/api/v2/responses' && manual.key === `${original.run}-continue-1`
    && same(JSON.parse(manual.body), { conversation: original.resources.agentIds[1],
      input: JSON.parse(first.body).text, stream: true }), 'recovery_commands_changed');
  check(same(recovery.helperSha256, directive.helperSha256)
    && Object.keys(directive.helperSha256).length > 0
    && Object.values(directive.helperSha256).every(value => /^[a-f0-9]{64}$/.test(value))
    && same(recovery.deploymentEvidenceSha256, directive.evidenceSha256), 'recovery_evidence_linkage_changed');
  for (const name of evidenceNames) {
    check(hash(files[name]) === directive.evidenceSha256[name], 'recovery_evidence_digest_changed');
  }
  const observation = json('observation.json');
  const root = json('root-reconciliation.json');
  const fresh = json('fresh-preflight.json');
  check(root.status === 'confirmed_nonadmission' && root.applicationCommit === result.commit
    && root.originalJournalSha256 === recovery.originalJournalSha256
    && root.originalUnsettledCommandSha256 === hash(original.commands['continue-0'])
    && root.observationSha256 === hash(observation) && root.freshKeyAuthorized === first.key
    && root.originalExecutionCannotAppend === true && root.originalKeyWillNeverBeResubmitted === true
    && fresh.applicationCommit === result.commit && fresh.nonadmission?.originalCommandAbsent === true
    && hash(fresh) === recovery.freshNonadmissionSha256
    && journal.freshNonadmissionSha256 === recovery.freshNonadmissionSha256,
  'recovery_nonadmission_linkage_changed');
  const paused = result.runtimeMode === 'recorded_pause_resumption';
  check(['same_runtime', 'recorded_pause_resumption'].includes(result.runtimeMode)
    && directive.runtimeMode === result.runtimeMode && root.runtimeMode === result.runtimeMode,
  'recovery_runtime_mode_changed');
  const finalNames = ['after.json', ...(paused ? ['resumption-lifecycle.json'] : [])];
  check(same(Object.keys(recovery.finalEvidenceSha256 ?? {}).sort(), [...finalNames].sort()), 'recovery_final_evidence_required');
  for (const name of finalNames) check(hash(files[name]) === recovery.finalEvidenceSha256[name], 'recovery_final_evidence_digest_changed');
  const after = json('after.json');
  const ids = [...original.resources.agentIds, original.bootstrapAgentId];
  check(new Set(ids).size === 3 && /^agt_[A-Za-z0-9_-]+$/.test(original.bootstrapAgentId)
    && journal.bootstrapAgentId === original.bootstrapAgentId
    && same(result.allOwnedAgentIds, ids) && result.totalOwnedAgents === 3
    && same(result.completedTurns, [2, 2]) && same(result.origins, ['trigger', 'manual'])
    && result.sameSandbox === true && result.separateWorktrees === true && result.retainedTranscriptPrefix === true
    && same(result.bootstrapAgent, { id: ids[2], creationOrigin: 'legacy_unknown', workspaceProfileIndex: 0, completedTurns: 0, untouched: true })
    && after.public.length === 3 && new Set(after.public.map(item => item.agent.worktree.id)).size === 3,
  'recovery_owned_scope_changed');
  for (const [index, item] of after.public.entries()) {
    check(item.agent.id === ids[index] && item.agent.status === 'ready'
      && item.agent.sandbox.id === original.resources.sandboxId && item.agent.workspace.id === original.resources.workspaceId
      && item.transcript.agent_id === ids[index] && item.transcript.turns.length === (index < 2 ? 2 : 0)
      && item.transcript.turns.every(turn => turn.outcome.type === 'completed')
      && item.provenance.agent_id === ids[index] && item.provenance.creation_origin === ['trigger', 'manual', 'legacy_unknown'][index]
      && (index < 2 ? same(item.transcript.turns[0], original.baseline.firstTurns[index])
        : item.provenance.details.workspace_profile_index === 0 && !item.agent.first_message), 'recovery_public_history_changed');
  }
  check(after.native.length === 2 && same(after.native, result.nativeIdentity), 'recovery_native_evidence_changed');
  const comparison = compareSnapshots(original.baseline.before, after.runtime);
  check(same(comparison, result.continuity), 'original_runtime_comparison_changed');
  if (!paused) {
    check(comparison.status === 'passed' && same(stableNative(after.native), stableNative(original.baseline.nativeBefore)), 'recovery_runtime_changed');
    return undefined;
  }
  const lifecycle = json('resumption-lifecycle.json');
  const resumption = result.resumption;
  check(observation.status === 'recorded_pause' && fresh.status === 'recorded_pause'
    && root.allowImplicitRevivalFromMessage === true && root.pauseObservationSha256 === hash(observation.pause)
    && same(fresh.pause, observation.pause) && same(fresh.sandboxEvents, observation.sandboxEvents)
    && same(lifecycle.resumption, resumption) && resumption.status === 'resumed_after_recorded_pause'
    && resumption.checkpointEpoch === observation.pause.latestSnapshotEpoch
    && resumption.pausedAt === observation.pause.completedAt && resumption.originalRuntimeComparisonPreserved === true
    && resumption.applicationRolloutSameRuntimeContinuity === 'not_established_by_this_paused_canary'
    && same(resumption.priorRuntime, original.baseline.before.runtimes[0])
    && same(resumption.resumedRuntime, after.runtime.runtimes[0])
    && same(resumption.priorNative, original.baseline.nativeBefore) && same(resumption.resumedNative, after.native)
    && comparison.status === 'failed' && comparison.failures.length > 0
    && comparison.failures.every(failure => ['runtime_replaced_or_missing', 'runtime_changed'].includes(failure.code)),
  'recorded_resumption_evidence_changed');
  check(lifecycle.sandbox.id === original.resources.sandboxId && lifecycle.sandbox.owner === original.owner
    && lifecycle.sandbox.status === 'active' && lifecycle.sandbox.lifecycle.latest_snapshot.epoch === resumption.checkpointEpoch
    && lifecycle.events.every((event, index) => event.sandbox_id === original.resources.sandboxId && event.seq === index + 1)
    && same(lifecycle.events.slice(0, observation.sandboxEvents.length), observation.sandboxEvents), 'resumption_lifecycle_changed');
  const additions = lifecycle.events.slice(observation.sandboxEvents.length);
  check(additions.some(event => event.kind.type === 'revived' && event.appended_at === resumption.revivedAt)
    && additions.some(event => event.kind.type === 'status_changed' && event.kind.from === 'reviving' && event.kind.to === 'active')
    && !additions.some(event => event.kind.type === 'work_step_failed' || event.kind.to === 'failed')
    && after.runtime.runtimes.length === 1 && resumption.resumedRuntime.uid !== resumption.priorRuntime.uid
    && Number(resumption.resumedRuntime.generation) > Number(resumption.priorRuntime.generation), 'successful_recorded_revival_required');
  return after.native.map((identity, index) => {
    const prior = original.baseline.nativeBefore[index];
    const agent = after.runtime.agents.find(value => value.id === ids[index]);
    check(identity.conversationSha256 === prior.conversationSha256 && identity.runtimeGeneration > prior.runtimeGeneration
      && identity.recordedOpenings?.length > 1 && identity.openings === identity.recordedOpenings.length
      && identity.recordedOpenings.every(opening => opening.conversationSha256 === prior.conversationSha256)
      && identity.recordedOpenings[0].runtimeGeneration === prior.runtimeGeneration
      && identity.recordedOpenings.at(-1).runtimeGeneration === identity.runtimeGeneration
      && Array.isArray(identity.recordedRetirements) && agent.events.length > 0,
    'recorded_native_resumption_required');
    return { events: agent.events, identity };
  });
}
