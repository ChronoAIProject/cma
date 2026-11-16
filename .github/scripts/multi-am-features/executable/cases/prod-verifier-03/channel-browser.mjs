import {invoke, visible, segment} from './helpers.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';

const prefix = '/api/event-gateway';

export const channelNodeName = name => `Channel: ${name} (Route events to agents)`;

export async function closeChannelInspector(ctx, page) {
  const dialogs = page.getByRole('dialog');
  const count = await dialogs.count();
  ctx.assert(count <= 1, 'channel_single_inspector');
  if (count) {
    await dialogs.getByRole('button', {name: 'Close', exact: true}).click();
    await dialogs.waitFor({state: 'hidden'});
    ctx.assert(!await dialogs.isVisible(), 'channel_inspector_closed_before_navigation');
  }
}

// NodeInspector is a retained modal. Its mask intercepts the graph until Close
// completes; selecting a different node must follow the same user interaction.
export async function openChannelNode(ctx, page, name) {
  await closeChannelInspector(ctx, page);
  const node = page.getByRole('region', {name: 'Delivery map', exact: true})
    .getByRole('button', {name, exact: true});
  await node.click();
  const dialog = page.getByRole('dialog');
  await visible(ctx, dialog, 'channel_node_inspector_visible');
  ctx.assert(await node.getAttribute('aria-pressed') === 'true', 'channel_node_selected');
  return dialog;
}

export function channelReceipt(ctx, receipts, requestId, method = 'GET', status = 200) {
  const matches = Array.isArray(receipts) ? receipts.filter(r => r.request_id === requestId) : [];
  ctx.assert(typeof requestId === 'string' && requestId.length > 0 && matches.length === 1
    && matches[0].method === method && matches[0].status === status && validEgReceipt(matches[0]),
  'channel_actual_eg_browser_receipt');
  return matches[0];
}

export async function channelRead(ctx, session, path, action) {
  const expected = new URL(prefix + path, session.origin);
  const [response] = await Promise.all([
    session.page.waitForResponse(r => r.request().method() === 'GET'
      && r.url() === expected.href,
    {timeout: Math.max(1, Math.min(ctx.remainingMs?.() ?? 15000, 15000))}),
    action(),
  ]);
  ctx.assert(response.status() === 200, 'channel_browser_read_status');
  const body = await response.json();
  await invoke(session, 'networkIdle');
  const receipt = channelReceipt(ctx, await invoke(session, 'egEvidence'), response.headers()['x-request-id']);
  await ctx.observe('channel_browser_eg_read', async () => ({schema: 'multi-eg-request-v1', receipt}));
  return {body, receipt};
}

// EventGateway uses an addressed list item directly. Only an unaddressed item
// triggers ExistingChannel's GET; a mandatory detail GET would hang on normal UI.
export async function openListedChannel(ctx, session, expected, navigate) {
  let result = await channelRead(ctx, session, '/v1/channels', navigate);
  const seen = new Set(), cursors = new Set();
  for (let n = 0; n < 20; n++) {
    ctx.assert(Array.isArray(result.body.items), 'channel_browser_list_items');
    for (const row of result.body.items) {
      ctx.assert(typeof row.record_id === 'string' && !seen.has(row.record_id), 'channel_browser_list_duplicate');
      seen.add(row.record_id);
    }
    const found = result.body.items.find(row => row.record_id === expected.record_id);
    if (found) {
      ctx.assert(found.definition?.name === expected.definition.name, 'channel_browser_list_name');
      const page = session.page;
      const row = page.getByRole('tree', {name: 'Channels', exact: true}).getByRole('treeitem')
        .filter({has: page.getByText(expected.definition.name, {exact: true})});
      ctx.assert(await row.count() === 1, 'channel_browser_owned_row_unique');
      const addressed = typeof found.channel_id === 'string' && found.channel_id.length > 0
        && typeof found.endpoints?.custom_event_url === 'string'
        && typeof found.endpoints?.nyxid_callback_url === 'string';
      const click = () => row.getByRole('button').click();
      if (addressed) await click();
      else result = await channelRead(ctx, session, found.channel_id
        ? '/v1/channels/' + segment(found.channel_id) : '/v1/channel-records/' + segment(found.record_id), click);
      const selected = addressed ? found : result.body;
      ctx.assert(selected.record_id === expected.record_id && selected.channel_id === expected.channel_id
        && selected.version === expected.version, 'channel_browser_selected_identity_version');
      await visible(ctx, page.getByRole('heading', {name: expected.definition.name, exact: true}),
        'channel_browser_selected_heading');
      ctx.assert(await row.getAttribute('aria-selected') === 'true', 'channel_browser_selected_row');
      return {body: selected, receipt: result.receipt};
    }
    const cursor = result.body.next_cursor;
    ctx.assert(typeof cursor === 'string' && cursor.length > 0 && !cursors.has(cursor), 'channel_browser_list_target_missing');
    cursors.add(cursor);
    result = await channelRead(ctx, session, '/v1/channels?after=' + segment(cursor),
      () => session.page.getByRole('button', {name: 'Load more channels', exact: true}).click());
  }
  ctx.assert(false, 'channel_browser_list_budget');
}
