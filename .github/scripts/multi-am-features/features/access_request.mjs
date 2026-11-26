import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual} from 'node:util';
import {assertOwner, assertResponse, check, hash, other, poll, terminal} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

export async function accessRequest({api, adapter, user, outsider, record, recordFailure}) {
  const {agentId} = record;
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(owner.am);
  const root = `/api/v1/agents/${agentId}`;
  const endpoint = root + '/access-requests';
  const accessRequest = randomUUID();
  const reason = 'Synthetic consent check ' + accessRequest;
  const read = async am => {
    const {body} = await user.request(am, 'GET', endpoint);
    check(Array.isArray(body?.items), 'agent_access_request_inventory_invalid');
    const rows = body.items.filter(row => row.reason === reason);
    check(rows.length <= 1 && rows.every(row => row.agent_id === agentId &&
      /^aar_[a-f0-9]{64}$/.test(row.id) && isDeepStrictEqual(row.services, ['fixture-access-service'])),
    'agent_access_request_identity_or_uniqueness_unproven');
    return rows[0];
  };
  const readScope = async am => {
    const {body} = await user.request(am, 'GET', `/api/v1/sandboxes/${record.sandboxId}`);
    check(body?.id === record.sandboxId && Array.isArray(body.nyxid_services), 'agent_access_service_scope_unbound');
    return body.nyxid_services;
  };
  const baseline = await readScope(owner.am);
  check(await read(owner.am) === undefined && await read(ingress) === undefined, 'agent_access_request_not_fresh');
  const propose = async am => {
    const caseId = 'access-' + randomUUID(), marker = 'WS4_ACCESS_' + randomUUID();
    await adapter.prepareTurn({caseId, marker, accessRequest});
    let stream, receipt;
    await withRestoration(async () => {
      await adapter.releaseTurn({caseId});
      stream = await api.send(am, agentId, `Use the service consent tool for the synthetic check ${marker}.`, caseId);
      const response = await poll(async () => {
        const {body} = await api.request(other(am), 'GET', `/api/v2/responses/${stream.responseId}`);
        return assertResponse(body, agentId, stream.responseId);
      }, terminal, {timeout: 120000, code: 'agent_access_request_turn_timeout'});
      check(response.status === 'completed' && JSON.stringify(response.output ?? []).includes(marker),
        'agent_access_request_turn_incomplete');
      const model = await poll(() => adapter.oracle({caseId}), observed => observed.active_streams === 0,
        {timeout: 20000, code: 'agent_access_model_still_active'});
      check(model.case_id === caseId && model.access_tool_calls === 1 && model.request_count === 2 &&
        model.completed_effect_count === 2, 'agent_access_tool_call_missing_or_duplicated');
      receipt = model.access_receipt;
    }, async () => { if (stream) { stream.abort(); await stream.done; } }, recordFailure);
    return receipt;
  };
  const toolPending = await propose(ingress);
  const pending = await read(owner.am);
  check(pending?.status === 'pending' && isDeepStrictEqual(pending, await read(ingress)),
    'agent_access_pending_receipt_unbound');
  check(toolPending?.request_sha256 === hash(pending.id) && toolPending.status === 'pending',
    'agent_access_pending_tool_receipt_unbound');
  const deny = `${endpoint}/${pending.id}/deny`;
  for (const am of [ingress, owner.am]) {
    const {body} = await outsider.request(am, 'POST', deny, {body: {}, key: randomUUID(), allowed: [404]});
    check(body?.code === 'agent_access_not_found', 'agent_access_foreign_denial_untyped');
  }
  check(isDeepStrictEqual(await read(ingress), pending), 'agent_access_foreign_denial_mutated_request');
  const intent = {body: {}, key: randomUUID()};
  const {body: denied} = await user.request(ingress, 'POST', deny, intent);
  check(denied?.id === pending.id && denied.status === 'denied' && denied.agent_id === agentId,
    'agent_access_denied_receipt_unbound');
  const {body: replay} = await user.request(owner.am, 'POST', deny, intent);
  check(isDeepStrictEqual(replay, denied), 'agent_access_denial_replay_changed_receipt');
  const toolReplay = await propose(owner.am);
  check(toolReplay?.request_sha256 === hash(pending.id) && toolReplay.status === 'denied',
    'agent_access_denied_tool_receipt_unbound');
  for (const am of [ingress, owner.am]) {
    check(isDeepStrictEqual(await read(am), denied), 'agent_access_reproposal_reopened_denied_request');
    check(isDeepStrictEqual(await readScope(am), baseline), 'agent_access_denial_changed_service_scope');
  }
}
