// Retained canary capture/continuation across one explicitly named deployment change.
// Separate journal: the original four-turn canary and its cleanup are untouched.
import { constants } from 'node:fs';
import { open, readFile, rename, mkdir, chmod, unlink } from 'node:fs/promises';
import { join } from 'node:path';
import { completedResponse, admittedResponse } from './cma-bridge-continuity-responses.mjs';
import { retainedCommand, loadOwnedConfig, loadOwnedFixture } from './cma-bridge-continuity-owned-config.mjs';
import { capture, digest } from './cma-bridge-continuity-observer.mjs';
import { transport } from './cma-bridge-continuity-transport.mjs';
import { nativeIdentity, stableNative, compareRetained } from './cma-bridge-continuity-native.mjs';
import { activeTurnSpec, checkActiveReading, checkOwnedTurnCounts, ACTIVE_START_SECONDS } from './cma-bridge-continuity-active.mjs';

const [configPath, action, change, commit] = process.argv.slice(2);
const requireValue = (condition, code) => { if (!condition) throw new Error(code); };
requireValue(['capture', 'continue', 'start-active', 'observe-active', 'recover-active'].includes(action), 'known_continuation_action_required');
requireValue(/^[a-z0-9][a-z0-9-]{0,47}$/.test(change ?? ''), 'bounded_change_identity_required');
requireValue(/^[0-9a-f]{7,40}$/.test(commit ?? ''), 'explicit_observed_commit_required');
const config = await loadOwnedConfig(configPath);
const root = config.outputDirectory;
const directory = join(root, 'continuations', change);
let token;
let journal;
let original;
let fixture;
let lock;

async function save(name, value) {
  const path = join(directory, name);
  const temporary = path + '.tmp';
  const handle = await open(temporary, 'w', 0o600);
  try { await handle.writeFile(JSON.stringify(value, null, 2) + '\n'); await handle.sync(); }
  finally { await handle.close(); }
  await chmod(temporary, 0o600);
  await rename(temporary, path);
  const parent = await open(directory, 'r');
  try { await parent.sync(); } finally { await parent.close(); }
}

async function request(path, method = 'GET', body, key, stream) {
  requireValue(path.startsWith('/api/v1/agents/') || path === '/auth/session'
    || (path === '/api/v2/responses' && method === 'POST' && stream)
    || (method === 'GET' && /^\/api\/v2\/(?:responses|commands)\/[A-Za-z0-9_-]{1,200}$/.test(path)), 'public_canary_path_required');
  let response;
  try {
    response = await fetch(config.origin + path, {
      method, redirect: 'error', signal: AbortSignal.timeout(stream && !stream.admissionOnly ? config.limits.turnSeconds * 1000 : 30_000),
      headers: { Authorization: `Bearer ${token}`, 'User-Agent': 'Mozilla/5.0',
        Accept: stream ? 'text/event-stream' : 'application/json',
        ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
        ...(key === undefined ? {} : { 'Idempotency-Key': key }) },
      ...(body === undefined ? {} : { body }),
    });
  } catch { throw new Error('public_request_outcome_unknown'); }
  if (response.ok && stream) return { status: response.status, value: stream.admissionOnly
    ? await admittedResponse(response, stream, stream.observe)
    : await completedResponse(response, stream, stream.observe, token) };
  let text;
  try { text = await response.text(); } catch { throw new Error('public_response_outcome_unknown'); }
  requireValue(Buffer.byteLength(text) < 4 * 1024 * 1024 && !text.includes(token), 'unsafe_or_oversized_response');
  let value;
  try { value = text ? JSON.parse(text) : null; } catch { throw new Error('invalid_public_json'); }
  requireValue(response.ok, `public_http_${response.status}`);
  return { status: response.status, value };
}
const read = async (path) => (await request(path)).value;

async function preflight() {
  fixture = await loadOwnedFixture(config);
  original = fixture.journal;
  const handle = await open(config.tokenFile, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const stat = await handle.stat();
    requireValue(stat.isFile() && stat.uid === process.getuid() && !(stat.mode & 0o077) && stat.size <= 16_384, 'private_saved_token_required');
    token = (await handle.readFile('utf8')).trim();
    requireValue(token && !/\s/.test(token), 'invalid_saved_token');
  } finally { await handle.close(); }
  const session = await read('/auth/session');
  requireValue(session.authenticated === true && session.visitor === false && session.subject === config.expectedSubject, 'owner_identity_changed');
}

