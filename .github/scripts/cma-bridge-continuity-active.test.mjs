import test from 'node:test';
import assert from 'node:assert/strict';
import { activeTurnSpec, checkActiveReading, checkOwnedTurnCounts } from './cma-bridge-continuity-active.mjs';
import { admittedResponse, completedResponse } from './cma-bridge-continuity-responses.mjs';
import { retainedCommand } from './cma-bridge-continuity-owned-config.mjs';

globalThis.fetch = () => { throw new Error('offline_validation_forbids_network'); };
const clone = (value) => structuredClone(value);
const spec = activeTurnSpec('active-final-ops', 'CMA-CANARY-owned-MANUAL');
const receipt = { commandId: 'cmd_owned', responseId: 'resp_owned', conversation: 'agt_owned' };
const expected = { agentId: 'agt_owned', receipt, beforeTurns: 2, spec };
const event = (seq, kind) => ({ seq, appended_at: `2026-09-16T00:00:0${seq}Z`, kind });
const fixture = () => ({
  agent: { id: 'agt_owned', status: 'working' },
  transcript: { agent_id: 'agt_owned', turns: [{ turn_sequence: 1 }, { turn_sequence: 2 }, {
    turn_sequence: 3, owner_message: spec.prompt, messages: [], outcome: { type: 'active' },
  }] },
  history: [event(1, { type: 'created' }), event(2, { type: 'conversation_opened' }),
    event(3, { type: 'turn_started', turn_sequence: 3 }),
    event(4, { type: 'tool_call_started', call_id: 'tool_owned', kind: 'execute', title: spec.shell, status: 'in_progress' })],
  response: { id: 'resp_owned', conversation: { id: 'agt_owned' }, previous_response_id: null, status: 'in_progress', output: [] },
  command: { receipt: { command_id: 'cmd_owned', response_id: 'resp_owned', agent_id: 'agt_owned' }, status: 'in_progress', turn_sequence: 3 },
});
const completed = () => {
  const value = fixture();
  value.history.push(event(5, { type: 'tool_call_updated', call_id: 'tool_owned', status: 'completed' }));
  value.history.push(event(6, { type: 'turn_completed', turn_sequence: 3 }));
  Object.assign(value.transcript.turns.at(-1), { outcome: { type: 'completed' }, messages: [{ role: 'assistant', content: spec.final }] });
  value.response.status = 'completed';
  value.response.output = [{ content: [{ text: spec.final }] }];
  value.command.status = 'completed';
  value.agent.status = 'ready';
  return value;
};

