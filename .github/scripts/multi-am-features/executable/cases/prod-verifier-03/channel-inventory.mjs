import {Blocked} from '../../runner.mjs';
import {define, acquire, openBrowser, egRequest, invoke, visible, goto, peer, segment, same, digest, uiReceipt} from './helpers.mjs';
import {channelRead, closeChannelInspector, openChannelNode, channelNodeName} from './channel-browser.mjs';
import {guardDiscoveryWrites} from './channel-capabilities.mjs';
import {activityWait} from './activity-observer.mjs';

const text = value => typeof value === 'string' && value.length > 0;
const hash = value => /^[a-f0-9]{64}$/.test(value ?? '');
const countKeys = ['turns', 'effects', 'deliveries', 'ingress'];

export function validateInventoryFixture(lease) {
  const e = lease.expected;
  if (!text(e?.ownerSubject) || !Array.isArray(e?.channels) || e.channels.length < 51 || e.channels.length > 100
    || !e.channels.every(c => text(c.recordId) && text(c.name) && text(c.creator)
      && Number.isSafeInteger(c.version) && c.version > 0 && Number.isFinite(Date.parse(c.createdAt))
      && hash(c.definitionDigest) && typeof c.addressed === 'boolean'
      && (c.addressed ? text(c.channelId) : c.channelId == null))
    || new Set(e.channels.map(c => c.recordId)).size !== e.channels.length
    || new Set(e.channels.map(c => c.name)).size !== e.channels.length
    || !e.channels.some(c => c.recordId === e.selectedRecordId && c.addressed && c.version > 1)
    || !e.channels.some(c => c.recordId === e.missingRecordId && !c.addressed)
    || !text(e.foreignRecordId) || e.channels.some(c => c.recordId === e.foreignRecordId))
    throw new Blocked('channel_inventory_two_page_owned_and_legacy_seed_required');
  if (!Array.isArray(e.nodes) || e.nodes.length < 4
    || new Set(e.nodes.map(node => node.name)).size !== e.nodes.length
    || !['producer', 'consumer'].every(kind => e.nodes.filter(node => node.kind === kind).length >= 2)
    || !e.nodes.every(node => ['producer', 'consumer'].includes(node.kind) && text(node.name)
      && node.facts && Object.keys(node.facts).length > 0
      && Object.entries(node.facts).every(([key, value]) => text(key) && text(value))
      && (node.kind === 'producer' ? text(node.facts['Access policy'])
        : ['Consumer ID', 'Provider', 'Binding'].every(key => text(node.facts[key]))))
    || !e.nodes.some(node => node.kind === 'consumer' && node.facts['Consumer ID'] === 'default'))
    throw new Blocked('channel_inventory_seeded_topology_facts_required');
  for (const method of ['request', 'counters', 'quiesce', 'scanCredentialValues', 'scanBrowserForCredentialValues'])
    if (typeof lease[method] !== 'function') throw new Blocked('channel_inventory_primitive_' + method.toLowerCase());
  return e;
}

export function assertInventoryRecord(ctx, record, expected, ownerSubject) {
  ctx.assert(record?.record_id === expected.recordId && record.version === expected.version
    && record.definition?.name === expected.name && record.definition.owner?.subject === ownerSubject
    && record.definition.created_at === expected.createdAt
    && digest(record.definition) === expected.definitionDigest, 'channel_inventory_exact_saved_record');
  const addressed = text(record.channel_id) && typeof record.endpoints?.custom_event_url === 'string'
    && typeof record.endpoints?.nyxid_callback_url === 'string';
  ctx.assert(addressed === expected.addressed
    && (expected.addressed ? record.channel_id === expected.channelId : record.channel_id == null && record.endpoints == null),
  'channel_inventory_exact_saved_address');
}

export function acceptInventoryPage(ctx, body, expected, collected, cursors) {
  ctx.assert(Array.isArray(body?.items) && body.items.length > 0 && body.items.length <= 50
    && (body.next_cursor === null || text(body.next_cursor)), 'channel_inventory_page_shape');
  for (const record of body.items) {
    const seeded = expected.channels.find(channel => channel.recordId === record.record_id);
    ctx.assert(seeded && !collected.some(previous => previous.record_id === record.record_id),
      'channel_inventory_owned_unique_records');
    assertInventoryRecord(ctx, record, seeded, expected.ownerSubject); collected.push(record);
  }
  if (body.next_cursor !== null) {
    ctx.assert(body.items.length === 50 && !cursors.has(body.next_cursor)
      && body.next_cursor === body.items.at(-1).record_id, 'channel_inventory_cursor_progress');
    cursors.add(body.next_cursor);
  } else ctx.assert(collected.length === expected.channels.length, 'channel_inventory_complete_owned_set');
}