async function events(id) {
  const items = [];
  let after = 0;
  for (let page = 0; page < 100; page++) {
    const result = await read(`/api/v1/agents/${id}/events?after=${after}&limit=100`);
    requireValue(Array.isArray(result.items), 'invalid_event_page');
    items.push(...result.items);
    if (result.next == null) return items;
    requireValue(result.next > after, 'nonadvancing_event_cursor');
    after = result.next;
  }
  throw new Error('event_page_limit');
}

const marker = (index) => index ? config.markers.manual : config.markers.trigger;
const assistantText = (turn) => turn.messages.filter((message) => message.role === 'assistant').map((message) => message.content).join('\n');

async function observed() {
  const resources = original.resources;
  const agents = await Promise.all(resources.agentIds.map(async (id, index) => {
    const [agent, transcript, provenance, history] = await Promise.all([
      read(`/api/v1/agents/${id}`), read(`/api/v1/agents/${id}/transcript`),
      read(`/api/v1/agents/${id}/provenance`), events(id),
    ]);
    requireValue(agent.id === id && agent.sandbox.id === resources.sandboxId
      && agent.workspace.id === resources.workspaceId, 'owned_agent_lineage_changed');
    requireValue(provenance.agent_id === id && provenance.creation_origin === (index ? 'manual' : 'trigger'), 'owned_agent_origin_changed');
    requireValue(Array.isArray(transcript.turns) && transcript.turns.length >= 2
      && digest(transcript.turns[0]) === digest(original.baseline.firstTurns[index]), 'original_transcript_changed');
    return { agent, transcript, native: nativeIdentity(history, fixture.nativeBoundaries?.[index]) };
  }));
  requireValue(agents[0].agent.worktree.id !== agents[1].agent.worktree.id, 'agents_share_worktree');
  const observer = { ...config, schema: 'cma-bridge-continuity-config-v1', agentIds: resources.agentIds };
  const snapshot = await capture(observer, await transport(observer));
  requireValue(digest(stableNative(agents.map((value) => value.native)))
    === digest(stableNative(fixture.result.nativeIdentity)), 'original_native_identity_changed');
  return { at: new Date().toISOString(), agents, snapshot };
}

const compare = compareRetained;

async function command(label, path, data, accepted, stream) {
  const body = JSON.stringify(data);
  const key = `${config.run}-${change}-${label}`;
  const prior = journal.commands[label];
  const recovered = retainedCommand(prior, path, body, key);
  if (recovered !== undefined) return recovered;
  journal.commands[label] = { ...prior, path, body, key, state: 'pending', attempts: (prior?.attempts ?? 0) + 1 };
  await save('journal.json', journal);
  try {
    const observe = async (receipt) => {
      const retained = journal.commands[label].streamReceipt;
      requireValue(!retained || (retained.responseId === receipt.responseId && retained.commandId === receipt.commandId
        && retained.conversation === receipt.conversation), 'v2_retained_receipt_changed');
      journal.commands[label].streamReceipt = receipt;
      await save('journal.json', journal);
    };
    const response = await request(path, 'POST', body, key, stream && { ...stream, observe });
    requireValue(response.status === accepted, 'unexpected_success_status');
    Object.assign(journal.commands[label], { state: 'accepted', status: response.status, value: response.value });
    await save('journal.json', journal);
    console.log(JSON.stringify({ stage: label, status: 'accepted' }));
    return response.value;
  } catch (error) {
    Object.assign(journal.commands[label], { state: 'unsettled', safeCode: safeCode(error) });
    await save('journal.json', journal);
    throw error;
  }
}
const safeCode = (error) => /^[a-z0-9_]+$/.test(error.message) ? error.message : 'continuation_failed';

async function completed(index) {
  const id = original.resources.agentIds[index];
  const count = journal.before.agents[index].transcript.turns.length + 1;
  const deadline = Date.now() + config.limits.turnSeconds * 1000;
  let announced = 0;
  while (Date.now() < deadline) {
    const [agent, transcript] = await Promise.all([read(`/api/v1/agents/${id}`), read(`/api/v1/agents/${id}/transcript`)]);
    requireValue(!['failed', 'stopped', 'deleted', 'waiting_approval'].includes(agent.status), 'agent_not_executable');
    requireValue(transcript.turns.length <= count, 'unexpected_extra_turn');
    if (transcript.turns.length === count && transcript.turns.at(-1).outcome.type !== 'active') {
      requireValue(transcript.turns.every((turn) => turn.outcome.type === 'completed'), 'unexpected_turn_outcome');
      requireValue(assistantText(transcript.turns.at(-1)).includes(marker(index)), 'original_marker_not_recalled');
      if (agent.status === 'ready') return;
    }
    if (Date.now() - announced >= 20_000) {
      console.log(JSON.stringify({ stage: `continued_agent_${index}`, status: 'waiting' })); announced = Date.now();
    }
    await new Promise((resolve) => setTimeout(resolve, config.limits.pollSeconds * 1000));
  }
  throw new Error('continued_turn_timeout');
}