test('one exact running execute tool, response, command and turn establish an active checkpoint', () => {
  const proof = checkActiveReading(fixture(), expected);
  assert.equal(proof.phase, 'active');
  assert.equal(proof.toolCallId, 'tool_owned');
  assert.equal(proof.turnSequence, 3);
});
test('queued acceptance and a working turn without a tool are not active-tool evidence', () => {
  const queued = fixture(); queued.transcript.turns.pop(); queued.history.splice(2);
  queued.command.turn_sequence = null; queued.command.status = 'queued'; queued.response.status = 'queued';
  assert.equal(checkActiveReading(queued, expected).phase, 'queued');
  const working = fixture(); working.history.pop();
  assert.equal(checkActiveReading(working, expected).phase, 'waiting_tool');
});
test('pending tools and finished tools cannot arm a running-tool checkpoint', () => {
  const pending = fixture(); pending.history.at(-1).kind.status = 'pending';
  assert.equal(checkActiveReading(pending, expected).phase, 'waiting_tool');
  const finished = fixture(); finished.history.push(event(5, { type: 'tool_call_updated', call_id: 'tool_owned', status: 'completed' }));
  assert.equal(checkActiveReading(finished, expected).phase, 'waiting_tool');
});
test('title-only updates cannot turn a completed or failed tool back into a running one', () => {
  const value = fixture();
  value.history.push(event(5, { type: 'tool_call_updated', call_id: 'tool_owned', status: 'completed' }));
  value.history.push(event(6, { type: 'tool_call_updated', call_id: 'tool_owned', status: 'updated', title: spec.shell }));
  assert.equal(checkActiveReading(value, expected).phase, 'waiting_tool');
});
test('response, command, Agent, prompt and bound-turn substitution are refused', () => {
  for (const mutate of [
    v => { v.response.id = 'resp_other'; }, v => { v.response.conversation.id = 'agt_other'; },
    v => { v.command.receipt.command_id = 'cmd_other'; }, v => { v.command.receipt.agent_id = 'agt_other'; },
    v => { v.agent.id = 'agt_other'; }, v => { v.transcript.turns.at(-1).owner_message = 'different'; },
    v => { v.command.turn_sequence = 4; },
  ]) { const value = fixture(); mutate(value); assert.throws(() => checkActiveReading(value, expected)); }
});
test('a yielded launch can be observed through bounded identifiable polling of its existing session', () => {
  const value = fixture();
  value.history.push(event(5, { type: 'tool_call_updated', call_id: 'tool_owned', status: 'completed' }));
  value.history.push(event(6, { type: 'tool_call_started', call_id: 'poll_owned', kind: 'other',
    title: `${spec.marker} poll same session`, status: 'in_progress' }));
  const proof = checkActiveReading(value, expected);
  assert.equal(proof.phase, 'active');
  assert.equal(proof.toolCallId, 'tool_owned');
  assert.equal(proof.activeToolCallId, 'poll_owned');
  assert.equal(proof.observedToolCalls, 2);
  value.history.at(-1).kind.title = 'Run another command';
  assert.throws(() => checkActiveReading(value, expected), /active_only_original_session_polls_allowed/);
});
test('another command or an unidentifiable tool title cannot masquerade as the sleep operation', () => {
  for (const mutate of [
    v => { v.history.at(-1).kind.title = 'Run a command'; },
    v => { v.history.at(-1).kind.title = `sleep 1 ${spec.marker}`; },
    v => { v.history.at(-1).kind.kind = 'read'; },
    v => { v.history.push(event(5, { ...v.history.at(-1).kind, call_id: 'tool_second' })); },
  ]) { const value = fixture(); mutate(value); assert.throws(() => checkActiveReading(value, expected)); }
});
test('approval waits, interruption, failure, extra turns and broken event history fail the drill', () => {
  for (const mutate of [
    v => { v.command.status = 'waiting_action'; }, v => { v.agent.status = 'waiting_approval'; },
    v => { v.response.status = 'incomplete'; }, v => { v.history.at(-1).kind.status = 'failed'; },
    v => { v.history.push(event(5, { type: 'approval_requested' })); },
    v => { v.history.push(event(5, { type: 'turn_started', turn_sequence: 5 })); },
    v => { v.transcript.turns.push(clone(v.transcript.turns.at(-1))); },
    v => { v.history[1].seq = 8; },
  ]) { const value = fixture(); mutate(value); assert.throws(() => checkActiveReading(value, expected)); }
});
test('completion retains the original tool/turn and requires its durable final outcome and marker', () => {
  const retained = checkActiveReading(fixture(), expected);
  const value = completed();
  assert.equal(checkActiveReading(value, { ...expected, retained }).phase, 'completed');
  for (const mutate of [
    v => { v.history[3].kind.call_id = 'tool_other'; v.history[4].kind.call_id = 'tool_other'; },
    v => { v.history[4].kind.status = 'in_progress'; },
    v => { v.transcript.turns.at(-1).messages[0].content = 'model says done'; },
    v => { v.response.output = []; }, v => { v.history.pop(); },
  ]) { const changed = clone(value); mutate(changed); assert.throws(() => checkActiveReading(changed, { ...expected, retained })); }
});
test('eventual command/response bookkeeping must settle before the result passes', () => {
  const value = completed(); value.command.status = 'in_progress';
  assert.equal(checkActiveReading(value, expected).phase, 'settling');
});
test('a retained turn cannot disappear back into the queue', () => {
  const value = fixture(); value.transcript.turns.pop(); value.command.turn_sequence = null;
  assert.throws(() => checkActiveReading(value, { ...expected, retained: { turnSequence: 3, toolCallId: 'tool_owned' } }), /active_retained_turn_missing/);
});
test('only the manual sibling gains exactly one turn', () => {
  const before = { agents: [2, 2].map(n => ({ transcript: { turns: Array(n) } })) };
  const after = { agents: [2, 3].map(n => ({ transcript: { turns: Array(n) } })) };
  checkOwnedTurnCounts(before, after);
  after.agents[0].transcript.turns.push({});
  assert.throws(() => checkOwnedTurnCounts(before, after), /active_sibling_turn_changed/);
});
test('dynamic prompt identifiers cannot inject shell or extra commands', () => {
  for (const change of ['bad;id', 'bad\ncommand', 'bad$(id)', '']) assert.throws(() => activeTurnSpec(change, 'CMA-MARKER'));
  assert.throws(() => activeTurnSpec('good', "marker' ; id"));
  assert.equal(spec.sleepSeconds, 600);
  assert.equal(spec.shell, "sleep 600; printf '%s\\n' 'CMA-ACTIVE-active-final-ops'");
});
test('saved receipt is persisted before closing the streaming observer', async () => {
  const order = [];
  const response = { status: 200, headers: new Headers({ 'content-type': 'text/event-stream',
    'x-cma-response-id': receipt.responseId, 'x-cma-command-id': receipt.commandId }),
  body: { cancel: async () => { order.push('close'); } } };
  const result = await admittedResponse(response, { conversation: 'agt_owned' }, async value => {
    assert.equal(value.responseId, receipt.responseId); order.push('durably_save');
  });
  assert.deepEqual(order, ['durably_save', 'close']);
  assert.equal(result.status, 'admitted_observer_closed');
});
test('missing durable IDs do not become accepted work', async () => {
  await assert.rejects(admittedResponse({ status: 200, headers: new Headers({ 'content-type': 'text/event-stream' }), body: {} }, expected, () => {}), /v2_durable_receipt_required/);
});
test('pending and unsettled active commands never dispatch again through retained command recovery', () => {
  const prior = { path: '/api/v2/responses', key: 'owned-active-key', body: JSON.stringify({ input: spec.prompt }), state: 'pending' };
  for (const state of ['pending', 'unsettled']) assert.throws(() => retainedCommand({ ...prior, state }, prior.path, prior.body, prior.key), /unsettled_command_requires_explicit_recovery/);
});
test('ordinary completed stream parsing still validates the same header receipt after extraction', async () => {
  const frames = ['queued', 'in_progress', 'completed'].map((status, sequence_number) => {
    const type = status === 'queued' ? 'response.created' : `response.${status}`;
    const response = { id: receipt.responseId, conversation: { id: 'agt_owned' }, status,
      previous_response_id: null, output: status === 'completed' ? [{ role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: spec.final }] }] : [] };
    return `id: ${sequence_number}\nevent: ${type}\ndata: ${JSON.stringify({ type, sequence_number, response })}\n\n`;
  }).join('');
  const response = new Response(frames, { headers: { 'content-type': 'text/event-stream',
    'x-cma-response-id': receipt.responseId, 'x-cma-command-id': receipt.commandId } });
  const result = await completedResponse(response, { conversation: 'agt_owned', marker: spec.final }, async () => {}, 'OFFLINE_FORBIDDEN_TOKEN');
  assert.equal(result.status, 'completed');
  assert.equal(result.commandId, receipt.commandId);
});
