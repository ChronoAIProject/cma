import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {check} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const settingsBody = value => Object.fromEntries(
  ['model', 'reasoning_effort', 'permission_mode'].map(key => [key, value[key]]));
const changedSettings = value => ({...settingsBody(value),
  permission_mode: value.permission_mode === 'guarded' ? 'autonomous' : 'guarded'});
const preferencesPath = resource => `/api/v1/agents/${resource.agentId}/preferences`;
const settingsPath = resource => `/api/v1/agents/${resource.agentId}/settings`;
const read = async (api, path) => (await api.request('a', 'GET', path)).body;
const patch = (api, path, body, allowed = [200]) => api.request('a', 'PATCH', path,
  {body, key: randomUUID(), allowed});

// These use the real owner-routed public API. A 200 for an unchanged value is
// insufficient: both the acknowledgement and a fresh read must show the change.
export async function configurationCases({api, record: resource, runCase, recordFailure,
  recordState = async () => {}}) {
  const preferences = preferencesPath(resource), settings = settingsPath(resource);
  await runCase('user.system_prompt_roundtrip', async () => {
    const before = await read(api, preferences);
    await recordState('prompt_before', before);
    check(before.prompt_supported === true && before.prompt_pending === false
      && Number.isSafeInteger(before.revision), 'prompt_fixture_not_supported_and_idle');
    const desired = 'Two-AM instructions ' + randomUUID();
    check(before.system_prompt !== desired, 'prompt_change_required');
    await withRestoration(async () => {
      const updated = (await patch(api, preferences,
        {expected_revision: before.revision, system_prompt: desired})).body;
      check(updated.system_prompt === desired && updated.prompt_pending === false
        && updated.revision === before.revision + 1, 'prompt_update_not_confirmed');
      const seen = await read(api, preferences);
      check(seen.system_prompt === desired && seen.revision === updated.revision
        && seen.prompt_pending === false, 'prompt_update_not_visible');
      await recordState('prompt_changed', seen);
      await patch(api, preferences, {expected_revision: before.revision,
        system_prompt: before.system_prompt}, [409]);
      check((await read(api, preferences)).system_prompt === desired, 'stale_prompt_changed_state');
    }, async () => {
      const current = await read(api, preferences);
      if (current.system_prompt !== before.system_prompt) {
        check(current.system_prompt === desired, 'prompt_restore_foreign_value');
        await patch(api, preferences, {expected_revision: current.revision,
          system_prompt: before.system_prompt});
      }
      const restored = await read(api, preferences);
      check(restored.system_prompt === before.system_prompt && restored.prompt_pending === false,
        'prompt_restore_not_confirmed');
      await recordState('prompt_restored', restored);
    }, recordFailure);
  });
  await runCase('user.session_settings_roundtrip', async () => {
    const before = await read(api, settings), prompt = await read(api, preferences);
    await recordState('settings_before', {settings: before, preferences: prompt});
    check(before.pending === false && ['guarded', 'autonomous'].includes(before.permission_mode),
      'settings_fixture_not_supported_and_idle');
    const desired = changedSettings(before);
    await withRestoration(async () => {
      const updated = (await patch(api, settings, desired)).body;
      check(equal(settingsBody(updated), desired) && updated.pending === false,
        'settings_update_not_confirmed');
      const seen = await read(api, settings);
      check(equal(settingsBody(seen), desired) && seen.pending === false,
        'settings_update_not_visible');
      const afterPrompt = await read(api, preferences);
      check(afterPrompt.system_prompt === prompt.system_prompt
        && afterPrompt.revision === prompt.revision, 'settings_changed_owner_prompt');
      await recordState('settings_changed', {settings: seen, preferences: afterPrompt});
    }, async () => {
      const current = await read(api, settings);
      if (!equal(settingsBody(current), settingsBody(before))) {
        check(equal(settingsBody(current), desired), 'settings_restore_foreign_value');
        await patch(api, settings, settingsBody(before));
      }
      const restored = await read(api, settings);
      check(equal(settingsBody(restored), settingsBody(before)) && restored.pending === false,
        'settings_restore_not_confirmed');
      await recordState('settings_restored', restored);
    }, recordFailure);
  });
}

// Called only after the held provider's real active-stream oracle is observed.
// This exercises working-state admission, not a deterministic injected CAS race.
export async function heldConfigurationCases({api, record: resource, runCase,
  recordState = async () => {}}) {
  const preferences = preferencesPath(resource), settings = settingsPath(resource);
  await runCase('writer.busy_configuration_refused', async () => {
    const before = await read(api, preferences), oldSettings = await read(api, settings);
    await recordState('busy_before', {preferences: before, settings: oldSettings});
    for (const [path, body] of [
      [preferences, {expected_revision: before.revision, system_prompt: 'Busy mutation ' + randomUUID()}],
      [settings, changedSettings(oldSettings)],
    ]) {
      const refused = await patch(api, path, body, [409]);
      check(refused.body?.code === 'agent_not_ready', 'busy_configuration_wrong_refusal');
    }
    const after = await read(api, preferences), afterSettings = await read(api, settings);
    check(equal(after, before), 'busy_prompt_mutated');
    check(equal(afterSettings, oldSettings), 'busy_settings_mutated');
    await recordState('busy_refused_unchanged', {preferences: after, settings: afterSettings});
  });
  await runCase('writer.held_rename', async () => {
    const before = await read(api, preferences), title = 'Owner move ' + randomUUID();
    check(before.title !== title, 'rename_change_required');
    const updated = (await patch(api, preferences, {expected_revision: before.revision, title})).body;
    check(updated.title === title && updated.revision === before.revision + 1, 'held_rename_ack');
    const seen = await read(api, preferences);
    check(seen.title === title && seen.revision === updated.revision, 'held_rename_readback');
    // This exact acknowledged value must survive the existing actual owner loss.
    resource.writerTitle = {title, revision: updated.revision};
    await recordState('held_rename_changed', seen);
  });
}

export async function assertWriterTitle(api, resource) {
  const expected = resource.writerTitle;
  check(expected && Number.isSafeInteger(expected.revision), 'held_title_evidence_missing');
  const current = await read(api, preferencesPath(resource));
  check(current.title === expected.title && current.revision === expected.revision,
    'acknowledged_title_lost_after_owner_move');
  return current;
}

export async function verifyLifecycleReplay(api, resource) {
  const options = {body: resource.createBody, key: resource.key, allowed: [202]};
  const replay = await api.request('a', 'POST', '/api/v1/agents', options);
  check(replay.body.id === resource.operationId, 'admission_replay_identity_changed');
  const refused = await api.request('a', 'POST', '/api/v1/agents', {
    ...options, body: {...resource.createBody, name: 'Altered ' + randomUUID()}, allowed: [409]});
  check(refused.body?.code === 'agent_busy', 'altered_admission_wrong_refusal');
  const original = await api.request('a', 'POST', '/api/v1/agents', options);
  check(original.body.id === resource.operationId, 'altered_admission_replaced_original');
}
