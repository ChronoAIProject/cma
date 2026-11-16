import {define, acquire, openBrowser, visible, invoke, goto, peer, same, digest, uiReceipt} from './helpers.mjs';
import {Blocked} from '../../runner.mjs';
import {configureChannelForm, armChannelCreation, assertCreatedChannel, assertRecoveredChannel} from './channel-creation.mjs';
import {channelReceipt, openListedChannel} from './channel-browser.mjs';

const pick = async (page, label, name) => {
  await page.getByRole('button', {name: label, exact: true}).click();
  await page.getByRole('menuitemradio', {name, exact: true}).click();
};

export function validateProfileFixture(lease) {
  const extra = lease.settings;
  if (!lease.existingAgentId || !lease.expected?.ownerSubject || !lease.draft?.name
    || !Array.isArray(lease.expected.allowedServices) || lease.expected.allowedServices.length < 1
    || !Array.isArray(extra?.skills) || extra.skills.length < 1 || extra.skills.length > 32
    || !extra.skills.every(s => s.source === 'ornn' && s.id && s.version)
    || !Array.isArray(extra.mcpServers) || extra.mcpServers.length < 1 || extra.mcpServers.length > 8
    || !extra.mcpServers.every(s => {
      try {
        const url = new URL(s.url);
        return url.protocol === 'https:' && !url.username && !url.password
          && ['127.0.0.1', '[::1]'].includes(url.hostname)
          && (s.auth?.type === 'none' || s.auth?.type === 'nyxid_service' && s.auth.slug);
      } catch {return false;}
    })
    || !extra.environment || Array.isArray(extra.environment) || typeof extra.environment !== 'object'
    || Object.keys(extra.environment).length < 1 || Object.keys(extra.environment).length > 8
    || !Object.entries(extra.environment).every(([key, value]) => /^[A-Z][A-Z0-9_]{0,63}$/.test(key)
      && (typeof value === 'string' && value.length <= 128 || typeof value === 'boolean'
        || typeof value === 'number' && Number.isFinite(value)))
    || !Number.isSafeInteger(extra.queueLifetimeSeconds) || extra.queueLifetimeSeconds < 1 || extra.queueLifetimeSeconds > 86400
    || ![lease.draft.model?.service, ...(lease.draft.permission?.service_scope ?? []),
      ...extra.mcpServers.filter(s => s.auth.type === 'nyxid_service').map(s => s.auth.slug)]
      .every(service => lease.expected.allowedServices.includes(service)))
    throw new Blocked('consumer_profile_supported_local_settings_required');
  return extra;
}

export function profileWithSettings(profile, settings) {
  return {...profile, queue_lifetime_seconds: settings.queueLifetimeSeconds,
    context: {...profile.context, skills: settings.skills, mcp_servers: settings.mcpServers},
    environment: settings.environment};
}

export function profileTargetFixture(lease, kind) {
  // Existing-Agent targets require automatic replies in routing_validation.rs.
  // Configure the real form accordingly before choosing that target.
  return {...lease, draft: {...lease.draft, name: lease.draft.name + ' ' + kind,
    autoFinal: kind === 'existing_agent' ? true : lease.draft.autoFinal}};
}

export async function fillProfileSettings(page, settings) {
  const fill = (label, value) => page.getByLabel(label, {exact: true}).fill(String(value));
  await fill('Maximum wait (seconds)', settings.queueLifetimeSeconds);
  for (let i = 0; i < settings.skills.length; i++) {
    await page.getByRole('button', {name: 'Add skill', exact: true}).click();
    await fill(`Skill ${i + 1} ID`, settings.skills[i].id);
    await fill(`Skill ${i + 1} version`, settings.skills[i].version);
  }
  for (let i = 0; i < settings.mcpServers.length; i++) {
    const server = settings.mcpServers[i];
    await page.getByRole('button', {name: 'Add MCP server', exact: true}).click();
    await fill(`Server ${i + 1} URL`, server.url);
    await pick(page, `Server ${i + 1} authentication`, server.auth.type);
    if (server.auth.type === 'nyxid_service') await fill(`Server ${i + 1} service`, server.auth.slug);
  }
  await page.getByRole('button', {name: 'Add environment settings', exact: true}).click();
  const group = page.getByRole('group', {name: 'Environment values', exact: true});
  for (const [key, value] of Object.entries(settings.environment)) {
    await group.getByLabel('Environment values field name', {exact: true}).fill(key);
    await pick(page, 'Environment values value type', typeof value);
    await group.getByRole('button', {name: 'Add field', exact: true}).click();
    if (typeof value === 'boolean') await group.getByRole('checkbox', {name: key, exact: true}).setChecked(value);
    else await group.getByLabel(key, {exact: true}).fill(String(value));
  }
}

