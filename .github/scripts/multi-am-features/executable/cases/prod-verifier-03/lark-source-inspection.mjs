import {Blocked} from '../../runner.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {egRequest, peer, segment, same, digest, invoke, openBrowser, goto, visible, uiReceipt} from './helpers.mjs';
import {openListedChannel, openChannelNode, closeChannelInspector, channelRead} from './channel-browser.mjs';
import {guardDiscoveryWrites} from './channel-capabilities.mjs';
import {assertRelayCounters} from './relay-verification.mjs';

const id = value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value);
const text = value => typeof value === 'string' && value.trim().length > 0;
const requireValue = (value, code) => {if (!value) throw new Blocked(code);};
const title = region => region === 'feishu' ? 'Feishu' : 'Lark';
const bounded = ctx => ({signal: ctx.signal, timeoutMs: Math.max(1, Math.min(ctx.remainingMs?.() ?? 15000, 15000))});

// Four owned channels cover the two regional protocols and keep the separate
// native app apart from the relay bot and its explicitly selected connection.
// Discovery metadata is not evidence of credential custody or resource access.
export function validateLarkInspectionFixture(lease) {
  requireValue(lease?.local === true && lease.synthetic === true && text(lease.ownerSubject)
    && /^[a-f0-9]{40}$/.test(lease.source ?? '') && /@sha256:[a-f0-9]{64}$/.test(lease.image ?? '')
    && Array.isArray(lease.regions) && lease.regions.length === 2
    && same(lease.regions.map(r => r.region).sort(), ['feishu', 'lark']), 'lark_distinct_regional_seed_required');
  for (const method of ['request', 'counters', 'close'])
    requireValue(typeof lease[method] === 'function', 'lark_inspection_primitive_' + method);
  const channels = [], records = [], apps = [], bots = [], connections = [];
  for (const region of lease.regions) {
    const {native, relay} = region;
    for (const branch of [native, relay]) {
      requireValue(branch && ['recordId', 'channelId', 'senderId', 'chatId'].every(k => id(branch[k]))
        && branch.senderId.startsWith('ou_') && branch.chatId.startsWith('oc_'), 'lark_owned_channel_seed_required');
      records.push(branch.recordId); channels.push(branch.channelId);
    }
    requireValue(native.identity?.region === region.region && id(native.identity.app_id)
      && native.identity.app_id.startsWith('cli_') && id(native.identity.tenant_key)
      && id(native.identity.bot_open_id) && native.identity.bot_open_id.startsWith('ou_'), 'lark_native_identity_seed_required');
    requireValue(text(relay.issuer) && id(relay.keyId) && id(relay.routeId)
      && id(relay.botId) && relay.botId.startsWith('ou_') && relay.binding
      && ['user_service_id', 'catalog_service_id', 'app_id'].every(k => id(relay.binding[k]))
      && relay.binding.app_id.startsWith('cli_'), 'lark_relay_binding_seed_required');
    apps.push(native.identity.app_id, relay.binding.app_id); bots.push(native.identity.bot_open_id, relay.botId);
    requireValue(Array.isArray(region.connections) && region.connections.length === 2
      && region.connections.every(c => c.platform === region.region && id(c.user_service_id) && id(c.catalog_service_id)
        && text(c.name) && c.name.length <= 256 && ['personal', 'organization'].includes(c.source))
      && region.connections.filter(c => c.user_service_id === relay.binding.user_service_id
        && c.catalog_service_id === relay.binding.catalog_service_id).length === 1, 'lark_selected_unselected_connections_required');
    connections.push(...region.connections.map(c => c.user_service_id));
  }
  requireValue([channels, records, apps, bots, connections].every(values => new Set(values).size === values.length),
    'lark_native_relay_custody_must_be_distinct');
  return lease.regions;
}

export function assertLarkCapabilities(ctx, capabilities) {
  const flags = capabilities?.relay_authoring;
  if (!flags || !['native_lark_writes_enabled', 'native_lark_intake_enabled', 'lark_resource_writes_enabled',
    'lark_message_effects_writes_enabled', 'multi_consumer_writes_enabled'].every(k => flags[k] === true)
    || flags.native_lark_controls_enabled !== false || flags.native_lark_forms_enabled !== false)
    throw new Blocked('lark_enabled_intake_resources_with_disabled_controls_fixture_required');
  for (const region of ['lark', 'feishu']) {
    const platforms = flags.platforms?.filter(p => p.id === region);
    ctx.assert(platforms?.length === 1
      && platforms[0].verification_modes?.some(v => v.id === 'native_event' && v.writes_enabled === true),
    'lark_actual_regional_relay_verification_capability');
  }
}

