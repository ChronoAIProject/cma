import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual} from 'node:util';
import {assertOwner, assertResponse, check, hash, other, poll, terminal} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const SERVICE = 'fixture-access-service';

export async function accessRequest({api, adapter, user, outsider, record, recordFailure}, approve = false) {
  const {agentId} = record;
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(owner.am);
  const root = `/api/v1/agents/${agentId}`;
  const endpoint = root + '/access-requests';
  const accessRequest = randomUUID();
  const reason = 'Synthetic consent check ' + accessRequest;
  const list = async am => {
    const {body} = await user.request(am, 'GET', endpoint);
    check(Array.isArray(body?.items), 'agent_access_request_inventory_invalid');
    return body.items;
  };
  const read = async am => {
    const rows = (await list(am)).filter(row => row.reason === reason);
    check(rows.length <= 1 && rows.every(row => row.agent_id === agentId &&
      /^aar_[a-f0-9]{64}$/.test(row.id) && isDeepStrictEqual(row.services, [SERVICE])),
    'agent_access_request_identity_or_uniqueness_unproven');
    return rows[0];
  };
  const readScope = async am => {
    const {body} = await user.request(am, 'GET', `/api/v1/sandboxes/${record.sandboxId}`);
    check(body?.id === record.sandboxId && Array.isArray(body.nyxid_services), 'agent_access_service_scope_unbound');
    return [...body.nyxid_services].sort();
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
  if (approve) {
    return approveAccess({adapter, user, outsider, record, recordFailure, list, read, readScope,
      baseline, pending, endpoint, owner, ingress});
  }
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

async function approveAccess({adapter, user, outsider, record, recordFailure, list, read, readScope,
  baseline, pending, endpoint, owner, ingress}) {
  const {agentId, sandboxId} = record;
  check(!baseline.includes(SERVICE) && baseline.length > 0, 'agent_access_approval_scope_not_fresh');
  const approvedScope = [...baseline, SERVICE].sort();
  const path = endpoint + '/' + pending.id + '/approve';
  const originalRuntime = await adapter.runtimeFor({agentId});
  check(originalRuntime?.sandbox_id === sandboxId && typeof originalRuntime.pod_uid === 'string'
    && originalRuntime.pod_uid.length > 0 && originalRuntime.ready === true, 'agent_access_approval_runtime_unbound');
  const sameRuntime = current => current?.sandbox_id === sandboxId && current.pod_uid === originalRuntime.pod_uid
    && current.home_identity === originalRuntime.home_identity && current.image === originalRuntime.image;
  const allow = {body: {services: approvedScope}, key: randomUUID()};
  const foreign = await outsider.request(ingress, 'POST', path, {...allow, allowed: [403, 404]});
  check(['agent_access_owner_authority_required', 'agent_access_forbidden', 'agent_access_not_found']
    .includes(foreign.body?.code), 'agent_access_foreign_approval_untyped');
  check(isDeepStrictEqual(await read(owner.am), pending)
    && isDeepStrictEqual(await readScope(ingress), baseline), 'agent_access_foreign_approval_mutated_scope');
  const waitForDecision = async (sample, code) => poll(sample, value => {
    check(value?.agent_id === agentId && ['pending', 'connecting', 'applying', 'succeeded', 'failed'].includes(value.status),
      'agent_access_approval_receipt_unbound');
    return ['succeeded', 'failed'].includes(value.status);
  }, {timeout: 120000, code});
  const detail = '/api/v1/agents/' + agentId;
  const waitReady = () => poll(async () => (await user.request(owner.am, 'GET', detail)).body, value => {
    check(value?.id === agentId && value.sandbox?.id === sandboxId, 'agent_access_runtime_lineage_changed');
    return value.status === 'ready';
  }, {timeout: 90000, code: 'agent_access_notification_did_not_quiesce'});

  await withRestoration(async () => {
    const {body: admitted} = await user.request(ingress, 'POST', path, allow);
    check(admitted?.id === pending.id && ['connecting', 'applying', 'succeeded'].includes(admitted.status),
      'agent_access_approval_admission_unbound');
    const settled = await waitForDecision(() => read(owner.am), 'agent_access_approval_timeout');
    check(settled.id === pending.id && settled.status === 'succeeded', 'agent_access_approval_did_not_apply');
    const {body: replay} = await user.request(owner.am, 'POST', path, allow);
    check(isDeepStrictEqual(replay, settled), 'agent_access_approval_replay_changed_receipt');
    const changed = await user.request(ingress, 'POST', path,
      {...allow, body: {services: baseline}, allowed: [409]});
    check(changed.body?.code === 'agent_access_conflict', 'agent_access_approval_changed_intent_untyped');
    await waitReady();
    const notices = [];
    for (const am of [ingress, owner.am]) {
      check(isDeepStrictEqual(await read(am), settled) && isDeepStrictEqual(await readScope(am), approvedScope),
        'agent_access_approval_peer_scope_or_receipt_drifted');
      const {body: transcript} = await user.request(am, 'GET', detail + '/transcript');
      const matching = transcript?.turns?.filter(turn => turn.owner_message?.startsWith(
        'The owner approved service access request ' + pending.id + '.'));
      check(transcript.agent_id === agentId && matching?.length === 1 && Number.isSafeInteger(matching[0].turn_sequence),
        'agent_access_approval_notification_missing_or_duplicated');
      notices.push(matching[0].turn_sequence);
    }
    check(notices[0] === notices[1] && sameRuntime(await adapter.runtimeFor({agentId})),
      'agent_access_approval_changed_runtime_or_notification');
  }, async () => {
    const decision = await read(owner.am);
    if (['connecting', 'applying'].includes(decision?.status)) {
      await waitForDecision(() => read(owner.am), 'agent_access_approval_uncertain');
    }
    const current = await readScope(owner.am);
    if (!isDeepStrictEqual(current, baseline)) {
      check(isDeepStrictEqual(current, approvedScope), 'agent_access_restore_foreign_scope');
      await waitReady();
      const {body: restore} = await user.request(owner.am, 'PUT', detail + '/service-access', {
        body: {services: baseline, expected_services: approvedScope}, key: randomUUID(), allowed: [202],
      });
      check(/^aar_[a-f0-9]{64}$/.test(restore?.id) && restore.id !== pending.id,
        'agent_access_scope_restoration_receipt_unbound');
      const settled = await waitForDecision(async () => (await list(ingress)).find(row => row.id === restore.id),
        'agent_access_scope_restoration_timeout');
      check(settled.status === 'succeeded', 'agent_access_scope_restoration_failed');
      await waitReady();
    }
    for (const am of [ingress, owner.am]) {
      check(isDeepStrictEqual(await readScope(am), baseline), 'agent_access_scope_restoration_unconfirmed');
    }
    check(sameRuntime(await adapter.runtimeFor({agentId})), 'agent_access_scope_restoration_changed_runtime');
  }, recordFailure);
}