export function assertProviderBinding(ctx, snapshot, record, desired, credentialHashes) {
  ctx.assert(snapshot?.binding_id === record.provider.binding_id && snapshot.bind_requests === 1
    && snapshot.owner === desired.profile.metadata.owner.subject && snapshot.agent_owner === snapshot.owner
    && snapshot.agent_key_hash === credentialHashes.agent,
  'consumer_provider_actual_binding_identity');
  ctx.assert(same(snapshot.profile, desired.profile) && same(snapshot.target, desired.provider.target)
    && snapshot.profile_digest === record.definition.profile_digest,
  'consumer_provider_actual_profile_target');
  ctx.assert(typeof snapshot.agent_id === 'string' && snapshot.agent_id.length > 0
    && (desired.provider.target.type !== 'existing_agent' || snapshot.agent_id === desired.provider.target.agent_id),
  'consumer_provider_resolved_agent');
  ctx.assert(Array.isArray(snapshot.refused) && snapshot.refused.length === 0
    && same(snapshot.applied_profile, desired.profile), 'consumer_provider_applied_configuration');
  ctx.assert(Array.isArray(snapshot.authorized_services)
    && [desired.profile.model.service, ...desired.profile.permission.service_scope,
      ...desired.profile.context.mcp_servers.filter(s => s.auth.type === 'nyxid_service').map(s => s.auth.slug)]
      .every(service => snapshot.authorized_services.includes(service)), 'consumer_provider_authorized_service_scope');
  ctx.assert(!Object.hasOwn(snapshot, 'agent_key') && !Object.hasOwn(snapshot, 'credentials'),
    'consumer_provider_snapshot_redacted');
}

async function inspectProfile(ctx, state) {
  const {session, desired} = state, page = session.page, profile = desired.profile;
  await page.getByRole('button', {name: `Consumer: Default consumer (${desired.provider.slug})`, exact: true}).click();
  const dialog = page.getByRole('dialog', {name: 'Default consumer', exact: true});
  await visible(ctx, dialog, 'consumer_profile_inspector_open');
  const value = async (label, expected) => ctx.assert(await dialog.getByLabel(label, {exact: true}).inputValue() === String(expected),
    'consumer_profile_saved_input');
  for (const [label, expected] of [['Agent configuration name', profile.metadata.name], ['Description', profile.metadata.description],
    ['Owner subject', profile.metadata.owner.subject], ['Profile provider', profile.metadata.provider],
    ['Model', profile.model.model], ['Model service', profile.model.service], ['System prompt', profile.context.system_prompt],
    ['Maximum wait (seconds)', profile.queue_lifetime_seconds]]) await value(label, expected);
  for (const [label, expected] of [['Thinking effort', profile.model.thinking_effort], ['Model access', profile.model.access],
    ['Model API', profile.model.api_type], ['Permission mode', profile.permission.action_mode === 'guarded'
      ? 'Ask before sensitive actions' : 'Autonomous']])
    ctx.assert(await dialog.getByRole('button', {name: label, exact: true}).innerText() === expected,
      'consumer_profile_saved_choice');
  for (let i = 0; i < profile.permission.service_scope.length; i++) await value('Service scope ' + (i + 1), profile.permission.service_scope[i]);
  for (let i = 0; i < profile.context.skills.length; i++) {
    await value(`Skill ${i + 1} ID`, profile.context.skills[i].id);
    await value(`Skill ${i + 1} version`, profile.context.skills[i].version);
  }
  for (let i = 0; i < profile.context.mcp_servers.length; i++) {
    const server = profile.context.mcp_servers[i]; await value(`Server ${i + 1} URL`, server.url);
    ctx.assert(await dialog.getByRole('button', {name: `Server ${i + 1} authentication`, exact: true}).innerText() === server.auth.type,
      'consumer_profile_saved_mcp_auth');
    if (server.auth.type === 'nyxid_service') await value(`Server ${i + 1} service`, server.auth.slug);
  }
  const environment = dialog.getByRole('group', {name: 'Environment values', exact: true});
  for (const [key, expected] of Object.entries(profile.environment)) {
    if (typeof expected === 'boolean') ctx.assert(await environment.getByRole('checkbox', {name: key, exact: true}).isChecked() === expected,
      'consumer_profile_saved_environment');
    else ctx.assert(await environment.getByLabel(key, {exact: true}).inputValue() === String(expected), 'consumer_profile_saved_environment');
  }
  await visible(ctx, dialog.getByText(desired.provider.target.type === 'profile'
    ? 'Agent created from this channel’s settings' : 'Existing agent: ' + desired.provider.target.agent_id, {exact: true}),
  'consumer_profile_immutable_target');
  const key = dialog.getByLabel('Provider agent key', {exact: true});
  ctx.assert(await key.getAttribute('type') === 'password' && await key.inputValue() === '', 'consumer_profile_saved_key_not_rendered');
  ctx.assert(await dialog.getByLabel('Owner subject', {exact: true}).isDisabled(), 'consumer_profile_owner_immutable');
}

