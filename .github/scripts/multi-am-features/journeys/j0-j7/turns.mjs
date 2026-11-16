import {check, hash, opposite, request, requireOwnerProof, unique, waitFor} from './helpers.mjs';
import {need} from './provenance.mjs';

export function exactTurn(response, message, outcome) {
  check(Array.isArray(response?.body?.turns) && response.body.next == null, 'turn_transcript_incomplete');
  const turns = response.body.turns.filter(turn => turn.owner_message === message);
  check(turns.length <= 1, 'turn_duplicated');
  return turns.length === 1 && turns[0].outcome?.type === outcome ? turns[0] : null;
}

export async function j3(ctx, state) {
  state.requestIds = [];
  const read = () => request(ctx, state.identity, {am: opposite(state.am), method: 'GET',
    path: `/api/v1/agents/${state.agentId}/transcript`, agentId: state.agentId, allowed: [200]});
  const firstMessage = 'Reply only OK.';
  check(!(await read()).body.turns.some(turn => turn.owner_message === firstMessage), 'reply_turn_already_exists');
  await request(ctx, state.identity, {method: 'POST', path: `/api/v1/agents/${state.agentId}/input`,
    agentId: state.agentId, body: {text: firstMessage}, key: unique('send'), allowed: [202], proofState: state.requestIds});
  const completed = await waitFor(ctx, read, value => exactTurn(value, firstMessage, 'completed'), 'send_reply_timeout');
  const reply = exactTurn(completed, firstMessage, 'completed');
  ctx.assert(Array.isArray(reply.agent_messages) && reply.agent_messages.length === 1
    && reply.agent_messages[0].trim() === 'OK', 'send_exact_ok_reply');
  await requireOwnerProof(ctx, state, 'j3_send_owner');

  const controls = ctx.fixture('runtime-controls');
  need(['prepareTurn', 'releaseTurn', 'oracle'].every(name => typeof controls?.[name] === 'function'),
    'interrupt_runtime_control_unavailable');
  const caseId = unique('interrupt-turn'), message = 'Keep this turn active until interrupted: ' + caseId;
  let released = false;
  const release = async () => { if (!released) { await controls.releaseTurn({caseId}); released = true; } };
  ctx.defer('interrupt_runtime_release', release);
  await controls.prepareTurn({caseId, kind: 'configuration', marker: caseId});
  await request(ctx, state.identity, {method: 'POST', path: `/api/v1/agents/${state.agentId}/input`,
    agentId: state.agentId, body: {text: message}, key: unique('interrupt-send'), allowed: [202], proofState: state.requestIds});
  const held = await waitFor(ctx, () => controls.oracle({caseId}), value => value.case_id === caseId
    && value.request_count === 1 && value.active_streams === 1 && value.completed_effect_count === 0,
  'interrupt_provider_not_active');
  await waitFor(ctx, read, value => exactTurn(value, message, 'active'), 'interrupt_turn_not_active');
  await ctx.observe('interrupt_active_provider', async () => held);
  await request(ctx, state.identity, {method: 'POST', path: `/api/v1/agents/${state.agentId}/interrupt`,
    agentId: state.agentId, key: unique('interrupt'), allowed: [202], proofState: state.requestIds});
  const interrupted = await waitFor(ctx, read, value => exactTurn(value, message, 'interrupted'),
    'interrupt_exact_turn_timeout');
  await requireOwnerProof(ctx, state, 'j3_interrupt_owner');
  // Releasing a held stream before observing interruption could turn this into a
  // completed-turn test. It is only released after durable interruption/cleanup.
  await release();
  ctx.assert(exactTurn(interrupted, message, 'interrupted') !== null, 'interrupt_exact_turn_confirmed');
  await ctx.observe('send_interrupt_result', async () => ({agent_sha256: hash(state.agentId),
    completed_turn: reply.turn_sequence, interrupted_turn: exactTurn(interrupted, message, 'interrupted').turn_sequence,
    reply_sha256: hash('OK'), request_ids: state.requestIds.map(row => row.request_id)}));
}
