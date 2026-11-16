import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {check, id} from '../cma/core.mjs';

// The caller has already observed the active provider stream. A second message
// must stay queued; editing/cancelling it must not execute a second provider call.
export async function queuedCommandCase({api, adapter, record, turn, recordState}) {
  const agent = record.agentId;
  let stream;
  try {
    stream = await api.send('a', agent, 'Queued original ' + randomUUID(), randomUUID());
    const response = stream.responseId;
    const base = `/api/v2/agents/${agent}/responses/${response}`;
    const list = async () => (await api.request('a', 'GET', `/api/v2/agents/${agent}/queued-inputs`)).body;
    const before = (await list()).data?.find(row => row.response?.receipt?.response_id === response);
    check(before?.editable === true && before.response.status === 'queued'
      && before.response.turn_sequence === null && before.response.receipt.agent_id === agent
      && id(before.response.receipt.command_id) && Number.isSafeInteger(before.command_revision),
    'queued_input_not_observed_before_dispatch');
    await recordState('queued_before', before);
    const originalReceipt = structuredClone(before.response.receipt);
    const edit = {source_command_id: originalReceipt.command_id,
      expected_command_revision: before.command_revision, text: 'Queued changed ' + randomUUID()};
    const key = randomUUID();
    const edited = (await api.request('a', 'POST', base + '/queued-input', {body: edit, key})).body;
    check(edited.edit?.status === 'completed' && edited.response?.status === 'queued'
      && equal(edited.response.receipt, originalReceipt) && edited.text === edit.text
      && edited.command_revision > before.command_revision, 'queued_edit_not_durable');
    // Reuse an acknowledged intent only, with the exact original body and key.
    const replay = (await api.request('a', 'POST', base + '/queued-input', {body: edit, key})).body;
    check(equal(replay, edited), 'queued_edit_replay_changed_receipt');
    const stale = (await api.request('a', 'POST', base + '/queued-input',
      {body: edit, key: randomUUID(), allowed: [409]})).body;
    check(stale.error?.code === 'queued_input_not_admitted', 'queued_stale_edit_wrong_refusal');
    const changed = (await list()).data?.find(row => row.response?.receipt?.response_id === response);
    check(changed?.text === edit.text && changed.command_revision === edited.command_revision
      && equal(changed.response.receipt, originalReceipt), 'queued_edit_readback_changed');
    await recordState('queued_edited', changed);
    const observed = (await api.request('a', 'GET', base + '/queued-cancellation')).body;
    check(observed.cancellable === true && equal(observed.response?.receipt, originalReceipt)
      && observed.command_revision === changed.command_revision, 'queued_cancellation_identity_mismatch');
    const cancel = {source_command_id: originalReceipt.command_id,
      expected_command_revision: observed.command_revision}, cancelKey = randomUUID();
    const cancelled = (await api.request('a', 'POST', base + '/queued-cancellation',
      {body: cancel, key: cancelKey})).body;
    check(cancelled.cancellation?.status === 'completed' && cancelled.response?.status === 'cancelled'
      && equal(cancelled.response.receipt, originalReceipt), 'queued_cancellation_not_confirmed');
    for (const view of [cancelled.cancellation, cancelled.response]) {
      for (const field of ['turn_sequence', 'delivery', 'observed_cursor', 'failure']) {
        check(view[field] === null, 'queued_cancellation_dispatched_a_turn');
      }
    }
    const again = (await api.request('a', 'POST', base + '/queued-cancellation',
      {body: cancel, key: cancelKey})).body;
    check(equal(again, cancelled), 'queued_cancellation_replay_changed_receipt');
    const final = (await api.request('a', 'GET', '/api/v2/responses/' + response)).body;
    check(final.id === response && final.conversation?.id === agent && final.status === 'incomplete'
      && final.incomplete_details?.reason === 'interrupted' && equal(final.output, []),
    'queued_cancellation_terminal_projection_invalid');
    const oracle = await adapter.oracle({caseId: turn.caseId});
    check(oracle.case_id === turn.caseId && oracle.request_count === 1 && oracle.active_streams === 1
      && oracle.completed_effect_count === 0, 'queued_mutation_changed_active_provider_effect');
    await recordState('queued_cancelled_without_turn', {cancelled, response: final, oracle});
  } finally {
    if (stream) { stream.abort(); await stream.done; }
  }
}
