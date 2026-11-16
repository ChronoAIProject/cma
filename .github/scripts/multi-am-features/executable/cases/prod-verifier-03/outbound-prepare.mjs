import {Blocked} from '../../runner.mjs';
import {define, acquire, openBrowser, egRequest, invoke, visible, goto, peer, segment, same, digest, uiReceipt} from './helpers.mjs';
import {channelReceipt, openListedChannel} from './channel-browser.mjs';

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const text = value => typeof value === 'string' && value.length > 0;
const mediaKinds = new Set(['image', 'file', 'audio', 'video']);

export function validateOutboundFixture(lease) {
  const body = lease.outboundBody, message = body?.message, expected = lease.expected;
  const attachments = message?.native_attachments ?? [];
  if (!text(lease.channelId) || !body || body.source_id !== expected?.sourceId
    || !uuid.test(body.route_id) || body.route_id !== expected.routeId || !message
    || (message.text !== undefined && typeof message.text !== 'string')
    || (message.native_attachments !== undefined && !Array.isArray(message.native_attachments))
    || attachments.length > 4
    || !attachments.every(item => mediaKinds.has(item.kind) && item.source?.type === 'url'
      && /^https:\/\//.test(item.source.url) && !item.source.url.includes('#'))
    || !lease.keys || !/^[\x21-\x7e]{1,128}$/.test(lease.keys.prepare)
    || !text(expected.destination) || expected.destination !== body.route_id)
    throw new Blocked('outbound_saved_destination_fixture_required');
  for (const method of ['request', 'captureEffects'])
    if (typeof lease[method] !== 'function') throw new Blocked('outbound_primitive_' + method.toLowerCase());
  return {body, expected};
}

export function assertPreparedReceipt(ctx, receipt, body, key, expected) {
  ctx.assert(receipt?.status === 'prepared' && text(receipt.delivery_id)
    && receipt.route_id === body.route_id && receipt.source_id === body.source_id
    && receipt.delivery == null, 'outbound_prepared_receipt');
  ctx.assert(Number.isSafeInteger(receipt.expires_at) && Number.isSafeInteger(receipt.retained_until)
    && receipt.retained_until >= receipt.expires_at, 'outbound_retention_window');
  ctx.assert(text(key) && expected.destination === receipt.route_id, 'outbound_destination_identity');
  ctx.assert(receipt.delivery?.outcome !== 'confirmed' && receipt.delivery?.outcome !== 'delivered',
    'outbound_prepare_not_delivered');
}

async function guardPrepare(ctx, session, channelId, body, key) {
  const url = new URL(`/api/event-gateway/v1/channels/${segment(channelId)}/outbound-messages`, session.origin).href;
  let result, attempts = 0, failure;
  const pending = new Set();
  const handler = route => {
    const operation = (async () => {
      try {
        const request = route.request();
        ctx.assert(++attempts === 1 && request.method() === 'POST' && request.url() === url,
          'outbound_prepare_single_post');
        ctx.assert(same(request.postDataJSON(), body), 'outbound_prepare_exact_body');
        const headers = await request.allHeaders();
        ctx.assert(headers['idempotency-key'] === key, 'outbound_prepare_exact_key');
        const response = await route.fetch({maxRetries: 0, maxRedirects: 0,
          timeout: Math.max(1, ctx.remainingMs?.() ?? 30000)});
        ctx.assert(response.status() === 201, 'outbound_prepare_http_status');
        const value = await response.json();
        assertPreparedReceipt(ctx, value, body, key, {destination: body.route_id});
        result = {value, requestId: response.headers()['x-request-id']};
        ctx.assert(text(result.requestId), 'outbound_prepare_request_id');
        await route.fulfill({response});
      } catch (error) {failure ??= error; try {await route.abort('failed');} catch {} }
    })();
    pending.add(operation); return operation.finally(() => pending.delete(operation));
  };
  const close = async () => {
    const errors = [];
    try {await pageUnroute(session.page, url, handler);} catch (error) {errors.push(error);}
    await Promise.allSettled([...pending]);
    if (failure) errors.push(failure);
    if (attempts !== 1) errors.push(Error('outbound_prepare_missing_or_duplicate_post'));
    if (errors.length) throw new AggregateError(errors, 'outbound_prepare_guard_cleanup_failed');
  };
  await session.page.route(url, handler);
  ctx.defer('remove_outbound_prepare_guard', close);
  return {url, result: () => result, check: () => {if (failure) throw failure;}, close};
}

async function pageUnroute(page, url, handler) {await page.unroute(url, handler);}

async function fillMedia(page, message) {
  const items = message.native_attachments ?? [];
  const add = page.getByRole('button', {name: 'Add media attachment', exact: true});
  if (!items.length) return;
  if (await add.count() !== 1) throw new Blocked('outbound_media_capability_not_bound');
  for (const [index, item] of items.entries()) {
    await add.click();
    await page.getByLabel(`HTTPS URL ${index + 1}`, {exact: true}).fill(item.source.url);
    const picker = page.getByRole('button', {name: `Media type ${index + 1}`, exact: true});
    await picker.click();
    await page.getByRole('menuitemradio', {name: item.kind[0].toUpperCase() + item.kind.slice(1), exact: true}).click();
    if (item.filename) await page.getByLabel(`Filename ${index + 1} (optional)`, {exact: true}).fill(item.filename);
    if (item.mime_type) await page.getByLabel(`MIME type ${index + 1} (optional)`, {exact: true}).fill(item.mime_type);
    if (item.caption) await page.getByLabel(`Caption ${index + 1} (optional)`, {exact: true}).fill(item.caption);
  }
}

async function fillOutbound(page, body) {
  const destination = page.getByRole('button', {name: 'Saved destination', exact: true});
  await destination.click();
  await page.getByRole('menuitemradio', {name: body.route_id, exact: true}).click();
  if (body.message.text) await page.getByLabel('Message text', {exact: true}).fill(body.message.text);
  await fillMedia(page, body.message);
}

export const outboundPrepareCase = define(29, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let lease, body, expected, session, guard, before, receipt, read;
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'outbound_prepare'); ({body, expected} = validateOutboundFixture(lease));
    before = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}`});
    ctx.assert(before.channel_id === lease.channelId, 'outbound_channel_identity');
    session = await openBrowser(ctx, 'user', {outboundPrepare: true});
    for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
      if (typeof session[method] !== 'function') throw new Blocked('outbound_actual_eg_browser_required');
    const selected = await openListedChannel(ctx, session, before, () => goto(session.page, '/event-gateway'));
    ctx.assert(selected.body.channel_id === lease.channelId, 'outbound_selected_channel');
    await session.page.getByRole('button', {name: 'Send message', exact: true}).click();
    const composer = session.page.getByRole('region', {name: 'Send to saved destination', exact: true});
    await visible(ctx, composer, 'outbound_composer_visible');
    guard = await guardPrepare(ctx, session, lease.channelId, body, lease.keys.prepare);
    await fillOutbound(composer, body);
    await composer.getByRole('button', {name: 'Review message', exact: true}).click();
    await visible(ctx, composer.getByRole('button', {name: 'Send message', exact: true}), 'outbound_prepared_ui');
    const result = guard.result();
    ctx.assert(result && result.value.delivery_id, 'outbound_browser_receipt_identity');
    const evidence = await invoke(session, 'egEvidence');
    const actual = channelReceipt(ctx, evidence, result.requestId, 'POST', 201);
    await ctx.observe('outbound_prepare_eg_receipt', async () => ({schema: 'multi-eg-request-v1', receipt: actual}));
    receipt = result.value;
    assertPreparedReceipt(ctx, receipt, body, lease.keys.prepare, expected);
  });
  await ctx.step('step_2', async () => {
    read = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}/outbound-messages/${segment(receipt.delivery_id)}`,
      replica: peer(ctx.am)});
    assertPreparedReceipt(ctx, read, body, lease.keys.prepare, expected);
    ctx.assert(same(read, receipt), 'outbound_peer_receipt_exact');
    const effects = await invoke(lease, 'captureEffects', {deliveryId: receipt.delivery_id});
    ctx.assert(Array.isArray(effects) && effects.length === 0, 'outbound_prepare_no_delivery_effect');
    ctx.assert(!JSON.stringify(read).includes(body.message.text ?? '__no_text__'), 'outbound_receipt_no_message_body');
  });
  await ctx.step('expected_result', async () => {
    const durable = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}/outbound-messages/${segment(receipt.delivery_id)}`});
    assertPreparedReceipt(ctx, durable, body, lease.keys.prepare, expected);
    ctx.assert(durable.delivery_id === receipt.delivery_id && durable.status === 'prepared',
      'outbound_durable_preparation_state');
    await uiReceipt(ctx, session.page);
  });
});
