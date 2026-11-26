import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {assertOwner, check, hash, id, other, poll} from '../cma/core.mjs';
import {retainFailure, withRestoration} from '../failure.mjs';
import {observeAlwaysOnOffInBrowser, relinquishAlwaysOnInBrowser} from './sandbox_always_on_browser.mjs';

export async function triggerAlwaysOn(context) {
  const {admin, config} = context;
  const path = '/api/v1/admin/settings';
  const read = async am => (await admin.request(am, 'GET', path)).body;
  const before = await read('a');
  const original = before.settings.triggers;
  check(typeof original.enabled === 'boolean' && typeof original.orchestration_paused === 'boolean',
    'trigger_platform_policy_missing');
  const intended = {enabled: true, orchestration_paused: false};
  const matches = (value, expected) => value.enabled === expected.enabled &&
    value.orchestration_paused === expected.orchestration_paused;
  const set = (am, expected_revision, triggers) => admin.request(am, 'PATCH', path,
    {body: {expected_revision, triggers}});
  let result;
  await withRestoration(async () => {
    if (!matches(original, intended)) await set('a', before.revision, intended);
    check(matches((await read('b')).settings.triggers, intended), 'trigger_platform_policy_unavailable');
    result = await exerciseTrigger(context);
  }, async () => {
    const current = await read('b');
    if (!matches(current.settings.triggers, original)) {
      check(matches(current.settings.triggers, intended), 'trigger_platform_policy_changed_by_another_writer');
      await set('b', current.revision, original);
    }
    check(matches((await read('a')).settings.triggers, original), 'trigger_platform_policy_restore_failed');
  }, (stage, error) => retainFailure(config.output_dir, stage, error, {case: 'trigger_platform_policy'}));
  return result;
}