async function activeReading() {
  const record = journal.commands['active-turn'];
  const receipt = record?.value ?? record?.streamReceipt;
  requireValue(receipt?.responseId && receipt?.commandId, 'active_retained_receipt_required');
  const id = original.resources.agentIds[1];
  const [agent, transcript, history, response, status] = await Promise.all([
    read(`/api/v1/agents/${id}`), read(`/api/v1/agents/${id}/transcript`), events(id),
    read(`/api/v2/responses/${receipt.responseId}`), read(`/api/v2/commands/${receipt.commandId}`),
  ]);
  const reading = { at: new Date().toISOString(), agent, transcript, history, response, command: status };
  // Retain raw observations before interpretation so a failed check is reviewable.
  await save('active-latest-reading.json', reading);
  const proof = checkActiveReading(reading, { agentId: id, receipt,
    beforeTurns: journal.before.agents[1].transcript.turns.length, spec: journal.active.spec,
    retained: journal.active.staged?.proof });
  return { reading, proof };
}

async function waitActive(phase, seconds) {
  const deadline = Date.now() + seconds * 1000;
  let announced = 0;
  while (Date.now() < deadline) {
    const value = await activeReading();
    if (value.proof.phase === phase) return value;
    if (phase === 'active') requireValue(!['finishing', 'settling', 'completed'].includes(value.proof.phase), 'active_tool_finished_before_rollout');
    if (Date.now() - announced >= 20_000) {
      console.log(JSON.stringify({ stage: `active_${phase}`, status: value.proof.phase }));
      announced = Date.now();
    }
    await new Promise((resolve) => setTimeout(resolve, config.limits.pollSeconds * 1000));
  }
  throw new Error(phase === 'active' ? 'active_tool_start_timeout' : 'active_tool_completion_timeout');
}

async function activeCheckpoint(name) {
  const observation = await observed();
  compare(journal.before, observation);
  checkOwnedTurnCounts(journal.before, observation);
  const value = await activeReading();
  requireValue(value.proof.phase === 'active', 'active_tool_not_running_at_checkpoint');
  if (journal.active.staged) compare(journal.active.staged.observation, observation, true);
  const checkpoint = { at: value.reading.at, commit, observation, ...value };
  const index = journal.active.checkpoints.length + 1;
  const filename = `active-${name}-${index}.json`;
  await save(filename, checkpoint);
  journal.active.checkpoints.push({ name, filename, at: checkpoint.at, commit });
  if (!journal.active.staged) journal.active.staged = checkpoint;
  if (name === 'armed') journal.active.armed = checkpoint;
  await save('journal.json', journal);
  console.log(JSON.stringify({ status: name === 'armed' ? 'active_turn_armed_for_rollout' : 'active_turn_staged',
    change, responseId: value.proof.responseId, commandId: value.proof.commandId,
    turnSequence: value.proof.turnSequence, toolCallId: value.proof.toolCallId,
    activeToolCallId: value.proof.activeToolCallId,
    checkpoint: filename, observationOnly: name === 'armed' }));
}

