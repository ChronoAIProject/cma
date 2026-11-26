import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {check, poll} from '../cma/core.mjs';

export async function installSkillCase({api, adapter, record, phase}) {
  const id = phase === 'before_restart' ? 'two-am-before-restart' : 'two-am-after-restart';
  check(['before_restart', 'after_restart'].includes(phase), 'skill_restart_phase_required');
  const version = '1.0';
  const path = `/api/v1/agents/${record.agentId}/skills`;
  const intent = {key: randomUUID(), body: {id, version}, allowed: [202]};
  const read = async () => (await api.request('a', 'GET', path)).body;
  const before = await read();
  check(before.can_install === true && !before.items.some(item => item.id === id), 'skill_fixture_not_idle_or_fresh');
  const admission = (await api.request('a', 'POST', path, intent)).body;
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
  check(equal(await adapter.skillEffects({agentId: record.agentId, skillId: id}),
    [{status: 'installed', version}]), 'one_durable_skill_install_required');
}
