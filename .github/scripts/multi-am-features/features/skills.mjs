import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {assertOwner, check, other, poll} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';
import {probeProvider} from './provider_probe.mjs';
import {retryOwnerPublication} from './writer_publication.mjs';

export async function installSkillCase({api, adapter, record, phase, recordFailure}, publicationRetry = false) {
  check(['before_restart', 'after_restart'].includes(phase), 'skill_restart_phase_required');
  const id = 'two-am-' + (publicationRetry ? 'retry-' : '') + phase.replace('_', '-');
  const version = '1.0';
  const path = `/api/v1/agents/${record.agentId}/skills`;
  const intent = {key: randomUUID(), body: {id, version}, allowed: [202]};
  const read = async () => (await api.request('a', 'GET', path)).body;
  const observeSkill = present => probeProvider({api, adapter, record},
    {skill: {id, present}}, 'skill_matches', 'native_skill_catalog_mismatch');
  const before = await read();
  check(before.can_install === true && !before.items.some(item => item.id === id), 'skill_fixture_not_idle_or_fresh');
  await observeSkill(false);
  const fixture = {api, adapter, record, path, intent, recordFailure};
  const admission = publicationRetry ? await installWithPublicationAbort(fixture) : await installWithPeerRename(fixture);
  check(admission.items.filter(item => item.id === id && item.version === version).length === 1,
    'skill_install_admission_missing');
  const installed = await poll(read, view => {
    const selected = view.items.filter(item => item.id === id);
    check(selected.length <= 1 && !selected.some(item => item.status === 'failed'), 'skill_install_failed_or_duplicate');
    return selected.length === 1 && selected[0].status === 'installed' && view.pending === false;
  }, {timeout: 90000, code: 'skill_install_not_settled'});
  check(installed.items.find(item => item.id === id).version === version, 'installed_skill_version_changed');
  const replay = (await api.request('a', 'POST', path, intent)).body;
  check(equal(admission, replay), 'skill_install_replay_changed_receipt');
  const witness = await adapter.skillEffects({agentId: record.agentId, skillId: id});
  const [pending, installedChange] = witness.changes ?? [];
  check(witness.agentFound === true && equal(witness.entries, [{status: 'installed', version}])
    && witness.changes?.length === 2 && pending?.status === 'pending' && installedChange?.status === 'installed'
    && Number.isSafeInteger(pending.seq) && Number.isSafeInteger(installedChange.seq)
    && pending.seq > 0 && installedChange.seq > pending.seq, 'one_durable_skill_install_required');
  await observeSkill(true);
}

async function installWithPublicationAbort({api, adapter, record, path, intent, recordFailure}) {
  const {agentId} = record;
  const owner = await adapter.ownerOf({agentId});
  return retryOwnerPublication({api, adapter, agentId, owner, operation: 'skill_install', recordFailure,
    request: am => api.rawRequest(am, 'POST', path, {...intent, timeout: 10000})});
}

async function installWithPeerRename({api, adapter, record, path, intent, recordFailure}) {
  const {agentId} = record;
  const owner = assertOwner(await adapter.ownerOf({agentId}), agentId);
  const preferences = `/api/v1/agents/${agentId}/preferences`;
  const title = 'Skill publication ' + randomUUID();
  const read = am => api.rawRequest(am, 'GET', preferences).then(value => value.body);
  const rename = (am, body) => api.rawRequest(am, 'PATCH', preferences, {key: randomUUID(), body});
  const before = await read(owner.am);
  check(typeof before.title === 'string' && Number.isSafeInteger(before.revision), 'skill_publication_baseline_unbound');
  let admitted;
  await withRestoration(async () => {
    let renamed;
    const response = await api.router.dispatch(agentId, async ingress => {
      check(ingress === other(owner.am), 'skill_publication_owner_changed');
      const outcomes = await Promise.allSettled([api.rawRequest(ingress, 'POST', path, intent),
        rename(owner.am, {expected_revision: before.revision, title})]);
      for (const outcome of outcomes) if (outcome.status === 'rejected') throw outcome.reason;
      renamed = outcomes[1].value.body;
      return outcomes[0].value;
    });
    admitted = response.body;
    const durable = await adapter.titleEffects({agentId, title});
    check(renamed.title === title && renamed.revision === before.revision + 1
      && durable.length === 1 && Number.isSafeInteger(durable[0]), 'skill_publication_rename_effect_unconfirmed');
    for (const am of ['a', 'b']) {
      const current = await read(am);
      check(current.title === title && current.revision === renamed.revision, 'skill_publication_peer_readback_changed');
    }
  }, async () => {
    const current = await read(owner.am);
    if (current.title !== before.title) {
      check(current.title === title, 'skill_publication_restore_foreign_title');
      const restored = (await rename(owner.am, {expected_revision: current.revision, title: before.title})).body;
      check(restored.title === before.title, 'skill_publication_restore_unconfirmed');
    }
  }, recordFailure);
  return admitted;
}
