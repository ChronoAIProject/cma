import {randomUUID} from 'node:crypto';
import {assertOwner, check, other} from '../cma/core.mjs';

export async function competingRenames({api, adapter, record, recordState = async () => {}}) {
  const {agentId} = record;
  const path = `/api/v1/agents/${agentId}/preferences`;
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const ingress = other(owner.am);
  const before = (await api.rawRequest(owner.am, 'GET', path)).body;
  check(Number.isSafeInteger(before.revision), 'rename_race_baseline_revision_missing');
  const changes = ['Owner', 'Peer'].map(label => ({key: randomUUID(), allowed: [200, 409],
    timeout: 10000, prefer: 'cma-settings-reasons',
    body: {expected_revision: before.revision, title: `${label} ${randomUUID()}`}}));
  let direct;
  const forwarded = await api.router.dispatch(agentId, async am => {
    check(am === ingress, 'rename_race_owner_changed_before_send');
    const results = await Promise.allSettled([
      api.rawRequest(owner.am, 'PATCH', path, changes[0]),
      api.rawRequest(am, 'PATCH', path, changes[1]),
    ]);
    for (const result of results) if (result.status === 'rejected') throw result.reason;
    direct = results[0].value;
    return results[1].value;
  });
  const responses = [direct, forwarded];
  const index = responses.findIndex(result => result.status === 200);
  check(index >= 0 && responses[1 - index].status === 409, 'rename_race_requires_exactly_one_winner');
  const winner = responses[index].body, loser = responses[1 - index].body;
  check(winner?.title === changes[index].body.title && winner.revision === before.revision + 1
    && loser?.settings_update_reason === 'stale_revision', 'rename_race_receipt_or_stale_reason_wrong');
  for (const am of [owner.am, ingress]) {
    const current = (await api.rawRequest(am, 'GET', path)).body;
    check(current.title === winner.title && current.revision === winner.revision, 'rename_race_peer_readback_changed');
  }
  const [published, refused] = await Promise.all([adapter.titleEffects({agentId, title: winner.title}),
    adapter.titleEffects({agentId, title: changes[1 - index].body.title})]);
  check(published?.length === 1 && Number.isSafeInteger(published[0]) && published[0] > 0
    && refused?.length === 0, 'rename_race_durable_effect_count_wrong');
  await recordState('competing_renames', {owner: owner.am, ingress, winning_ingress: index === 0 ? owner.am : ingress,
    expected_revision: before.revision, final_revision: winner.revision, publications: published.length});
}