async function exerciseTrigger({adapter, user, admin, outsider, config, browser}) {
  const triggerKey = randomUUID();
  const content = {
    presentation: {title: 'Two Manager Trigger ' + triggerKey, description: 'Synthetic Trigger journey',
      button_label: 'open_in_cma', theme: 'auto', signup_hint: 'Use a fixture account'},
    target: {inline: {agent: {reasoning_effort: 'medium', permission_mode: 'guarded'}}},
    reuse: {sandbox: 'always_new', workspace: 'always_new', agent: 'always_new'},
    consent: {always_review: true},
  };
  const {body: trigger} = await admin.request('a', 'POST', '/api/v1/triggers', {
    body: {content}, key: triggerKey, allowed: [201],
  });
  check(id(trigger.id) && Number.isSafeInteger(trigger.draft_version), 'trigger_draft_identity_missing');
  const triggerPath = '/api/v1/triggers/' + encodeURIComponent(trigger.id);
  let launchPath, observed, resources, relinquishment;
  const recordFailure = (stage, error) => retainFailure(config.output_dir, stage, error,
    {case: 'trigger_origin_always_on_and_replay'});

  await withRestoration(async () => {
    const {body: published} = await admin.request('b', 'POST', triggerPath + '/publish', {
      body: {draft_version: trigger.draft_version}, key: randomUUID(), allowed: [201],
    });
    check(id(published.public_id), 'trigger_publish_identity_missing');
    const admission = '/api/v1/triggers/public/' + encodeURIComponent(published.public_id) + '/launches';
    const request = {start_if_ready: false, referrer_origin: config.public_origin};
    const launchKey = randomUUID();
    const {body: launch} = await user.request('a', 'POST', admission, {body: request, key: launchKey, allowed: [201]});
    check(id(launch.id) && launch.trigger_id === trigger.id && !launch.started_at &&
      launch.resolution?.agent?.state === 'pending',
      'trigger_created_work_before_consent');
    launchPath = '/api/v1/launches/' + encodeURIComponent(launch.id);
    const trust = launch.trust?.confirmation_digest;
    const startBody = trust ? {trust_confirmed_digest: trust} : {};
    const startKey = randomUUID();
    await outsider.request('b', 'POST', launchPath + '/start', {body: startBody, key: randomUUID(), allowed: [404]});
    await user.request('b', 'POST', launchPath + '/start', {body: startBody, key: startKey, allowed: [202]});
    observed = await poll(async () => {
      const {body} = await user.request('a', 'GET', launchPath);
      check(body.id === launch.id && body.revision_digest === launch.revision_digest, 'trigger_launch_identity_changed');
      check(!['failed', 'cancelled', 'waiting_for_connection', 'needs_service_connection'].includes(body.state),
        'trigger_launch_did_not_progress');
      return body;
    }, launch => launch.state === 'ready', {timeout: 420000, code: 'trigger_launch_ready_timeout'});
    const {sandbox, agent} = observed.resolution;
    check(sandbox.state === 'created' && agent.state === 'created' && id(sandbox.id) && id(agent.id),
      'fresh_trigger_origin_resources_required');
    resources = {sandbox: sandbox.id, agent: agent.id};
    const active = await poll(() => adapter.ownerOf({agentId: agent.id}), value => value !== null,
      {timeout: 30000, code: 'trigger_initial_owner_missing'});
    const owner = assertOwner(active, agent.id);
    const peer = other(owner.am);
    const path = '/api/v1/admin/sandboxes/' + encodeURIComponent(sandbox.id);
    const agentPath = '/api/v1/agents/' + encodeURIComponent(agent.id);
    const readSandbox = async am => (await admin.request(am, 'GET', path)).body.sandbox;
    const verify = async desired => {
      for (const am of [owner.am, peer]) {
        const {body: view} = await user.request(am, 'GET', agentPath);
        check(view.id === agent.id && view.sandbox?.id === sandbox.id && ['ready', 'working'].includes(view.status),
          'trigger_agent_read_failed_after_always_on');
        const {body: provenance} = await user.request(am, 'GET', agentPath + '/provenance');
        check(provenance?.agent_id === agent.id && provenance.creation_origin === 'trigger'
          && provenance.details?.trigger?.trigger_id === trigger.id
          && provenance.details.trigger.launch_id === launch.id, 'trigger_agent_provenance_changed');
        const current = await readSandbox(am);
        check(current.id === sandbox.id && current.profile.always_on === desired, 'trigger_sandbox_always_on_not_visible');
      }
      const {body: denied} = await user.request(peer, 'POST', admission, {
        body: {...request, start_if_ready: true}, key: launchKey, allowed: [422],
      });
      check(denied.code === 'idempotency_key_reused', 'trigger_changed_payload_refusal_mismatch');
      const {body: replay} = await user.request(owner.am, 'POST', admission, {body: request, key: launchKey, allowed: [201]});
      check(replay.id === launch.id && replay.revision_digest === launch.revision_digest,
        'trigger_always_on_changed_original_admission');
      const {body: startReplay} = await user.request(peer, 'POST', launchPath + '/start', {
        body: startBody, key: startKey, allowed: [202],
      });
      check(startReplay.id === launch.id && startReplay.revision_digest === launch.revision_digest,
        'trigger_always_on_changed_start_receipt');
    };
    const original = await readSandbox(owner.am);
    check(Number.isSafeInteger(original.revision) && original.profile.always_on !== true,
      'known_trigger_sandbox_baseline_required');
    const set = (am, always_on, expected_revision, allowed = [200]) => admin.request(am, 'POST', path + '/always-on', {
      body: {always_on, expected_revision}, allowed,
    });
    await outsider.request(peer, 'POST', path + '/always-on', {
      body: {always_on: true, expected_revision: original.revision}, allowed: [403],
    });
    await withRestoration(async () => {
      await set(peer, true, original.revision);
      await verify(true);
      await set(owner.am, false, original.revision, [409]);
      const currentOwner = assertOwner(await adapter.ownerOf({agentId: agent.id}), agent.id);
      const ingress = other(currentOwner.am);
      const enabled = await readSandbox(ingress);
      check(enabled.profile.always_on === true && enabled.revision === original.revision + 1,
        'trigger_always_on_approval_revision_unbound');
      const ownerPath = '/api/v1/sandboxes/' + sandbox.id + '/always-on';
      const write = (client, am, value, expected_revision, allowed) => client.request(am, 'POST', ownerPath, {
        body: {always_on: value, expected_revision}, key: randomUUID(), allowed,
      });
      const selfApproval = await write(user, ingress, true, enabled.revision, [403]);
      check(selfApproval.body?.code === 'always_on_approval_required', 'trigger_owner_self_approval_refusal_untyped');
      const stale = await write(user, ingress, false, original.revision, [409]);
      check(stale.body?.code === 'sandbox_invalid_state', 'trigger_stale_relinquishment_refusal_untyped');
      await write(outsider, currentOwner.am, false, enabled.revision, [404]);
      const stillEnabled = await readSandbox(currentOwner.am);
      check(stillEnabled.profile.always_on === true && stillEnabled.revision === enabled.revision,
        'trigger_refused_relinquishment_mutated_approval');
      const {intent, receipt: released, read, mutation} = await relinquishAlwaysOnInBrowser(browser, ingress,
        sandbox.id, enabled.revision);
      check(released?.id === sandbox.id && released.always_on === false && released.revision === enabled.revision + 1,
        'trigger_owner_relinquishment_unconfirmed');
      const {body: replayed, timing} = await user.request(currentOwner.am, 'POST', ownerPath, intent);
      check(equal(replayed, released), 'trigger_owner_relinquishment_replay_changed');
      check(typeof timing?.request_id === 'string' && timing.request_id.trim().length > 0,
        'trigger_owner_relinquishment_replay_unbound');
      for (const am of [ingress, currentOwner.am]) {
        const current = await readSandbox(am);
        check(current.profile.always_on === false && current.revision === released.revision,
          'trigger_owner_relinquishment_peer_state_changed');
      }
      const peerRead = await observeAlwaysOnOffInBrowser(browser, currentOwner.am, sandbox.id, released.revision);
      const requests = {approved_read: read, relinquishment: mutation, replay: hash(timing.request_id), peer_read: peerRead};
      check(new Set(Object.values(requests)).size === 4, 'trigger_owner_relinquishment_request_id_reused');
      const settledOwner = assertOwner(await adapter.ownerOf({agentId: agent.id}), agent.id);
      check(settledOwner.am === currentOwner.am && settledOwner.epoch === currentOwner.epoch
        && settledOwner.pod_uid === currentOwner.pod_uid, 'trigger_owner_changed_during_browser_relinquishment');
      relinquishment = {ingress, owner: currentOwner.am, epoch: currentOwner.epoch, request_sha256: requests,
        receipt_sha256: hash(JSON.stringify(released)), revisions_advanced: released.revision - enabled.revision};
    }, async () => {
      const current = await readSandbox(owner.am);
      if (current.profile.always_on === true) await set(peer, false, current.revision);
      await verify(false);
    }, recordFailure);
  }, () => withRestoration(async () => {
      if (launchPath && !resources) {
        const {body: settled} = await user.request('b', 'GET', launchPath);
        const {agent, sandbox} = settled.resolution;
        if (agent.state === 'created' && sandbox.state === 'created' && id(agent.id) && id(sandbox.id)) {
          resources = {agent: agent.id, sandbox: sandbox.id};
        }
        if (!['ready', 'failed', 'cancelled'].includes(settled.state)) {
          await user.request('a', 'POST', launchPath + '/cancel', {body: {}, key: randomUUID(), allowed: [200, 409]});
        }
      }
      if (resources) await retireLaunchedResources(user, resources, config.owner_id);
    }, async () => {
      const {body: retired} = await admin.request('b', 'POST', triggerPath + '/retire', {key: randomUUID()});
      check(retired.id === trigger.id && typeof retired.retired_at === 'string', 'trigger_retirement_unobserved');
    },
    recordFailure), recordFailure);
  return {launch_sha256: hash(observed.id), sandbox_sha256: hash(resources.sandbox),
    owner_relinquishment: relinquishment};
}

