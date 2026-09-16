// A terminal admitted failure remains a failure when a separately reconciled successor succeeds.
import { createHash } from 'node:crypto';
import { compareSnapshots, digest, eventPrefix } from './cma-bridge-continuity-observer.mjs';

export const check = (condition, code) => { if (!condition) throw new Error(code); };
export const hash = value => createHash('sha256').update(Buffer.isBuffer(value) || typeof value === 'string' ? value : JSON.stringify(value)).digest('hex');
const same = (a, b) => digest(a) === digest(b);
export const successorLabel = 'continue-0-after-terminal-nondispatch-1';
export const predecessorNames = ['predecessor-journal.json', 'predecessor-directive.json', 'predecessor-observation.json', 'terminal-capture.json'];
export const successorEvidenceNames = ['api-receipt.json', 'web-receipt.json', 'after-rollout.json', 'root-reconciliation.json', 'observation.json', ...predecessorNames];

export function validateTerminalRow(row, original) {
  check(row?._id && row.agent_id === original.resources.agentIds[0] && row.status === 'failed'
    && row.record?.work?.failure?.code === 'authority_unavailable'
    && Date.parse(row.record.work.failure.at) >= Date.parse(row.record.receipt?.admitted_at)
    && typeof row.response_id === 'string', 'terminal_authority_failure_required');
  return { commandId: row._id, responseId: row.response_id, status: row.status,
    admittedAt: row.record.receipt.admitted_at, failure: row.record.work.failure };
}

export function validatePredecessor(original, files, expected) {
  for (const name of predecessorNames) check(hash(files[name]) === expected[name], 'predecessor_raw_digest_changed');
  const journal = JSON.parse(files['predecessor-journal.json']);
  const directive = JSON.parse(files['predecessor-directive.json']);
  const observation = JSON.parse(files['predecessor-observation.json']);
  const capture = JSON.parse(files['terminal-capture.json']);
  const label = 'continue-0-after-nonadmission-1';
  const command = journal.commands?.[label];
  check(journal.schema === 'cma-nonadmission-followup-journal-v1' && !journal.result
    && directive.schema === 'cma-confirmed-nonadmission-followup-directive-v1'
    && journal.recoveryId === directive.recoveryId && journal.directiveHash === hash(files['predecessor-directive.json'])
    && journal.originalJournalSha256 === directive.originalJournalSha256
    && hash(files['predecessor-observation.json']) === directive.evidenceSha256['observation.json']
    && same(journal.resources, original.resources) && journal.bootstrapAgentId === original.bootstrapAgentId
    && same(Object.keys(journal.commands), [label]) && command.state === 'accepted' && command.status === 202
    && command.attempts === 1 && command.value?.queued === true
    && command.key === `${original.run}-${label}` && command.path === original.commands['continue-0'].path
    && command.body === original.commands['continue-0'].body
    && same(directive.followup, { path: command.path, body: command.body, key: command.key, status: 202 })
    && same(journal.original403.command, original.commands['continue-0'])
    && original.commands['continue-0'].state === 'unsettled'
    && original.commands['continue-0'].safeCode === 'public_http_403_forbidden', 'failed_predecessor_changed');
  const rows = capture.results?.owned_followup_command_status;
  check(capture.schema === 'cma-owned-command-status-read-v1' && capture.status === 'captured'
    && capture.mutatingCommands === 0 && capture.readConcern === 'majority' && capture.readPreference === 'primary'
    && capture.effectiveReadConcern === 'snapshot' && capture.snapshot?.majorityCommitted === true
    && rows?.complete === true && rows.rows.length === 1, 'terminal_capture_required');
  return { journal, directive, observation, terminal: validateTerminalRow(rows.rows[0], original) };
}

