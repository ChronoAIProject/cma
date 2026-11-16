import {define, acquire, openBrowser, visible, goto, invoke, same, peer, segment, uiReceipt} from './helpers.mjs';
import {Blocked} from '../../runner.mjs';
import {validServingReceipt} from '../../serving.mjs';

const labels = {creating: 'Creating', active: 'Active', disabled: 'Disabled', failed: 'Needs attention',
  deleting: 'Deleting', deleted: 'Deleted'};
const botPath = id => '/api/v1/bots/' + segment(id);
const canonical = path => path.replace(/^\/api\/agent-manager\//, '/api/');

export function requireInventory(lease) {
  const e = lease.expected;
  if (!e || typeof e.ownerSubject !== 'string' || !e.ownerSubject
    || typeof e.gatedSubject !== 'string' || !e.gatedSubject || e.gatedSubject === e.ownerSubject
    || !Array.isArray(e.ownedBotIds) || e.ownedBotIds.length <= 50 || e.ownedBotIds.length > 150
    || e.ownedBotIds.some(id => typeof id !== 'string' || !id)
    || new Set(e.ownedBotIds).size !== e.ownedBotIds.length
    || ![lease.botId, e.provisioningBotId, e.failedBotId, e.callbackBotId].every(id => e.ownedBotIds.includes(id))
    || new Set([lease.botId, e.provisioningBotId, e.failedBotId, e.callbackBotId]).size !== 4
    || typeof e.foreignBotId !== 'string' || !e.foreignBotId || e.ownedBotIds.includes(e.foreignBotId)
    || !Array.isArray(e.connections) || !e.connections.length
    || e.connections.some(c => !c.connection_ref || !c.label)
    || new Set(e.connections.map(c => c.connection_ref)).size !== e.connections.length
    || !Array.isArray(e.legacyBots) || !e.legacyBots.length
    || e.legacyBots.some(b => !b.id || !b.name || !b.platform)) {
    throw new Blocked('consumer_inventory_authentic_seed_required');
  }
  return e;
}

// A response ID binds the UI action to the actual process receipt. The selected
// AM is only a preference until this join succeeds; these are global Bot reads.
export function inventoryReceipt(ctx, evidence, requestId, status = 200) {
  if (evidence.authentication !== 'browser_session') throw new Blocked('consumer_inventory_cookie_session_required');
  const matches = evidence.requests?.filter(r => r.request_id === requestId) ?? [];
  ctx.assert(typeof requestId === 'string' && requestId.length > 0 && matches.length === 1
    && matches[0].method === 'GET' && matches[0].status === status && validServingReceipt(matches[0]),
  'consumer_inventory_actual_am_receipt');
  return matches[0];
}

async function readUI(ctx, session, path, action, after) {
  const page = session.page;
  const [response] = await Promise.all([
    page.waitForResponse(r => {
      const url = new URL(r.url());
      return url.origin === session.origin && r.request().method() === 'GET'
        && canonical(url.pathname) === path
        && (path !== '/api/v1/bots' || url.searchParams.get('after') === (after ?? null));
    }, {timeout: Math.max(1, Math.min(ctx.remainingMs?.() ?? 15000, 15000))}), action(),
  ]);
  ctx.assert(response.status() === 200, 'consumer_inventory_browser_read_status');
  const body = await response.json();
  const evidence = await invoke(ctx.fixture('browser'), 'evidence', page);
  const receipt = inventoryReceipt(ctx, evidence, response.headers()['x-request-id']);
  return {body, receipt};
}

export async function cookieGet(ctx, session, path, allowed = [200]) {
  const evidence = await invoke(ctx.fixture('browser'), 'evidence', session.page);
  if (evidence.authentication !== 'browser_session') throw new Blocked('consumer_inventory_cookie_session_required');
  const result = await session.page.evaluate(async ({path, timeout}) => {
    const response = await fetch(path, {method: 'GET', credentials: 'same-origin', cache: 'no-store',
      signal: AbortSignal.timeout(timeout)});
    return {status: response.status, requestId: response.headers.get('x-request-id'), body: await response.json()};
  }, {path, timeout: Math.max(1, Math.min(ctx.remainingMs?.() ?? 15000, 15000))});
  ctx.assert(allowed.includes(result.status), 'consumer_inventory_cookie_read_status');
  const receipt = inventoryReceipt(ctx, await invoke(ctx.fixture('browser'), 'evidence', session.page),
    result.requestId, result.status);
  return {...result, receipt};
}

export function assertInventoryPage(ctx, body, expected, seen, cursors) {
  ctx.assert(Array.isArray(body.items) && body.items.length <= 50
    && (body.items.length > 0 || body.next_after === null),
    'consumer_inventory_page_shape');
  for (const item of body.items) {
    ctx.assert(item.owner === expected.ownerSubject && expected.ownedBotIds.includes(item.bot_id)
      && item.bot_id !== expected.foreignBotId, 'consumer_inventory_owner_isolation');
    ctx.assert(!seen.has(item.bot_id), 'consumer_inventory_duplicate_bot'); seen.add(item.bot_id);
    ctx.assert(typeof item.name === 'string' && item.name && labels[item.state]
      && Number.isFinite(Date.parse(item.updated_at)), 'consumer_inventory_item_shape');
  }
  const cursor = body.next_after;
  ctx.assert(cursor === null || (typeof cursor === 'string' && cursor && !cursors.has(cursor)),
    'consumer_inventory_cursor_progress');
  if (cursor !== null) cursors.add(cursor);
}

async function assertTable(ctx, page, items) {
  const table = page.getByRole('table', {name: 'Your consumers', exact: true});
  if (items.length === 0) {
    await visible(ctx, table.getByText('No consumers on this page.', {exact: true}), 'consumer_inventory_empty_terminal_page');
    ctx.assert(await table.getByRole('row').count() === 2, 'consumer_inventory_empty_page_rows');
    return;
  }
  for (const item of items) {
    const row = table.getByRole('row').filter({has: page.locator(`a[href="/bots/${segment(item.bot_id)}"]`)});
    await visible(ctx, row.getByRole('link', {name: item.name, exact: true}), 'consumer_inventory_visible_row');
    const cells = row.getByRole('cell');
    ctx.assert((await cells.nth(1).innerText()) === labels[item.state], 'consumer_inventory_visible_state');
    ctx.assert((await cells.nth(2).innerText()) === (item.channel?.level ?? 'Not reported'), 'consumer_inventory_visible_level');
    ctx.assert((await cells.nth(3).innerText()) === (item.last_test === null ? 'Not tested'
      : `Accepted after ${item.last_test.accepted_after_ms} ms`), 'consumer_inventory_visible_last_test');
    ctx.assert(await row.locator('time').getAttribute('datetime') === item.updated_at, 'consumer_inventory_visible_timestamp');
  }
  ctx.assert(await table.getByRole('row').count() === items.length + 1, 'consumer_inventory_visible_exact_page');
}

async function assertDetail(ctx, page, value, expectedId, owner) {
  ctx.assert(value.bot_id === expectedId && value.owner === owner && labels[value.state], 'consumer_detail_identity_state');
  const detail = page.locator('section[aria-labelledby="bot-detail-title"]');
  await visible(ctx, detail.getByRole('heading', {name: value.name, exact: true}), 'consumer_detail_open');
  await visible(ctx, detail.getByText(labels[value.state], {exact: true}), 'consumer_detail_current_state');
  const fact = async (label, value) => {
    const actual = detail.locator('dt').filter({hasText: new RegExp('^' + label + '$')})
      .locator('xpath=following-sibling::dd[1]');
    await visible(ctx, actual, 'consumer_detail_fact_visible');
    ctx.assert((await actual.innerText()) === value, 'consumer_detail_fact_value');
  };
  await fact('Provider', value.provider === 'cma_codex' ? 'CMA' : value.provider);
  await fact('Provider level', value.channel?.level ?? 'Not reported');
  for (const [label, pin] of [['Sandbox profile', value.sandbox_profile], ['Workspace profile', value.workspace_profile]])
    await fact(label, pin ? `${pin.id} · Version ${pin.revision}` : 'Not reported');
  const expiry = value.credentials_expire_at === null ? 'Not reported'
    : await page.evaluate(stamp => new Date(stamp).toLocaleString(), value.credentials_expire_at);
  await fact('Access expiry', expiry);
  if (value.connection_ref) await visible(ctx, detail.getByRole('heading', {name: 'Event Gateway connection', exact: true}), 'consumer_detail_connection');
  else if (value.channel) {
    await visible(ctx, detail.getByRole('heading', {name: 'Callback URL', exact: true}), 'consumer_detail_callback');
    ctx.assert(await detail.locator('.bots__callback').innerText() === value.channel.callback_url, 'consumer_detail_callback_value');
  }
  if (value.failure !== null) await visible(ctx, detail.getByRole('alert').filter({hasText: value.failure}), 'consumer_detail_failure_explicit');
  if (value.last_test === null) await visible(ctx, detail.getByText('No test has been recorded.', {exact: true}), 'consumer_detail_untested');
  else {
    await visible(ctx, detail.getByText(`Accepted after ${value.last_test.accepted_after_ms} ms · Provider level ${value.last_test.level}`,
      {exact: true}), 'consumer_detail_test_timing');
    await visible(ctx, detail.getByText(value.last_test.first_text, {exact: true}), 'consumer_detail_test_text');
    ctx.assert(await detail.locator('time').getAttribute('datetime') === value.last_test.tested_at, 'consumer_detail_test_timestamp');
  }
}

export const consumerInventoryCase = define(47, 'browser', ['browser', 'user-fixtures'], async ctx => {
  let lease, expected, session, selected, firstPage;
  const seen = new Set(), cursors = new Set();
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'consumer_inventory'); expected = requireInventory(lease);
    session = await openBrowser(ctx, 'user', {initialPath: '/bots'});
    const identity = await cookieGet(ctx, session, '/auth/session');
    ctx.assert(identity.body.authenticated === true && identity.body.subject === expected.ownerSubject
      && identity.body.visitor !== true && identity.body.capabilities?.includes('manage_own_bots'), 'consumer_owner_cookie_identity');
    await visible(ctx, session.page.getByRole('heading', {name: 'Consumers', exact: true}), 'consumer_list_open');
    let after, pages = 0;
    const history = [];
    for (;;) {
      const result = await readUI(ctx, session, '/api/v1/bots', () => session.page.getByRole('button',
        {name: pages === 0 ? 'Refresh consumers' : 'Next page', exact: true}).click(), after);
      if (pages++ === 0) firstPage = result.body;
      history.push({body: result.body, after});
      assertInventoryPage(ctx, result.body, expected, seen, cursors);
      await assertTable(ctx, session.page, result.body.items);
      after = result.body.next_after;
      if (after === null) {
        ctx.assert(await session.page.getByRole('button', {name: 'Next page', exact: true}).isDisabled(), 'consumer_last_page_terminal');
        break;
      }
      ctx.assert(pages < 4, 'consumer_inventory_page_budget');
      await session.switchAM(peer(result.receipt.am));
    }
    ctx.assert(pages >= 2 && same([...seen].sort(), [...expected.ownedBotIds].sort()), 'consumer_inventory_complete_owned_set');
    const previous = history.at(-2);
    const back = await readUI(ctx, session, '/api/v1/bots', () => session.page
      .getByRole('button', {name: 'Previous page', exact: true}).click(), previous.after);
    ctx.assert(same(back.body, previous.body), 'consumer_inventory_previous_page');
    await assertTable(ctx, session.page, back.body.items); selected = back.body.items[0];
    const detail = await readUI(ctx, session, botPath(selected.bot_id), () => session.page
      .getByRole('table', {name: 'Your consumers', exact: true}).getByRole('link', {name: selected.name, exact: true}).click());
    await assertDetail(ctx, session.page, detail.body, selected.bot_id, expected.ownerSubject);
  });
  await ctx.step('step_2', async () => {
    for (const [id, kind] of [[lease.botId, 'active'], [expected.provisioningBotId, 'creating'],
      [expected.failedBotId, 'failed'], [expected.callbackBotId, 'callback']]) {
      const opened = await readUI(ctx, session, botPath(id), () => goto(session.page, '/bots/' + segment(id)));
      await session.switchAM(peer(opened.receipt.am));
      const refreshed = await readUI(ctx, session, botPath(id), () => session.page
        .getByRole('button', {name: 'Refresh status', exact: true}).click());
      ctx.assert(refreshed.receipt.am !== opened.receipt.am && same(refreshed.body, opened.body), 'consumer_detail_both_actual_ams');
      const value = refreshed.body;
      ctx.assert(kind === 'callback' ? !value.connection_ref && Boolean(value.channel?.callback_url)
        : value.state === kind, 'consumer_detail_seed_state');
      await assertDetail(ctx, session.page, value, id, expected.ownerSubject);
      if (kind === 'active') ctx.assert(value.connection_ref && value.last_test && value.operation === null, 'consumer_active_metadata_fixture');
      if (kind === 'failed') ctx.assert(typeof value.failure === 'string' && value.failure.length > 0, 'consumer_failure_fixture');
      if (kind === 'creating') {
        ctx.assert(value.connection_ref && value.operation?.operation === 'create' && value.operation.step === 'create_channel',
          'consumer_provisioning_fixture');
        const progress = session.page.getByRole('list', {name: 'Consumer setup progress', exact: true});
        await visible(ctx, progress, 'consumer_provisioning_progress');
        ctx.assert(same(await progress.getByRole('listitem').allTextContents(), ['Check Event Gateway connection · Done',
          'Prepare Agent access · Done', 'Connect Agent provider · In progress']), 'consumer_provisioning_steps');
        ctx.assert(await progress.locator('[aria-current="step"]').innerText() === 'Connect Agent provider · In progress',
          'consumer_provisioning_current_step');
      }
    }
    const choices = await readUI(ctx, session, '/api/v1/bots/connections', () => goto(session.page, '/bots/new'));
    await session.switchAM(peer(choices.receipt.am));
    const updated = await readUI(ctx, session, '/api/v1/bots/connections', () => session.page
      .getByRole('button', {name: 'Refresh Event Gateway connections', exact: true}).click());
    const ordered = value => [...value].sort((a, b) => a.connection_ref.localeCompare(b.connection_ref));
    ctx.assert(Array.isArray(updated.body) && Array.isArray(choices.body)
      && same(ordered(updated.body), ordered(expected.connections)) && same(ordered(updated.body), ordered(choices.body))
      && updated.receipt.am !== choices.receipt.am, 'consumer_connection_choices_actual_ams');
    await session.page.getByRole('button', {name: 'Event Gateway connection', exact: true}).click();
    const menu = session.page.getByRole('menu', {name: 'Event Gateway connection', exact: true});
    for (const item of updated.body) await visible(ctx, menu.getByRole('menuitemradio', {name: item.label, exact: true}), 'consumer_connection_choice_visible');
    ctx.assert(await menu.getByRole('menuitemradio').count() === updated.body.length + 1, 'consumer_connection_choices_exact');
    await session.page.keyboard.press('Escape');
    for (const am of ['a', 'b']) {
      await session.switchAM(am);
      const legacy = await cookieGet(ctx, session, '/api/v1/bots/nyxid-bots');
      ctx.assert(legacy.receipt.am === am && Array.isArray(legacy.body)
        && same([...legacy.body].sort((a, b) => a.id.localeCompare(b.id)),
          [...expected.legacyBots].sort((a, b) => a.id.localeCompare(b.id))), 'consumer_legacy_choices_owner_scoped');
      const foreign = await cookieGet(ctx, session, botPath(expected.foreignBotId), [403, 404]);
      ctx.assert(foreign.receipt.am === am && !foreign.body?.bot_id, 'consumer_foreign_detail_denied');
    }
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(firstPage.next_after !== null && seen.size === expected.ownedBotIds.length, 'consumer_inventory_paginated');
    const gated = await openBrowser(ctx, 'outsider', {initialPath: '/bots'});
    for (const am of ['a', 'b']) {
      await gated.switchAM(am);
      const identity = await cookieGet(ctx, gated, '/auth/session');
      ctx.assert(identity.receipt.am === am && identity.body.authenticated === true
        && identity.body.subject === expected.gatedSubject && identity.body.visitor !== true
        && Array.isArray(identity.body.capabilities) && !identity.body.capabilities.includes('manage_own_bots'),
      'consumer_management_missing_capability');
      for (const path of ['/bots', '/bots/' + segment(lease.botId), '/bots/new']) {
        await goto(gated.page, path);
        await visible(ctx, gated.page.getByRole('heading', {name: 'Bot management unavailable', exact: true}), 'consumer_management_gated');
        await visible(ctx, gated.page.getByText('Your account needs permission to manage its Bots.', {exact: true}), 'consumer_management_gate_reason');
        ctx.assert(await gated.page.getByRole('table', {name: 'Your consumers', exact: true}).count() === 0
          && await gated.page.getByRole('button', {name: 'Create consumer', exact: true}).count() === 0,
        'consumer_management_no_owned_content');
      }
      const refusal = await cookieGet(ctx, gated, '/api/v1/bots', [403]);
      ctx.assert(refusal.receipt.am === am && !refusal.body?.items, 'consumer_management_api_gate');
    }
    await uiReceipt(ctx, session.page); await uiReceipt(ctx, gated.page, 'consumer_gated_browser_receipt');
  });
});