async function retireLaunchedResources(client, resources, owner) {
  const agent = '/api/v1/agents/' + encodeURIComponent(resources.agent);
  const sandbox = '/api/v1/sandboxes/' + encodeURIComponent(resources.sandbox);
  const {body: inspected} = await client.request('a', 'GET', sandbox);
  check(inspected.id === resources.sandbox && inspected.owner === owner, 'trigger_cleanup_public_owner_mismatch');
  const {body: detail} = await client.request('b', 'GET', agent);
  check(detail.id === resources.agent && detail.sandbox?.id === resources.sandbox, 'trigger_cleanup_lineage_mismatch');
  if (['ready', 'working'].includes(detail.status)) {
    await client.request('b', 'POST', agent + '/stop', {body: {}, key: randomUUID(), allowed: [200, 202, 204]});
    await poll(() => client.request('a', 'GET', agent), value => value.body.status === 'stopped',
      {timeout: 180000, code: 'trigger_agent_stop_timeout'});
  }
  await client.request('a', 'DELETE', agent, {key: randomUUID(), allowed: [200, 202, 204]});
  await poll(() => client.request('b', 'GET', agent, {allowed: [200, 404]}),
    value => value.status === 404 || value.body.status === 'deleted', {timeout: 180000, code: 'trigger_agent_retirement_timeout'});
  let cursor;
  const seen = new Set();
  do {
    const query = new URLSearchParams({limit: '200', ...(cursor ? {cursor} : {})});
    const {body: page} = await client.request('a', 'GET', '/api/v1/agents?' + query);
    check(Array.isArray(page?.items) && !page.items.some(row =>
      row.sandbox_id === resources.sandbox && row.status !== 'deleted'), 'trigger_cleanup_live_agent');
    cursor = page.next;
    if (cursor) {
      check(typeof cursor === 'string' && !seen.has(cursor) && seen.size < 20, 'trigger_cleanup_inventory_incomplete');
      seen.add(cursor);
    }
  } while (cursor);
  await client.request('b', 'DELETE', sandbox, {key: randomUUID(), allowed: [200, 202, 204]});
  await poll(() => client.request('a', 'GET', sandbox, {allowed: [200, 404]}),
    value => value.status === 404 || value.body.status === 'retired', {timeout: 180000, code: 'trigger_sandbox_retirement_timeout'});
}