export function validatePublicHistory(original, values, baselinePublic, counts, status) {
  const ids = [...original.resources.agentIds, original.bootstrapAgentId];
  check(new Set(ids).size === 3 && values?.length === 3 && baselinePublic?.length === 3
    && same(counts.slice(2), [0]), 'exact_three_owned_agents_required');
  values.forEach((item, index) => {
    const baseline = baselinePublic[index];
    check(item.agent.id === ids[index] && item.agent.status === status
      && item.agent.sandbox.id === original.resources.sandboxId && item.agent.workspace.id === original.resources.workspaceId
      && same(item.agent.worktree, baseline.agent.worktree) && same(item.provenance, baseline.provenance)
      && same(item.agent.profile, baseline.agent.profile) && item.agent.created_at === baseline.agent.created_at
      && item.transcript.agent_id === ids[index] && item.transcript.turns.length === counts[index]
      && item.transcript.turns.every(turn => turn.outcome.type === 'completed')
      && item.provenance.creation_origin === ['trigger', 'manual', 'legacy_unknown'][index], 'successor_public_scope_changed');
    if (index < 2) {
      check(same(item.transcript.turns[0], original.baseline.firstTurns[index]), 'original_turn_changed');
      if (counts[index] === 2) {
        const turn = item.transcript.turns[1];
        check(turn.owner_message === JSON.parse(original.commands['continue-0'].body).text
          && turn.messages.filter(message => message.role === 'assistant').map(message => message.content).join('\n')
            .includes(original.baseline.firstTurns[index].agent_messages[0]), 'successor_marker_or_input_changed');
      }
    }
    else check(item.provenance.details.workspace_profile_index === 0 && !item.agent.first_message, 'bootstrap_changed');
  });
  check(new Set(values.map(item => item.agent.worktree.id)).size === 3, 'owned_worktrees_changed');
}