export function assertLarkSource(ctx, lease, region, kind, record) {
  const seed = region[kind], definition = record?.definition, source = definition?.sources?.[0];
  ctx.assert(record?.record_id === seed.recordId && record.channel_id === seed.channelId
    && Number.isSafeInteger(record.version) && record.version > 0 && text(definition?.name)
    && definition.owner?.subject === lease.ownerSubject && definition.creation?.ready === true
    && record.provider?.slug === 'cma_codex' && definition.sources.length === 1
    && definition.connection_ref == null, 'lark_actual_owned_source_record');
  ctx.assert(source.admission?.type === 'scoped'
    && same(source.admission.senders, {type: 'allowlist', ids: [seed.senderId]})
    && same(source.admission.chats, {type: 'allowlist', ids: [seed.chatId]})
    && source.admission.groups === 'mention' && [undefined, 'deny'].includes(source.admission.bot_messages),
  'lark_exact_sender_conversation_scope');
  if (kind === 'native') {
    ctx.assert(source.type === 'native_lark' && source.separate_app === true && same(source.identity, seed.identity)
      && source.lark_resources == null && source.lark_message_effects == null && source.route_ids == null
      && source.issuer == null && source.trust_normalized == null && source.verify_raw_event == null,
    'lark_native_app_has_no_relay_binding');
    ctx.assert(text(record.endpoints?.native_lark_webhook_url), 'lark_native_callback_advertised');
  } else {
    ctx.assert(source.type === 'nyxid_relay' && source.platform === region.region && source.issuer === seed.issuer
      && source.key_id === seed.keyId && same(source.route_ids, [seed.routeId]) && source.bot_id === seed.botId
      && source.trust_normalized !== true && source.verify_raw_event !== true
      && source.identity == null && source.separate_app == null && same(source.lark_resources, seed.binding)
      && same(source.lark_message_effects, {connection: seed.binding, delete: true, reaction: true}),
    'lark_relay_exact_selected_connection_and_effect_consent');
  }
}

export function assertLarkConnections(ctx, region, body) {
  ctx.assert(Array.isArray(body?.items) && body.items.length === region.connections.length
    && new Set(body.items.map(c => c.user_service_id)).size === body.items.length,
  'lark_discovery_exact_isolated_inventory');
  for (const expected of region.connections) {
    const actual = body.items.find(c => c.user_service_id === expected.user_service_id);
    ctx.assert(same(actual, expected), 'lark_discovery_actual_connection_metadata');
  }
}

function actualReceipt(ctx, lease, replica, receipt) {
  ctx.assert(validEgReceipt(receipt) && receipt.replica === replica && receipt.method === 'GET'
    && receipt.status === 200 && receipt.serving.source === lease.source && receipt.serving.image === lease.image,
  'lark_inspection_actual_eg_source_image_receipt');
}

// Read real public documents through both EG processes. Keep native and relay
// definitions separate; all stored state is compared again after the UI reads.
export async function readLarkSources(ctx, lease, previous) {
  validateLarkInspectionFixture(lease);
  const records = new Map(); let capabilities;
  const read = async (replica, path) => {
    const response = await egRequest(ctx, lease, {replica, path, ...bounded(ctx), returnResponse: true});
    actualReceipt(ctx, lease, replica, response.eg_receipt); return response.body;
  };
  for (const replica of [ctx.am, peer(ctx.am)]) {
    const observed = await read(replica, '/v1/channel-capabilities'); assertLarkCapabilities(ctx, observed);
    if (capabilities) ctx.assert(same(observed, capabilities), 'lark_same_capabilities_on_both_egs');
    else capabilities = observed;
    if (previous) ctx.assert(same(observed, previous.capabilities), 'lark_capabilities_unchanged');
    for (const region of lease.regions) {
      assertLarkConnections(ctx, region, await read(replica, '/v1/relay-resource-connections?platform=' + region.region));
      for (const kind of ['native', 'relay']) {
        const seed = region[kind], record = await read(replica, '/v1/channels/' + segment(seed.channelId));
        assertLarkSource(ctx, lease, region, kind, record);
        if (records.has(seed.channelId)) ctx.assert(same(record, records.get(seed.channelId)), 'lark_same_saved_source_on_both_egs');
        else records.set(seed.channelId, record);
        if (previous) ctx.assert(same(record, previous.records.get(seed.channelId)), 'lark_saved_source_unchanged');
      }
    }
  }
  return {records, capabilities};
}

