import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {check} from '../cma/core.mjs';

export async function configurationDenials({api, outsider, record, runCase, recordState}) {
  await runCase('writer.unauthorized_configuration_refused', async () => {
    const root = '/api/v1/agents/' + record.agentId;
    const before = {};
    for (const field of ['preferences', 'settings']) {
      before[field] = (await api.request('a', 'GET', root + '/' + field)).body;
      const response = await outsider.request('a', 'GET', root + '/' + field, {allowed: [403, 404]});
      check([403, 404].includes(response.status), 'foreign_configuration_read_allowed');
    }
    const changes = [
      ['preferences', {expected_revision: before.preferences.revision, title: 'Foreign ' + randomUUID()}],
      ['preferences', {expected_revision: before.preferences.revision, system_prompt: 'Foreign ' + randomUUID()}],
      ['settings', {model: before.settings.model, reasoning_effort: before.settings.reasoning_effort,
        permission_mode: before.settings.permission_mode === 'guarded' ? 'autonomous' : 'guarded'}],
    ];
    for (const [field, body] of changes) {
      const response = await outsider.request('a', 'PATCH', root + '/' + field,
        {body, key: randomUUID(), allowed: [403, 404]});
      check([403, 404].includes(response.status), 'foreign_configuration_write_allowed');
    }
    const after = {};
    for (const field of ['preferences', 'settings']) {
      after[field] = (await api.request('a', 'GET', root + '/' + field)).body;
      check(equal(after[field], before[field]), 'foreign_configuration_changed_owner_state');
    }
    await recordState('outsider_refused_owner_state_preserved', {before, after});
  });
}
