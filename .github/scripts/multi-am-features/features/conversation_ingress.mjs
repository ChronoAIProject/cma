import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {assertOracle, assertOwner, assertResponse, check, hash, id, other, poll, terminal} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';
import {requestPacer} from '../pacing.mjs';

export async function verifyRecoveredReceipt({api, adapter, agentId, owner, turn, completed}) {
  const ingress = other(owner.am);
  const {commandId, responseId} = turn.stream;
  check(id(commandId) && id(responseId), 'recovered_original_receipt_unbound');
  const commandPath = '/api/v2/commands/' + commandId;
  const responsePath = '/api/v2/responses/' + responseId;
  const read = async (am, path) => (await api.rawRequest(am, 'GET', path)).body;
  const command = await read(owner.am, commandPath);
  check(command?.status === 'completed' && command.failure === null && command.receipt?.agent_id === agentId
    && command.receipt.command_id === commandId && command.receipt.response_id === responseId,
  'recovered_command_receipt_unbound');
  const replay = await api.send(ingress, agentId, turn.input, turn.caseId);
  try {
    await replay.done;
    check(replay.ingress_am === ingress && replay.commandId === commandId && replay.responseId === responseId
      && replay.error === null && replay.events.filter(event => event.type === 'response.completed').length === 1,
    'recovered_admission_replay_changed');
    const refused = await api.request(ingress, 'POST', `/api/v2/agents/${agentId}/responses`, {
      body: {input: turn.input + ' Changed.', stream: true}, key: turn.caseId, allowed: [409], direct: true,
    });
    check(refused.body?.code === 'idempotency_key_reused', 'recovered_admission_changed_intent_untyped');
    for (const am of [ingress, owner.am]) {
      check(equal(await read(am, commandPath), command), 'recovered_command_changed_after_replay');
      check(equal(await read(am, responsePath), completed), 'recovered_response_changed_after_replay');
    }
    const current = assertOwner(await adapter.ownerOf({agentId}), agentId);
    check(current.am === owner.am && current.epoch === owner.epoch && current.pod_uid === owner.pod_uid,
      'recovered_admission_owner_changed');
    assertOracle(await adapter.oracle({caseId: turn.caseId}), turn.caseId, 1);
    const request = replay.timing?.request_id;
    check(typeof request === 'string' && request.trim().length > 0 && request !== turn.stream.timing?.request_id,
      'recovered_replay_request_identity_unbound');
    return {ingress, command_sha256: hash(commandId), request_sha256: hash(request)};
  } finally {
    replay.abort();
    await replay.done;
  }
}

export async function sharedIngressReceipt({api, adapter, record, recordState, recordFailure}) {
  const {agentId} = record;
  const caseId = 'shared-ingress-' + randomUUID();
  const marker = 'WS4_' + randomUUID().replaceAll('-', '');
  const input = 'Return the exact marker ' + marker + ' without tools or file changes.';
  const read = async (am, path) => (await api.rawRequest(am, 'GET', path)).body;
  const streams = [];
  await adapter.prepareTurn({caseId, marker});
  await withRestoration(async () => {
    const agui = await api.send('a', agentId, input, caseId, 'agui');
    streams.push(agui);
    const ingress = agui.ingress_am, peer = other(ingress);
    await poll(() => adapter.oracle({caseId}), value => {
      check(value?.case_id === caseId && value.request_count <= 1
        && value.completed_effect_count === 0 && value.released === false, 'agui_turn_barrier_changed');
      return value.request_count === 1 && value.active_streams === 1;
    }, {timeout: 45000, code: 'agui_runtime_request_missing'});
    await adapter.releaseTurn({caseId});
    const responsePath = '/api/v2/responses/' + agui.responseId;
    const completed = await poll(async () => assertResponse(await read(peer, responsePath), agentId, agui.responseId),
      terminal, {timeout: 90000, code: 'agui_response_completion_timeout'});
    check(completed.status === 'completed' && JSON.stringify(completed.output ?? []).includes('WS4 complete ' + marker),
      'agui_response_output_missing');
    await agui.done;
    check(agui.error === null && agui.runOutcome === 'complete'
      && agui.events.filter(event => event.type === 'RUN_FINISHED').length === 1, 'agui_stream_completion_invalid');
    const commandPath = '/api/v2/commands/' + agui.commandId;
    const command = await read(peer, commandPath);
    check(command?.status === 'completed' && command.failure === null && command.receipt?.agent_id === agentId
      && command.receipt.command_id === agui.commandId && command.receipt.response_id === agui.responseId
      && command.turn_sequence === Number(agui.runId), 'shared_ingress_command_receipt_unbound');

    await requestPacer.wait(api.config.bearer, 'POST');
    const replay = await api.rawSend(peer, agentId, input, caseId, 'root-responses');
    streams.push(replay);
    check(replay.ingress_am === peer && replay.commandId === agui.commandId && replay.responseId === agui.responseId,
      'root_responses_replay_changed_receipt');
    await replay.done;
    check(replay.error === null && replay.events.filter(event => event.type === 'response.completed').length === 1,
      'root_responses_replay_missing_completion');
    const changed = await api.request(ingress, 'POST', '/api/v2/responses', {
      body: {conversation: agentId, input: input + ' Changed.', stream: true},
      key: caseId, allowed: [409],
    });
    check(changed.body?.code === 'idempotency_key_reused', 'root_responses_changed_intent_wrong_refusal');
    for (const am of [ingress, peer]) {
      check(equal(await read(am, commandPath), command), 'shared_ingress_peer_command_changed');
      check(equal(await read(am, responsePath), completed), 'shared_ingress_peer_response_changed');
    }
    const transcript = await read(peer, '/api/v1/agents/' + agentId + '/transcript');
    const matching = transcript?.turns?.filter(turn => turn.owner_message === input);
    check(transcript.agent_id === agentId && matching?.length === 1
      && matching[0].turn_sequence === command.turn_sequence, 'shared_ingress_duplicate_or_missing_turn');
    const settled = await poll(() => adapter.oracle({caseId}), value => value.active_streams === 0,
      {timeout: 20000, code: 'shared_ingress_provider_still_active'});
    assertOracle(settled, caseId, 1);
    await recordState('shared_command_receipt', {ingress, peer, command_sha256: hash(agui.commandId),
      response_sha256: hash(agui.responseId), effects: settled.completed_effect_count});
  }, async () => {
    try { await adapter.releaseTurn({caseId}); }
    finally {
      for (const stream of streams) stream.abort();
      await Promise.all(streams.map(stream => stream.done));
    }
  }, recordFailure);
}