async function checked(ctx, root, name) {
  const checkbox = root.getByRole('checkbox', {name, exact: true});
  await visible(ctx, checkbox, 'lark_saved_consent_visible');
  ctx.assert(await checkbox.isChecked(), 'lark_saved_consent_checked');
}

export async function inspectNativeLarkProducer(ctx, page, region) {
  const dialog = await openChannelNode(ctx, page, `Producer: ${title(region.region)} (Direct ${title(region.region)} app)`);
  const group = dialog.getByRole('group', {name: 'Lark / Feishu app', exact: true});
  await visible(ctx, group, 'lark_native_identity_fields_visible');
  for (const [name, key] of [['App ID', 'app_id'], ['Tenant key', 'tenant_key'], ['Bot open ID', 'bot_open_id']])
    ctx.assert(await group.getByLabel(name, {exact: true}).inputValue() === region.native.identity[key], 'lark_native_saved_identity_visible');
  ctx.assert(await group.getByRole('button', {name: 'App region', exact: true}).innerText()
    === (region.region === 'lark' ? 'Lark (international)' : 'Feishu (China)'), 'lark_native_saved_region_visible');
  await checked(ctx, group, 'This is a separate app with its own webhook');
  await visible(ctx, group.getByText(/Keep existing NyxID app webhooks unchanged/), 'lark_separate_app_webhook_guidance');
  await visible(ctx, group.getByText(/Interactive card, button and choice replies are disabled on this gateway/),
    'lark_disabled_native_controls_notice');
  await visible(ctx, group.getByText(/Form replies are disabled/), 'lark_disabled_native_forms_notice');
  for (const name of ['Read media in rich-text posts', 'Allow message effects'])
    ctx.assert(await dialog.getByRole('checkbox', {name, exact: true}).count() === 0, 'lark_native_does_not_offer_relay_bindings');
  ctx.assert(await dialog.getByRole('button', {name: 'Messages from bots', exact: true}).count() === 0,
    'lark_unsupported_native_bot_control_not_offered');
  await dialog.getByRole('button', {name: 'Group messages', exact: true}).click();
  const menu = page.getByRole('menu', {name: 'Group messages', exact: true});
  await visible(ctx, menu, 'lark_native_group_policy_menu_visible');
  ctx.assert(await menu.getByRole('menuitemradio').count() === 3, 'lark_native_group_policy_options_bounded');
  for (const name of ['deny', 'all', 'mention']) {
    const item = menu.getByRole('menuitemradio', {name, exact: true});
    await visible(ctx, item, 'lark_native_supported_group_policy');
    ctx.assert((await item.getAttribute('aria-checked') === 'true') === (name === 'mention'),
      'lark_native_saved_group_policy_selected');
  }
  await page.keyboard.press('Escape'); await menu.waitFor({state: 'hidden'});
  await closeChannelInspector(ctx, page);
}

async function inspectConnectionSelector(ctx, lease, session, replica, region, group) {
  const result = await channelRead(ctx, session, '/v1/relay-resource-connections?platform=' + region.region,
    () => group.getByRole('button', {name: 'Load my bot connections', exact: true}).click());
  actualReceipt(ctx, lease, replica, result.receipt); assertLarkConnections(ctx, region, result.body);
  const selected = region.connections.find(c => c.user_service_id === region.relay.binding.user_service_id);
  const label = c => `${c.name} · ${c.source === 'organization' ? 'Organization' : 'Personal'} · ${c.user_service_id}`;
  const picker = group.getByRole('button', {name: 'Existing bot API connection', exact: true});
  ctx.assert(await picker.innerText() === label(selected), 'lark_compiled_selector_retains_exact_connection');
  await picker.click();
  const menu = session.page.getByRole('menu', {name: 'Existing bot API connection', exact: true});
  await visible(ctx, menu, 'lark_connection_menu_visible');
  for (const connection of region.connections) {
    const item = menu.getByRole('menuitemradio', {name: label(connection), exact: true});
    await visible(ctx, item, 'lark_connection_inventory_rendered');
    ctx.assert((await item.getAttribute('aria-checked') === 'true') === (connection === selected),
      'lark_unselected_connection_not_silently_granted');
  }
  await session.page.keyboard.press('Escape'); await menu.waitFor({state: 'hidden'});
  await visible(ctx, group.getByText(/does not grant/), 'lark_connection_selection_not_authorization_notice');
}

