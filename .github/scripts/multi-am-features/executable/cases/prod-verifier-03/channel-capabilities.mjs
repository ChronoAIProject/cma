import {define, acquire, openBrowser, egRequest, invoke, visible, goto, peer, segment, same, uiReceipt} from './helpers.mjs';
import {Blocked} from '../../runner.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';

const prefix = '/api/event-gateway';
const modes = ['raw_event', 'trusted_normalized'];
const modeLabels = ['Verify original platform event', 'Trust normalized NyxID messages'];
const platforms = ['telegram', 'lark', 'feishu', 'discord', 'whatsapp', 'slack'];

export function assertCapabilityShape(ctx, value) {
  ctx.assert(Array.isArray(value.platforms) && value.platforms.length > 0
    && value.platforms.every(p => p.id && p.label && Array.isArray(p.connection_methods)
      && Array.isArray(p.optional_capabilities)), 'channel_managed_connection_capabilities');
  const relay = value.relay_authoring;
  if (!relay) throw new Blocked('channel_relay_authoring_discovery_required');
  ctx.assert(['dynamic_sources_writes_enabled', 'relay_raw_verification_writes_enabled',
    'multi_consumer_writes_enabled', 'native_bot_mentions_writes_enabled', 'thread_contract_writes_enabled']
    .every(key => typeof relay[key] === 'boolean'), 'channel_relay_gate_shape');
  ctx.assert(Array.isArray(relay.platforms) && relay.platforms.length > 0
    && new Set(relay.platforms.map(p => p.id)).size === relay.platforms.length
    && relay.platforms.every(p => p.id && p.label && Array.isArray(p.verification_modes)
      && p.verification_modes.length > 0
      && new Set(p.verification_modes.map(m => m.id)).size === p.verification_modes.length
      && p.verification_modes.every(m => m.id && typeof m.writes_enabled === 'boolean')),
  'channel_relay_verification_capabilities');
  return relay;
}

export function sourceChoices(relay, provider = 'cma_codex') {
  return [
    {label: 'NyxID messaging connection', present: true, enabled: true},
    {label: 'Direct Telegram bot', present: true, enabled: true},
    {label: 'Custom event', present: true, enabled: true},
    {label: 'Direct Lark / Feishu app', present: relay.native_lark_writes_enabled === true
      && relay.native_lark_intake_enabled === true, enabled: true},
    {label: 'Direct WhatsApp Business', present: relay.native_whatsapp_writes_enabled === true
      && relay.native_whatsapp_auto_activation_enabled === true, enabled: true},
    {label: 'Direct Discord bot', present: relay.native_discord_writes_enabled === true
      && relay.native_discord_auto_activation_enabled === true && relay.multi_consumer_writes_enabled === true,
    enabled: relay.response_recovery_providers == null || relay.response_recovery_providers.includes(provider)},
  ];
}

export function egBrowserReceipt(ctx, receipts, requestId, status = 200) {
  const matches = Array.isArray(receipts) ? receipts.filter(r => r.request_id === requestId) : [];
  ctx.assert(typeof requestId === 'string' && requestId && matches.length === 1
    && matches[0].method === 'GET' && matches[0].status === status && validEgReceipt(matches[0]),
  'channel_browser_actual_eg_receipt');
  return matches[0];
}

async function readCapabilitiesUI(ctx, session) {
  const [response] = await Promise.all([
    session.page.waitForResponse(r => r.request().method() === 'GET'
      && new URL(r.url()).origin === session.origin
      && new URL(r.url()).pathname === prefix + '/v1/channel-capabilities',
    {timeout: Math.max(1, Math.min(ctx.remainingMs?.() ?? 15000, 15000))}),
    goto(session.page, '/event-gateway'),
  ]);
  ctx.assert(response.status() === 200, 'channel_browser_capabilities_status');
  const body = await response.json();
  const receipt = egBrowserReceipt(ctx, await invoke(session, 'egEvidence'), response.headers()['x-request-id']);
  await ctx.observe('channel_browser_capabilities_receipt', async () => ({schema: 'multi-eg-request-v1', receipt}));
  return {body, receipt};
}

