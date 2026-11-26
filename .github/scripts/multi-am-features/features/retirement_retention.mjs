import {randomUUID} from 'node:crypto';
import {check, hash, id} from '../cma/core.mjs';

export async function retirementRetention({adapter, user, outsider, record, cleanup}) {
  const {agentId, sandboxId} = record;
  check(id(agentId) && id(sandboxId), 'retirement_lineage_required');
  const agentPath = `/api/v1/agents/${agentId}`;
  const sandboxPath = `/api/v1/sandboxes/${sandboxId}`;
  const snapshot = () => adapter.lifecycleSnapshot({sandboxId, agentIds: [agentId]});
  const before = await snapshot();
  check(before?.status === 'active' && before.children?.length === 1 &&
    before.children[0]?.status === 'ready' && Number.isSafeInteger(before.children[0].last_seq) &&
    before.children[0].last_seq > 0, 'retirement_live_history_unbound');
  for (const am of ['a', 'b']) {
    await outsider.request(am, 'DELETE', sandboxPath, {key: randomUUID(), allowed: [404]});
    await outsider.request(am, 'DELETE', agentPath, {key: randomUUID(), allowed: [404]});
  }
  const protectedState = await snapshot();
  check(protectedState?.status === 'active' && protectedState.children?.[0]?.status === 'ready' &&
    protectedState.children[0].last_seq >= before.children[0].last_seq, 'retirement_foreign_delete_changed_state');
  await cleanup(record);
  const retained = await snapshot();
  check(retained?.status === 'retired' && retained.runtime_detached === true && retained.purge_deadline_future === true &&
    retained.children?.length === 1 && retained.children[0]?.status === 'deleted' &&
    retained.children[0].last_seq > before.children[0].last_seq, 'retirement_retained_state_or_deadline_missing');
  const events = [];
  for (const am of ['a', 'b']) {
    const [agent, sandbox, history] = await Promise.all([
      user.request(am, 'GET', agentPath), user.request(am, 'GET', sandboxPath),
      user.request(am, 'GET', `${sandboxPath}/events?limit=200`),
    ]);
    check(agent.body?.id === agentId && agent.body.status === 'deleted' && sandbox.body?.id === sandboxId &&
      sandbox.body.status === 'retired', 'retirement_exact_public_readback_missing');
    check(Array.isArray(history.body) && history.body.length < 200 &&
      history.body.every(entry => entry.sandbox_id === sandboxId && Number.isSafeInteger(entry.seq)),
    'retirement_history_unbound');
    const retired = history.body.filter(entry => entry.kind?.type === 'status_changed' && entry.kind?.to === 'retired');
    check(retired.length === 1, 'retirement_history_missing_or_duplicated');
    events.push(retired[0].seq);
  }
  check(events[0] === events[1], 'retirement_peer_history_mismatch');
  return {agent_sha256: hash(agentId), sandbox_sha256: hash(sandboxId), terminal_event_count: 1,
    retained_history: true, observed_future_purge_deadline: true};
}