export async function inspectRelayLarkProducer(ctx, lease, session, replica, region) {
  const dialog = await openChannelNode(ctx, session.page, `Producer: ${title(region.region)} (NyxID relay)`);
  const source = dialog.getByRole('group', {name: 'NyxID connection', exact: true});
  for (const [name, value] of [['NyxID issuer', region.relay.issuer], ['Signing key ID', region.relay.keyId],
    ['Route IDs 1', region.relay.routeId], ['Bot ID', region.relay.botId]])
    ctx.assert(await source.getByLabel(name, {exact: true}).inputValue() === value, 'lark_relay_saved_identity_visible');
  ctx.assert(await source.getByRole('button', {name: 'Messaging platform', exact: true}).innerText() === region.region,
    'lark_relay_saved_region_visible');
  const resources = dialog.getByRole('group', {name: 'Embedded images and videos · advanced setup', exact: true});
  const effects = dialog.getByRole('group', {name: 'Manage the bot’s sent messages', exact: true});
  await checked(ctx, resources, 'Read media in rich-text posts');
  await checked(ctx, effects, 'Allow message effects');
  await checked(ctx, effects, 'Delete sent messages');
  await checked(ctx, effects, 'Add, replace and clear the bot’s reaction');
  for (const [group, names] of [[resources, ['NyxID service connection ID', 'NyxID catalog service ID', 'Receiving app ID']],
    [effects, ['Message effects connection ID', 'Message effects catalog service ID', 'Message effects app ID']]]) {
    for (const [i, field] of ['user_service_id', 'catalog_service_id', 'app_id'].entries())
      ctx.assert(await group.getByLabel(names[i], {exact: true}).inputValue() === region.relay.binding[field],
        'lark_relay_selected_binding_visible');
    await inspectConnectionSelector(ctx, lease, session, replica, region, group);
  }
  ctx.assert(await dialog.getByRole('group', {name: 'Lark / Feishu app', exact: true}).count() === 0,
    'lark_relay_does_not_offer_native_app_fields');
  await closeChannelInspector(ctx, session.page);
}

export async function inspectLarkSources(ctx, lease) {
  const initial = await readLarkSources(ctx, lease);
  assertRelayCounters(ctx, await invoke(lease, 'counters', bounded(ctx)), 0, 0);
  const session = await openBrowser(ctx, 'user', {channelEditor: true});
  for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
    requireValue(typeof session[method] === 'function', 'lark_actual_eg_browser_required');
  const guard = await guardDiscoveryWrites(ctx, session.page);
  for (const replica of [ctx.am, peer(ctx.am)]) {
    await session.switchEG(replica);
    for (const region of lease.regions) for (const kind of ['native', 'relay']) {
      const expected = initial.records.get(region[kind].channelId);
      const selected = await openListedChannel(ctx, session, expected, () => goto(session.page, '/event-gateway'));
      actualReceipt(ctx, lease, replica, selected.receipt);
      ctx.assert(same(selected.body, expected), 'lark_compiled_ui_actual_saved_source');
      if (kind === 'native') await inspectNativeLarkProducer(ctx, session.page, region);
      else await inspectRelayLarkProducer(ctx, lease, session, replica, region);
    }
  }
  await uiReceipt(ctx, session.page, 'lark_native_relay_source_ui');
  const evidence = await invoke(ctx.fixture('browser'), 'evidence', session.page);
  ctx.assert(evidence.compiled_source === lease.source && guard.attempts() === 0, 'lark_source_ui_exact_build_no_mutations');
  await readLarkSources(ctx, lease, initial);
  assertRelayCounters(ctx, await invoke(lease, 'counters', bounded(ctx)), 0, 0);
  await ctx.observe('lark_separate_saved_sources', async () => ({schema: 'multi-eg-lark-source-inspection-v1',
    source: lease.source, image: lease.image, record_count: initial.records.size,
    definitions_sha256: [...initial.records.values()].map(digest), capabilities_sha256: digest(initial.capabilities),
    resource_access_verified: false}));
  return initial;
}