// This discovery case is read-only. A broken UI attempting an automatic save is
// rejected before dispatch and remains a failure, including during cleanup.
export async function guardDiscoveryWrites(ctx, page) {
  const pending = new Set(), failures = []; let attempts = 0;
  const matcher = url => url.origin === new URL(page.url()).origin && url.pathname.startsWith(prefix + '/');
  const handler = route => {
    const work = (async () => {
      if (['GET', 'HEAD', 'OPTIONS'].includes(route.request().method())) return route.fallback();
      attempts++;
      try {await route.abort('blockedbyclient');} catch (error) {failures.push(error);}
    })();
    pending.add(work); return work.finally(() => pending.delete(work));
  };
  await page.route(matcher, handler);
  ctx.defer('remove_channel_discovery_write_guard', async () => {
    const errors = [];
    try {await page.unroute(matcher, handler);} catch (error) {errors.push(error);}
    const drained = await Promise.allSettled([...pending]);
    errors.push(...drained.filter(r => r.status === 'rejected').map(r => r.reason), ...failures);
    if (attempts) errors.push(Error('channel_discovery_attempted_write'));
    if (errors.length) throw new AggregateError(errors, 'channel_discovery_guard_failed');
  });
  return {attempts: () => attempts};
}

const menuItem = (menu, label) => menu.getByRole('menuitemradio').filter({hasText: label});
async function menu(ctx, page, label) {
  await page.getByRole('button', {name: label, exact: true}).click();
  const result = page.getByRole('menu', {name: label, exact: true});
  await visible(ctx, result, 'channel_choice_menu_visible'); return result;
}
async function assertEnabled(ctx, item, enabled) {
  ctx.assert(await item.count() === 1, 'channel_choice_unique');
  ctx.assert((await item.getAttribute('aria-disabled') !== 'true') === enabled, 'channel_choice_capability_gate');
}
const counts = async (ctx, lease) => {
  const value = await invoke(lease, 'counters');
  for (const key of ['platformConnections', 'providerTurns', 'deliveries'])
    ctx.assert(Number.isSafeInteger(value[key]) && value[key] >= 0, 'channel_discovery_effect_counters');
  return {platformConnections: value.platformConnections, providerTurns: value.providerTurns, deliveries: value.deliveries};
};