async function executeActive() {
  requireValue(journal?.before, 'prechange_capture_required');
  if (action === 'recover-active' && journal.result?.status === 'passed'
    && journal.result.mode === 'active_owned_tool_turn') {
    requireValue(journal.afterCommit === commit, 'after_commit_changed');
    console.log(JSON.stringify({ status: 'already_passed', mode: journal.result.mode, change, additionalTurns: 1 }));
    return;
  }
  requireValue(!journal.result, 'unused_prechange_capture_required');
  requireValue(!journal.commands['continue-0'] && !journal.commands['continue-1'], 'active_and_idle_modes_cannot_mix');
  requireValue(/^[0-9a-f]{40}$/.test(commit), 'active_full_commit_required');
  if (action !== 'recover-active') requireValue(journal.beforeCommit === commit, 'before_commit_changed');
  if (action === 'start-active') {
    if (!journal.active) {
      const beforeStart = await observed();
      compare(journal.before, beforeStart);
      requireValue(beforeStart.agents.every((value, index) => value.agent.status === 'ready'
        && digest(value.transcript.turns) === digest(journal.before.agents[index].transcript.turns)), 'quiet_unchanged_owned_pair_required');
      journal.active = { schema: 'cma-active-owned-turn-v1', spec: activeTurnSpec(change, marker(1)), checkpoints: [] };
      await save('journal.json', journal);
    }
    requireValue(!journal.active.armed, 'active_turn_already_armed_use_observation');
    await command('active-turn', '/api/v2/responses', {
      conversation: original.resources.agentIds[1], input: journal.active.spec.prompt, stream: true,
    }, 200, { conversation: original.resources.agentIds[1], admissionOnly: true });
    await waitActive('active', ACTIVE_START_SECONDS);
    await activeCheckpoint('staged');
    return;
  }
  requireValue(journal.active && journal.commands['active-turn'], 'active_turn_not_staged');
  if (action === 'observe-active') {
    // Also permits read-only inspection if headers were saved before uncertainty.
    await activeCheckpoint('armed');
    return;
  }
  requireValue(journal.active.staged && journal.active.armed, 'fresh_prerollout_active_checkpoint_required');
  requireValue(commit !== journal.beforeCommit, 'different_after_commit_required');
  requireValue(!journal.afterCommit || journal.afterCommit === commit, 'after_commit_changed');
  journal.afterCommit = commit;
  await save('journal.json', journal);
  const afterReady = await observed();
  compare(journal.before, afterReady);
  compare(journal.active.armed.observation, afterReady, true);
  checkOwnedTurnCounts(journal.before, afterReady);
  const readyReading = await activeReading();
  if (!journal.active.afterReady) {
    journal.active.afterReady = { at: readyReading.reading.at, commit, observation: afterReady, ...readyReading };
    await save('active-after-ready.json', journal.active.afterReady);
    await save('journal.json', journal);
  }
  // No POST, cancel, restart, lifecycle request, or replacement prompt here.
  await waitActive('completed', journal.active.spec.sleepSeconds + config.limits.turnSeconds);
  const after = await observed();
  const continuity = compare(journal.active.armed.observation, after, true);
  compare(journal.before, after);
  checkOwnedTurnCounts(journal.before, after);
  const completed = await activeReading();
  requireValue(completed.proof.phase === 'completed', 'active_final_completion_changed');
  const result = { status: 'passed', mode: 'active_owned_tool_turn', change, at: completed.reading.at,
    beforeCommit: journal.beforeCommit, afterCommit: commit, resources: original.resources,
    additionalTurns: 1, submittedRequests: 1, responseId: completed.proof.responseId,
    commandId: completed.proof.commandId, turnSequence: completed.proof.turnSequence,
    toolCallId: completed.proof.toolCallId, armedAt: journal.active.armed.at,
    armedToolCallId: journal.active.armed.proof.activeToolCallId, observedToolCalls: completed.proof.observedToolCalls,
    sameResponseCompleted: true, siblingTurnsUnchanged: true, nativeIdentity: after.agents.map((value) => value.native),
    continuity, limitations: ['The coordinator must correlate the armed checkpoint with actual rollout start/readiness receipts; SHA arguments are assertions.',
      'Public events identify the original launch tool and bounded same-session poll summaries, not exactly-once physical execution or the full shell/poll arguments.',
      'No continuous availability, automatic stream reconnect, dirty-file, approval, pause/revive, or provider-webhook claim.'] };
  journal.result = result;
  await save('active-completed-reading.json', completed);
  await save('after.json', after);
  await save('journal.json', journal);
  await save('result.json', result);
  console.log(JSON.stringify({ status: 'passed', mode: result.mode, change, additionalTurns: 1,
    responseId: result.responseId, turnSequence: result.turnSequence }));
}