// Full sandbox events document each generation advance; raw Agent events bind native IDs to the retained prefix.
export function validateRecordedHistory(original, state, prior) {
  const { sandbox, sandboxEvents: events, agentEvents } = state;
  check(sandbox?.id === original.resources.sandboxId && sandbox.owner === original.owner
    && ['active', 'paused'].includes(sandbox.status) && Array.isArray(events) && events.length > 0
    && events.every((event, index) => event.seq === index + 1 && event.sandbox_id === sandbox.id
      && Number.isFinite(Date.parse(event.appended_at)))
    && Array.isArray(agentEvents) && agentEvents.length === 3, 'complete_owned_history_required');
  check(sandbox.agent_count === 3 && sandbox.worktree_count === 3 && sandbox.workspace_count === 1
    && sandbox.workspaces?.length === 1 && sandbox.workspaces[0].id === original.resources.workspaceId,
  'sandbox_owned_inventory_changed');
  agentEvents.forEach((items,index) => check(items.length > 0
    && items.every((event,i) => event.seq === i+1 && Number.isFinite(Date.parse(event.appended_at)))
    && state.public[index].transcript.through_seq === items.at(-1).seq, 'complete_agent_transcript_boundary_required'));
  if (prior) {
    check(same(events.slice(0, prior.sandboxEvents.length), prior.sandboxEvents), 'sandbox_history_prefix_changed');
    if (prior.agentEvents) agentEvents.forEach((items, index) =>
      check(same(items.slice(0, prior.agentEvents[index].length), prior.agentEvents[index]), 'agent_history_prefix_changed'));
  }
  const cycles = []; let sealing; let pause; let revival;
  for (const event of events) {
    const kind = event.kind;
    check(kind.to !== 'failed' && !['rotated', 'deleted'].includes(kind.type), 'unexplained_lifecycle_failure_or_rotation');
    if (kind.type === 'work_step_failed') check(kind.failure?.class === 'retryable_transient'
      && kind.step?.kind === 'run_lifecycle_operation'
      && events.some(later => later.seq > event.seq && later.kind.type === 'work_step_completed' && same(later.kind.step, kind.step)),
    'unresolved_or_terminal_lifecycle_failure');
    if (kind.type === 'status_changed' && kind.to === 'pausing') { check(!pause && !revival, 'overlapping_pause'); sealing = event; }
    if (kind.type === 'checkpoint_sealed') { check(sealing, 'checkpoint_without_pause'); sealing = { ...sealing, sealed: event }; }
    if (kind.type === 'status_changed' && kind.to === 'paused') { check(sealing?.sealed && !pause, 'pause_without_checkpoint'); pause = { start: sealing, paused: event }; }
    if (kind.type === 'status_changed' && kind.to === 'reviving') { check(pause && !revival, 'revival_without_recorded_pause'); revival = event; }
    if (kind.type === 'revived') {
      check(pause && revival && events.some(value => value.seq >= revival.seq && value.seq < event.seq
        && value.kind.type === 'status_changed' && value.kind.from === 'reviving' && value.kind.to === 'active'), 'revival_without_active_transition');
      cycles.push({ ...pause, revival, revived: event }); pause = null; revival = null; sealing = null;
    }
  }
  check(!revival && (sandbox.status === 'paused' ? !!pause : !pause && !sealing), 'settled_lifecycle_required');
  const checkpoints = events.filter(event => event.kind.type === 'checkpoint_sealed');
  check(sandbox.lifecycle?.latest_snapshot?.epoch === checkpoints.length && checkpoints.length > 0
    && sandbox.lifecycle.latest_snapshot.saved_at === checkpoints.at(-1).appended_at, 'actual_checkpoint_history_required');
  const native = agentEvents.slice(0, 2).map((items, index) => {
    const originalIdentity = original.baseline.nativeBefore[index];
    const baseline = original.baseline.before.agents.find(agent => agent.id === original.resources.agentIds[index]);
    check(items.every((event, i) => event.seq === i + 1 && Number.isFinite(Date.parse(event.appended_at)))
      && same(eventPrefix(items.slice(0, baseline.events.length)), baseline.events), 'original_agent_event_prefix_changed');
    let generation = originalIdentity.runtimeGeneration;
    const openings = []; const retirements = []; const reopenedCycles = new Set(); let pendingCycle;
    const cycleAt = (at, opening = false) => cycles.findIndex(cycle => Date.parse(at) >= Date.parse((opening ? cycle.revival : cycle.start).appended_at)
      && Date.parse(at) <= Date.parse(cycle.revived.appended_at));
    for (const event of items) {
      const kind = event.kind;
      if (kind.type === 'runtime_generation_retired') {
        const cycle = cycleAt(event.appended_at);
        check(kind.previous_generation === generation && kind.runtime_generation === generation + 1
          && cycle >= 0 && !reopenedCycles.has(cycle), 'unrecorded_generation_change');
        generation = kind.runtime_generation;
        pendingCycle = cycle;
        retirements.push({ sequence: event.seq, at: event.appended_at, ...kind });
      }
      if (kind.type === 'conversation_opened') {
        check(typeof kind.conversation_id === 'string' && hash(kind.conversation_id) === originalIdentity.conversationSha256
          && Number.isSafeInteger(kind.runtime_generation) && kind.runtime_generation >= generation, 'native_conversation_or_generation_changed');
        if (kind.runtime_generation > generation || pendingCycle !== undefined) {
          const cycle = cycleAt(event.appended_at, true);
          check(cycle >= 0 && !reopenedCycles.has(cycle) && (pendingCycle === undefined || pendingCycle === cycle), 'unrecorded_native_reopen');
          reopenedCycles.add(cycle); generation = kind.runtime_generation; pendingCycle = undefined;
        }
        openings.push({ sequence: event.seq, at: event.appended_at, conversationSha256: hash(kind.conversation_id), runtimeGeneration: generation });
      }
    }
    check(openings.length >= originalIdentity.openings && openings.at(-1).runtimeGeneration === generation
      && reopenedCycles.size === cycles.length && pendingCycle === undefined,
    'complete_generation_history_required');
    return { conversationSha256: originalIdentity.conversationSha256, runtimeGeneration: generation, openings: openings.length,
      recordedOpenings: openings, recordedRetirements: retirements };
  });
  if (state.runtime) {
    const comparison = compareSnapshots(original.baseline.before, state.runtime);
    check(comparison.failures.every(failure => ['runtime_replaced_or_missing', 'runtime_changed'].includes(failure.code))
      && state.runtime.runtimes.length === 1 && native.every(identity => Number(state.runtime.runtimes[0].generation) === identity.runtimeGeneration)
      && state.runtime.agents.every((agent, index) => same(agent.events, eventPrefix(agentEvents[index]))), 'runtime_history_or_lineage_changed');
  }
  return { native, cycles: cycles.length, checkpointEpoch: checkpoints.length, pausedAt: pause?.paused.appended_at ?? null };
}