export const consumerProfileCase = {...define(25, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let lease, settings; const states = [];
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'consumer_profile'); settings = validateProfileFixture(lease);
    for (const kind of ['profile', 'existing_agent']) {
      const variant = profileTargetFixture(lease, kind);
      const session = await openBrowser(ctx, 'user', {channelCreation: true});
      for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
        if (typeof session[method] !== 'function') throw new Blocked('consumer_profile_actual_eg_browser_required');
      const configured = await configureChannelForm(ctx, session, variant);
      if (kind === 'existing_agent') {
        await pick(session.page, 'Agent target', 'Use an existing agent');
        await session.page.getByLabel('Agent ID', {exact: true}).fill(lease.existingAgentId);
        configured.desired.provider.target = {type: kind, agent_id: lease.existingAgentId};
      }
      ctx.assert(await session.page.getByLabel('Provider', {exact: true}).inputValue() === lease.draft.provider,
        'consumer_profile_provider_selected');
      ctx.assert(await session.page.getByRole('button', {name: 'Agent target', exact: true}).innerText()
        === (kind === 'profile' ? 'Create an agent from these settings' : 'Use an existing agent'),
      'consumer_profile_target_selected');
      states.push({session, ...configured});
    }
  });
  await ctx.step('step_2', async () => {
    for (const state of states) {
      const {session, desired, credentialHashes} = state, page = session.page;
      await fillProfileSettings(page, settings); desired.profile = profileWithSettings(desired.profile, settings);
      state.submission = await armChannelCreation(ctx, page, lease, desired, credentialHashes, {loseResponse: false});
      await page.getByRole('button', {name: 'Create channel', exact: true}).click();
      state.accepted = await state.submission.result();
      await visible(ctx, page.getByRole('heading', {name: desired.name, exact: true}), 'consumer_profile_created_channel');
      await invoke(session, 'networkIdle');
      state.receipt = channelReceipt(ctx, await invoke(session, 'egEvidence'), state.accepted.requestId, 'POST', 201);
      await ctx.observe('consumer_profile_create_receipt', async () => ({schema: 'multi-eg-request-v1', receipt: state.receipt}));
      await session.switchEG(peer(state.receipt.replica));
      const selected = await openListedChannel(ctx, session, state.accepted.record, () => goto(page, '/event-gateway'));
      ctx.assert(selected.receipt.replica !== state.receipt.replica
        && selected.receipt.serving.instance_id !== state.receipt.serving.instance_id, 'consumer_profile_other_actual_eg');
      assertCreatedChannel(ctx, selected.body, state.accepted.intent, credentialHashes);
      assertRecoveredChannel(ctx, selected.body, state.accepted.record);
      await inspectProfile(ctx, state);
    }
  });
  await ctx.step('expected_result', async () => {
    for (const state of states) {
      const {record, intent} = state.accepted;
      const snapshot = await invoke(lease, 'bindingSnapshot', {bindingId: record.provider.binding_id});
      assertProviderBinding(ctx, snapshot, record, state.desired, state.credentialHashes);
      const records = await invoke(lease, 'channelCreationRecords', {recordId: intent.recordId, key: intent.key});
      ctx.assert(state.submission.attempts() === 1 && records.createAttempts === 1
        && same(records.recordIds, [intent.recordId]) && same(records.channelIds, [record.channel_id])
        && same(records.bindingIds, [record.provider.binding_id])
        && records.retained_body_sha256 === intent.body_sha256 && records.retained_key_sha256 === digest(intent.key),
      'consumer_profile_single_allocation');
      ctx.assert(records.providerTurns === 0 && records.deliveries === 0 && records.pending === 0 && records.unknown === 0,
        'consumer_profile_no_unresolved_work');
      const scan = await invoke(lease, 'scanBrowserForCredentialValues', {page: state.session.page});
      ctx.assert(scan.matches === 0 && scan.scannedInputs === true && scan.scannedText === true, 'consumer_profile_no_credential_disclosure');
      await ctx.observe('consumer_profile_binding', async () => ({binding_sha256: digest(snapshot.binding_id),
        owner_sha256: digest(snapshot.owner), agent_sha256: digest(snapshot.agent_id), profile_sha256: digest(snapshot.profile),
        target: state.desired.provider.target.type, bind_requests: snapshot.bind_requests}));
      await uiReceipt(ctx, state.session.page);
    }
    ctx.assert(new Set(states.map(state => state.accepted.record.record_id)).size === 2
      && new Set(states.map(state => state.accepted.record.provider.binding_id)).size === 2, 'consumer_profile_distinct_target_bindings');
  });
}), evidence_scope: 'eg_only'};