export function assertInventoryCounters(ctx, before, after) {
  ctx.assert([before, after].every(value => value && countKeys.every(key =>
    Number.isSafeInteger(value[key]) && value[key] >= 0)), 'channel_inventory_scoped_effect_counters');
  ctx.assert(countKeys.every(key => before[key] === after[key]), 'channel_inventory_no_provider_platform_effects');
}

// Read only actual DOM metadata. Date formatting is locale-dependent, so join
// the datetime and the exact source-backed metadata tooltip instead of a clock.
export function readInventoryRows(elements) {
  return elements.map(row => ({record: row.getAttribute('data-node-id'),
    name: row.querySelector('.tree-row__name')?.textContent,
    createdAt: row.querySelector('.tree-row__age time')?.getAttribute('datetime'),
    metadata: row.querySelector('.tree-row__age')?.getAttribute('title')}));
}

export const channelInventoryCase = {...define(18, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let lease, expected, session, guard, baseline, firstReplica, lastReplica, records;
  const scanBody = async body => {
    const scan = await invoke(lease, 'scanCredentialValues', {value: body});
    ctx.assert(scan?.scanned === true && scan.matches === 0, 'channel_inventory_no_credential_in_response');
  };
  const scanPage = async () => {
    const scan = await invoke(lease, 'scanBrowserForCredentialValues', {page: session.page});
    ctx.assert(scan?.scannedText === true && scan.scannedInputs === true && scan.matches === 0,
      'channel_inventory_no_credential_in_ui');
  };
  const rendered = async collected => {
    const want = collected.map(record => {
      const seeded = expected.channels.find(c => c.recordId === record.record_id);
      return {record: 'channel:' + seeded.recordId, name: seeded.name, createdAt: seeded.createdAt,
        metadata: `Created ${seeded.createdAt} · By ${seeded.creator}`};
    });
    await activityWait(ctx, async () => same(await session.page.getByRole('tree', {name: 'Channels', exact: true})
      .getByRole('treeitem').evaluateAll(readInventoryRows), want), 'channel_inventory_rendered_records_metadata');
    await scanPage();
  };
  const readList = async action => {
    let response = await channelRead(ctx, session, '/v1/channels', action);
    const replica = response.receipt.replica, collected = [], cursors = new Set(); let pages = 0;
    for (;;) {
      ctx.assert(++pages <= 2 && response.receipt.replica === replica, 'channel_inventory_bounded_actual_replica');
      await scanBody(response.body);
      acceptInventoryPage(ctx, response.body, expected, collected, cursors);
      await rendered(collected);
      if (response.body.next_cursor === null) break;
      const cursor = response.body.next_cursor;
      response = await channelRead(ctx, session, '/v1/channels?after=' + segment(cursor),
        () => session.page.getByRole('button', {name: 'Load more channels', exact: true}).click());
    }
    ctx.assert(pages === 2 && await session.page.getByRole('button', {name: 'Load more channels', exact: true}).count() === 0,
      'channel_inventory_two_complete_pages');
    await ctx.observe('channel_inventory_list', async () => ({replica, count: collected.length,
      pages, records_sha256: digest(collected)}));
    return {replica, collected};
  };
  const select = async seeded => {
    await closeChannelInspector(ctx, session.page);
    const row = session.page.getByRole('tree', {name: 'Channels', exact: true}).getByRole('treeitem')
      .filter({has: session.page.getByText(seeded.name, {exact: true})});
    ctx.assert(await row.count() === 1 && await row.getAttribute('data-node-id') === 'channel:' + seeded.recordId,
      'channel_inventory_selected_row_identity');
    await row.getByRole('button').click();
    await visible(ctx, session.page.getByRole('heading', {name: seeded.name, exact: true}), 'channel_inventory_selected_heading');
    ctx.assert(await row.getAttribute('aria-selected') === 'true', 'channel_inventory_selected_row');
  };
  const fact = async (dialog, label, value) => {
    const row = dialog.locator('.channel-inspector__summary .channel-inspector__facts > div')
      .filter({has: session.page.getByText(label, {exact: true})});
    ctx.assert(await row.count() === 1 && (await row.locator('dd').innerText()) === value,
      'channel_inventory_saved_inspector_fact');
  };
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'channel_inventory'); expected = validateInventoryFixture(lease);
    baseline = await invoke(lease, 'counters');
    session = await openBrowser(ctx, 'user', {channelInventory: true, initialPath: 'about:blank'});
    for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
      if (typeof session[method] !== 'function') throw new Blocked('channel_inventory_actual_eg_browser_required');
    ctx.assert(session.identity?.subject === expected.ownerSubject, 'channel_inventory_owner_cookie');
    guard = await guardDiscoveryWrites(ctx, session.page);
    const initial = await readList(() => goto(session.page, '/event-gateway')); firstReplica = initial.replica;
    await session.switchEG(peer(firstReplica));
    const refreshed = await readList(() => session.page.getByRole('button', {name: 'Refresh channels', exact: true}).click());
    lastReplica = refreshed.replica; records = refreshed.collected;
    const savedRecords = values => values.map(record => ({record_id: record.record_id,
      channel_id: record.channel_id, version: record.version, definition: record.definition}));
    ctx.assert(lastReplica !== firstReplica && same(savedRecords(records), savedRecords(initial.collected)),
      'channel_inventory_actual_peer_refresh_same_saved_records');
  });
  await ctx.step('step_2', async () => {
    const seeded = expected.channels.find(c => c.recordId === expected.selectedRecordId);
    const saved = await egRequest(ctx, lease, {path: '/v1/channels/' + segment(seeded.channelId), replica: firstReplica});
    await scanBody(saved); assertInventoryRecord(ctx, saved, seeded, expected.ownerSubject);
    await select(seeded);
    const detail = session.page.getByRole('region', {name: seeded.name + ' details', exact: true});
    ctx.assert((await detail.locator('.page-header__chip').innerText())
      === `${saved.provider.slug} · ${saved.provider.level} · Version ${seeded.version}`, 'channel_inventory_header_version');
    const map = session.page.getByRole('region', {name: 'Delivery map', exact: true});
    await visible(ctx, map, 'channel_inventory_topology_visible');
    await visible(ctx, map.getByRole('button', {name: channelNodeName(seeded.name), exact: true}),
      'channel_inventory_lazy_graph_ready');
    const names = await map.locator('button[data-kind="producer"], button[data-kind="consumer"]')
      .evaluateAll(nodes => nodes.map(node => node.getAttribute('aria-label')));
    ctx.assert(same(names.sort(), expected.nodes.map(node => node.name).sort()), 'channel_inventory_exact_topology_nodes');
    let dialog = await openChannelNode(ctx, session.page, channelNodeName(seeded.name));
    await fact(dialog, 'Configuration version', String(seeded.version)); await scanPage();
    for (const node of expected.nodes) {
      dialog = await openChannelNode(ctx, session.page, node.name);
      for (const [label, value] of Object.entries(node.facts)) await fact(dialog, label, value);
      await scanPage();
    }
    const missing = expected.channels.find(c => c.recordId === expected.missingRecordId);
    const response = await channelRead(ctx, session, '/v1/channel-records/' + segment(missing.recordId), () => select(missing));
    await scanBody(response.body); assertInventoryRecord(ctx, response.body, missing, expected.ownerSubject);
    const legacy = session.page.getByRole('region', {name: missing.name + ' details', exact: true});
    await visible(ctx, legacy.getByText('This existing channel keeps its original configuration and address.', {exact: true}),
      'channel_inventory_missing_address_explained');
    const address = legacy.getByRole('textbox', {name: 'Original channel ID', exact: true});
    await visible(ctx, address, 'channel_inventory_original_address_prompt');
    ctx.assert(await address.inputValue() === '' && await legacy.getByRole('region', {name: 'Delivery map', exact: true}).count() === 0,
      'channel_inventory_no_invented_address_or_topology');
    await legacy.locator('summary').filter({hasText: /^Channel configuration$/}).click();
    ctx.assert(same(JSON.parse(await legacy.locator('pre').innerText()), response.body.definition),
      'channel_inventory_legacy_saved_configuration');
    await scanPage();
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(guard.attempts() === 0, 'channel_inventory_no_mutation_attempt');
    assertInventoryCounters(ctx, baseline, await invoke(lease, 'quiesce'));
    ctx.assert(records.length === expected.channels.length && firstReplica !== lastReplica,
      'channel_inventory_complete_both_eg_observations');
    // Read both addressed and legacy records after inspection; no UI-only
    // comparison can establish that saved versions/configuration stayed intact.
    for (const replica of [firstReplica, lastReplica]) for (const id of [expected.selectedRecordId, expected.missingRecordId]) {
      const seeded = expected.channels.find(c => c.recordId === id);
      const saved = await egRequest(ctx, lease, {replica, path: seeded.addressed
        ? '/v1/channels/' + segment(seeded.channelId) : '/v1/channel-records/' + segment(seeded.recordId)});
      assertInventoryRecord(ctx, saved, seeded, expected.ownerSubject); await scanBody(saved);
    }
    await scanPage(); await uiReceipt(ctx, session.page);
  });
}), evidence_scope: 'eg_only'};