async function execute() {
  await preflight();
  if (action === 'capture') {
    await mkdir(join(root, 'continuations'), { mode: 0o700, recursive: true });
    await mkdir(directory, { mode: 0o700, recursive: true });
  }
  try { lock = await open(join(directory, 'active.lock'), 'wx', 0o600); }
  catch (error) { if (error.code === 'EEXIST') throw new Error('continuation_already_active'); throw error; }
  await lock.writeFile(JSON.stringify({ pid: process.pid, action, change, commit }) + '\n');
  await lock.sync();
  try { journal = JSON.parse(await readFile(join(directory, 'journal.json'), 'utf8')); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  if (journal) {
    requireValue(journal.schema === 'cma-retained-canary-continuation-v1' && journal.change === change
      && journal.run === config.run && digest(journal.resources) === digest(original.resources), 'retained_change_scope_changed');
  }
  if (action.endsWith('-active')) { await executeActive(); return; }
  requireValue(!journal?.active, 'active_and_idle_modes_cannot_mix');
  if (action === 'capture') {
    if (journal) {
      requireValue(journal.beforeCommit === commit, 'before_commit_changed');
      console.log(JSON.stringify({ status: 'already_captured', change, mutations: 0 }));
      return;
    }
    const before = await observed();
    for (const [index, value] of before.agents.entries()) {
      requireValue(value.agent.status === 'ready' && value.transcript.turns.every((turn) => turn.outcome.type === 'completed'), 'quiet_completed_agents_required');
      requireValue(assistantText(value.transcript.turns.at(-1)).includes(marker(index)), 'baseline_marker_missing');
    }
    journal = { schema: 'cma-retained-canary-continuation-v1', change, run: config.run,
      resources: original.resources, originalCommit: original.commit, beforeCommit: commit,
      before, commands: {}, capturedAt: new Date().toISOString() };
    await save('journal.json', journal);
    await save('before.json', before);
    console.log(JSON.stringify({ status: 'captured_before_change', change, mutations: 0,
      turns: before.agents.map((value) => value.transcript.turns.length) }));
    return;
  }
  requireValue(journal?.before, 'prechange_capture_required');
  requireValue(!journal.afterCommit || journal.afterCommit === commit, 'after_commit_changed');
  if (journal.result?.status === 'passed') {
    console.log(JSON.stringify({ status: 'already_passed', change, additionalTurns: 2 })); return;
  }
  journal.afterCommit = commit;
  await save('journal.json', journal);
  const afterChange = await observed();
  compare(journal.before, afterChange);
  for (const [index, value] of afterChange.agents.entries()) {
    const priorCount = journal.before.agents[index].transcript.turns.length;
    requireValue(value.transcript.turns.length <= priorCount + (journal.commands[`continue-${index}`] ? 1 : 0), 'unexpected_precontinuation_turn');
  }
  if (!journal.afterChange) { journal.afterChange = afterChange; await save('journal.json', journal); }
  const text = 'Repeat the marker from my first message, with no extra text. Do not use tools, access files, or modify anything.';
  const [triggerId, manualId] = original.resources.agentIds;
  await command('continue-0', `/api/v1/agents/${triggerId}/input`, { text }, 202);
  const v2 = await command('continue-1', '/api/v2/responses', { conversation: manualId, input: text, stream: true }, 200,
    { conversation: manualId, marker: marker(1) });
  await Promise.all([completed(0), completed(1)]);
  const after = await observed();
  const continuity = compare(journal.before, after);
  for (const [index, value] of after.agents.entries()) {
    requireValue(value.transcript.turns.length === journal.before.agents[index].transcript.turns.length + 1
      && value.transcript.turns.every((turn) => turn.outcome.type === 'completed')
      && assistantText(value.transcript.turns.at(-1)).includes(marker(index)), 'continued_turn_finality_changed');
  }
  const result = { status: 'passed', change, beforeCommit: journal.beforeCommit, afterCommit: commit,
    at: new Date().toISOString(), resources: original.resources, additionalTurns: 2,
    totalTurns: after.agents.map((value) => value.transcript.turns.length), retainedTranscriptPrefix: true,
    nativeIdentity: after.agents.map((value) => value.native), continuity,
    continuations: [{ interface: 'v1_agent_input', agentId: triggerId }, { interface: 'v2_responses_stream', ...v2 }],
    limitations: ['Checks sampled before/after state and successful continued interaction; continuous availability is not measured.',
      'No pause/revive, dirty-file, approval or provider-webhook claim. No cleanup is performed.'] };
  journal.result = result;
  await save('after.json', after);
  await save('journal.json', journal);
  await save('result.json', result);
  console.log(JSON.stringify({ status: 'passed', change, additionalTurns: 2, totalTurns: result.totalTurns }));
}

try { await execute(); }
catch (error) {
  const code = safeCode(error);
  if (lock) await save('failure.json', { status: 'failed', code, at: new Date().toISOString() });
  console.error(JSON.stringify({ status: 'failed', code }));
  process.exitCode = 1;
} finally {
  if (lock) { await lock.close(); await unlink(join(directory, 'active.lock')); }
}