export const channelCapabilitiesCase = {...define(19, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let lease, session, capability, relay, before, baseline, guard;
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'channel_capabilities');
    before = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId)});
    baseline = await counts(ctx, lease);
    const source = before.definition?.sources?.[0];
    if (source?.type !== 'nyxid_relay' || source.platform !== 'discord' || source.verify_raw_event !== true
      || before.definition.connection_ref || before.definition.sources.length !== 1)
      throw new Blocked('channel_saved_disabled_raw_fixture_required');
    ctx.assert(before.channel_id === lease.channelId && before.definition.name === lease.channelName
      && before.definition.owner?.subject === lease.expected.ownerSubject, 'channel_discovery_owned_fixture');
    session = await openBrowser(ctx, 'user', {channelEditor: true});
    for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
      if (typeof session[method] !== 'function') throw new Blocked('channel_actual_eg_browser_binding_required');
    guard = await guardDiscoveryWrites(ctx, session.page);
    const observed = await readCapabilitiesUI(ctx, session); capability = observed.body;
    relay = assertCapabilityShape(ctx, capability);
    const raw = relay.platforms.find(p => p.id === 'discord')?.verification_modes.find(m => m.id === 'raw_event');
    const trusted = relay.platforms.find(p => p.id === 'discord')?.verification_modes.find(m => m.id === 'trusted_normalized');
    if (raw?.writes_enabled !== false || trusted?.writes_enabled !== true)
      throw new Blocked('channel_disabled_raw_enabled_normalized_fixture_required');
    await session.switchEG(peer(observed.receipt.replica));
    const other = await readCapabilitiesUI(ctx, session);
    ctx.assert(other.receipt.replica !== observed.receipt.replica && same(other.body, capability), 'channel_capabilities_both_actual_egs');
    const api = await egRequest(ctx, lease, {path: '/v1/channel-capabilities', replica: other.receipt.replica});
    ctx.assert(same(api, capability), 'channel_ui_uses_actual_capability_document');
  });
  await ctx.step('step_2', async () => {
    const page = session.page;
    await page.getByRole('button', {name: 'New channel', exact: true}).click();
    await visible(ctx, page.getByRole('heading', {name: 'New channel', exact: true}), 'channel_new_form_open');
    let choices = await menu(ctx, page, 'Producer 1 connection type');
    for (const choice of sourceChoices(relay)) {
      const item = menuItem(choices, choice.label);
      if (choice.present) await assertEnabled(ctx, item, choice.enabled);
      else ctx.assert(await item.count() === 0, 'channel_unavailable_new_native_setup_hidden');
    }
    await page.keyboard.press('Escape');
    choices = await menu(ctx, page, 'Messaging platform');
    for (const platform of platforms) await assertEnabled(ctx, choices.getByRole('menuitemradio', {name: new RegExp('^' + platform + '(?:$| )')}),
      relay.platforms.some(p => p.id === platform));
    await choices.getByRole('menuitemradio', {name: 'discord', exact: true}).click();
    ctx.assert(await page.getByRole('button', {name: 'Incoming message verification', exact: true}).innerText() === 'Choose verification',
      'channel_new_source_no_implicit_trust');
    choices = await menu(ctx, page, 'Incoming message verification');
    for (let i = 0; i < modes.length; i++) await assertEnabled(ctx, menuItem(choices, modeLabels[i]),
      relay.platforms.find(p => p.id === 'discord').verification_modes.find(m => m.id === modes[i])?.writes_enabled === true);
    await menuItem(choices, modeLabels[1]).click();
    ctx.assert(await page.getByRole('button', {name: 'Incoming message verification', exact: true}).innerText() === modeLabels[1],
      'channel_supported_trust_requires_explicit_selection');
    await visible(ctx, page.getByText(/support here does not start a Discord Gateway connection/), 'channel_discovery_not_connectivity');
    choices = await menu(ctx, page, 'Destination 1 type');
    for (const label of ['Original conversation', 'Result webhook', 'Synchronous response'])
      await assertEnabled(ctx, menuItem(choices, label), true);
    await page.keyboard.press('Escape');
    const thread = page.getByRole('checkbox', {name: 'Enable verified thread following', exact: true});
    ctx.assert(!await thread.isChecked() && await thread.isDisabled() === !relay.thread_contract_writes_enabled,
      'channel_new_thread_delivery_gate');
    await page.getByRole('button', {name: 'Cancel', exact: true}).click();
    await page.getByRole('tree', {name: 'Channels', exact: true}).getByRole('treeitem')
      .filter({has: page.getByText(lease.channelName, {exact: true})}).getByRole('button').click();
    await visible(ctx, page.getByRole('heading', {name: lease.channelName, exact: true}), 'channel_saved_configuration_open');
    await page.getByRole('button', {name: 'Producer: Discord (NyxID relay)', exact: true}).click();
    const dialog = page.getByRole('dialog', {name: 'Discord', exact: true});
    await visible(ctx, dialog, 'channel_saved_producer_open');
    const verification = dialog.getByRole('button', {name: 'Incoming message verification', exact: true});
    ctx.assert(await verification.innerText() === modeLabels[0], 'channel_saved_raw_choice_retained');
    choices = await menu(ctx, page, 'Incoming message verification');
    const retained = menuItem(choices, modeLabels[0]);
    await assertEnabled(ctx, retained, false);
    ctx.assert(await retained.getAttribute('aria-checked') === 'true', 'channel_disabled_saved_choice_selected');
    await page.keyboard.press('Escape');
    await visible(ctx, dialog.getByText(/there is no automatic switch to normalized trust/), 'channel_no_automatic_downgrade_notice');
    const username = dialog.getByLabel('Bot username', {exact: true});
    const changed = (await username.inputValue()) + '_draft'; await username.fill(changed);
    await dialog.getByRole('button', {name: 'Save producer', exact: true}).click();
    await visible(ctx, dialog.getByRole('alert').filter({hasText: 'Your draft has been retained'}), 'channel_disabled_write_refused_locally');
    ctx.assert(await username.inputValue() === changed && await verification.innerText() === modeLabels[0], 'channel_refused_draft_not_downgraded');
    await dialog.getByRole('button', {name: 'Close', exact: true}).click();
    await invoke(session, 'networkIdle');
    ctx.assert(guard.attempts() === 0, 'channel_capability_discovery_no_write_attempt');
  });
  await ctx.step('expected_result', async () => {
    for (const replica of ['a', 'b']) {
      const after = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(lease.channelId), replica});
      ctx.assert(same(after, before), 'channel_discovery_preserves_exact_saved_record');
    }
    ctx.assert(same(await counts(ctx, lease), baseline), 'channel_discovery_has_no_platform_or_provider_effect');
    const receipts = await invoke(session, 'egEvidence');
    ctx.assert(receipts.length > 0 && receipts.every(r => validEgReceipt(r)
      && ['GET', 'HEAD', 'OPTIONS'].includes(r.method)), 'channel_browser_discovery_only_reads');
    for (const receipt of receipts) await ctx.observe('channel_browser_eg_read', async () => ({schema: 'multi-eg-request-v1', receipt}));
    await uiReceipt(ctx, session.page);
  });
}), evidence_scope: 'eg_only'};