export function historyBoundary(state) {
  return { public: state.public.map(item => ({ worktree: item.agent.worktree, transcript: item.transcript, provenance: item.provenance })),
    sandboxEvents: state.sandboxEvents, agentEvents: state.agentEvents, checkpoint: state.sandbox.lifecycle.latest_snapshot,
    status: state.sandbox.status, runtime: state.runtime ? { scope: state.runtime.scope, agents: state.runtime.agents, runtimes: state.runtime.runtimes } : null,
    native: state.native };
}

export function resolveSuccessorFixture(original, result, evidence) {
  const { files, originalBytes } = evidence;
  const json = name => JSON.parse(files[name]);
  const recovery = result.recovery; const directive = json('directive.json'); const journal = json('journal.json');
  check(same(JSON.parse(originalBytes), original) && hash(originalBytes) === recovery.originalJournalSha256
    && journal.originalJournalSha256 === recovery.originalJournalSha256 && directive.originalJournalSha256 === recovery.originalJournalSha256,
  'original_journal_digest_changed');
  check(directive.schema === 'cma-terminal-nondispatch-successor-directive-v1' && journal.schema === 'cma-terminal-nondispatch-successor-journal-v1'
    && directive.recoveryId === recovery.id && journal.recoveryId === recovery.id
    && hash(files['directive.json']) === recovery.directiveSha256 && journal.directiveHash === recovery.directiveSha256
    && same(journal.result, result) && same(json('result.json'), result)
    && same(journal.resources, original.resources) && journal.bootstrapAgentId === original.bootstrapAgentId, 'successor_result_linkage_changed');
  check(same(recovery.predecessorEvidenceSha256, directive.predecessorEvidenceSha256)
    && same(journal.predecessorEvidenceSha256, directive.predecessorEvidenceSha256), 'successor_predecessor_linkage_changed');
  const predecessor = validatePredecessor(original, files, recovery.predecessorEvidenceSha256);
  check(predecessor.journal.originalJournalSha256 === recovery.originalJournalSha256
    && same(journal.predecessorFailure, predecessor.terminal) && same(recovery.predecessorFailure, predecessor.terminal)
    && same(journal.original403.command, original.commands['continue-0']) && journal.original403.disposition === 'not_resubmitted'
    && directive.priorKeysWillNeverBeResubmitted === true && recovery.priorKeysRetried === false
    && recovery.originalForbiddenPreserved === true && recovery.exactOriginalKeyRetried === false
    && recovery.originalKeyDisposition === 'not_resubmitted' && result.originalFailedRunPreserved === true,
  'prior_failed_attempts_not_preserved');
  check(/^[a-f0-9]{40}$/.test(result.commit) && result.commit === result.resumedApplicationCommit
    && result.commit !== predecessor.journal.resumedApplicationCommit && result.commit !== original.commit
    && result.startedApplicationCommit === original.commit
    && [directive, journal].every(value => value.startedApplicationCommit === original.commit && value.resumedApplicationCommit === result.commit)
    && same(result.turnApplicationCommits, Array(2).fill([original.commit, result.commit])), 'successor_application_attribution_changed');
  check(same(Object.keys(journal.commands).sort(), [successorLabel, 'continue-1'].sort())
    && directive.pollSeconds === 15 && directive.newResourcesPermitted === 0 && same(directive.plannedFinalTurns, [2, 2, 0]), 'successor_scope_changed');
  const first = journal.commands[successorLabel]; const manual = journal.commands['continue-1'];
  check(first.state === 'accepted' && first.attempts === 1 && first.status === 202 && first.value?.queued === true
    && first.path === original.commands['continue-0'].path && first.body === original.commands['continue-0'].body
    && first.key === `${original.run}-${successorLabel}` && first.key === recovery.replacementFollowupKey
    && same(directive.followup, { path: first.path, body: first.body, key: first.key, status: 202 })
    && manual.state === 'accepted' && manual.attempts === 1 && manual.status === 200
    && manual.path === '/api/v2/responses' && manual.key === `${original.run}-continue-1`
    && same(JSON.parse(manual.body), { conversation: original.resources.agentIds[1], input: JSON.parse(first.body).text, stream: true }),
  'successor_commands_changed');
  check(same(recovery.helperSha256, directive.helperSha256) && Object.keys(directive.helperSha256).length > 0
    && Object.values(directive.helperSha256).every(value => /^[a-f0-9]{64}$/.test(value))
    && same(recovery.deploymentEvidenceSha256, directive.evidenceSha256), 'successor_evidence_linkage_changed');
  for (const name of successorEvidenceNames) check(hash(files[name]) === directive.evidenceSha256[name], 'successor_evidence_digest_changed');
  const root = json('root-reconciliation.json'); const observation = json('observation.json'); const fresh = json('fresh-preflight.json');
  check(root.status === 'confirmed_terminal_nondispatch' && root.applicationCommit === result.commit
    && root.originalJournalSha256 === recovery.originalJournalSha256 && root.originalExecutionCannotAppend === true
    && root.predecessorExecutionCannotAppend === true && root.priorKeysWillNeverBeResubmitted === true
    && same(root.predecessorFailure, predecessor.terminal) && same(root.predecessorEvidenceSha256, recovery.predecessorEvidenceSha256)
    && root.sourceProof?.applicationCommit === predecessor.journal.resumedApplicationCommit
    && root.sourceProof.terminalBeforeDispatch === true && /^[a-f0-9]{64}$/.test(root.sourceProof.blobSha256)
    && typeof root.sourceProof.path === 'string' && root.sourceProof.path.length > 0
    && root.freshKeyAuthorized === first.key && root.observationSha256 === hash(observation)
    && fresh.applicationCommit === result.commit && hash(fresh) === recovery.freshPreflightSha256
    && journal.freshPreflightSha256 === recovery.freshPreflightSha256
    && same(historyBoundary(fresh), historyBoundary(observation))
    && fresh.nonadmission?.originalCommandAbsent === true && fresh.nonadmission?.predecessorTerminalUnchanged === true
    && fresh.nonadmission?.proposedKeyUnused === true, 'terminal_nondispatch_reconciliation_required');
  const finalNames = ['after.json', 'resumption-lifecycle.json'];
  check(same(Object.keys(recovery.finalEvidenceSha256).sort(), finalNames.sort()), 'successor_final_evidence_required');
  for (const name of finalNames) check(hash(files[name]) === recovery.finalEvidenceSha256[name], 'successor_final_evidence_digest_changed');
  const after = json('after.json'); const lifecycle = json('resumption-lifecycle.json');
  check(same(after.sandbox, lifecycle.sandbox) && same(after.sandboxEvents, lifecycle.events), 'final_lifecycle_linkage_changed');
  validatePublicHistory(original, observation.public, predecessor.observation.public, [1, 1, 0], observation.sandbox.status === 'paused' ? 'paused' : 'ready');
  validatePublicHistory(original, after.public, predecessor.observation.public, [2, 2, 0], 'ready');
  const beforeHistory = validateRecordedHistory(original, observation, predecessor.observation);
  const finalHistory = validateRecordedHistory(original, after, fresh);
  const paused = observation.sandbox.status === 'paused';
  check(result.runtimeMode === 'recorded_lifecycle_successor' && directive.runtimeMode === result.runtimeMode
    && root.runtimeMode === result.runtimeMode && (!paused || root.allowImplicitRevivalFromMessage === true)
    && finalHistory.cycles === beforeHistory.cycles + Number(paused) && after.sandbox.status === 'active'
    && same(after.native, finalHistory.native) && same(result.nativeIdentity, finalHistory.native)
    && same(result.lifecycle, { before: beforeHistory, after: finalHistory })
    && same(result.continuity, compareSnapshots(original.baseline.before, after.runtime)), 'successor_recorded_history_changed');
  if (!paused) check(compareSnapshots(fresh.runtime, after.runtime).status === 'passed', 'successor_runtime_changed');
  const ids = [...original.resources.agentIds, original.bootstrapAgentId];
  check(same(result.allOwnedAgentIds, ids) && result.totalOwnedAgents === 3 && same(result.completedTurns, [2, 2])
    && same(result.origins, ['trigger', 'manual']) && result.sameSandbox === true && result.separateWorktrees === true
    && result.retainedTranscriptPrefix === true
    && same(result.bootstrapAgent, { id: ids[2], creationOrigin: 'legacy_unknown', workspaceProfileIndex: 0, completedTurns: 0, untouched: true }),
  'successor_owned_scope_changed');
  return after.native.map((identity, index) => ({ events: eventPrefix(after.agentEvents[index]), identity }));
}
