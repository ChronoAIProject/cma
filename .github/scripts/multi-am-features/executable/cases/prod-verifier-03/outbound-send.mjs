import {Blocked} from '../../runner.mjs';
import {define, acquire, openBrowser, egRequest, invoke, visible, goto, peer, segment, same, uiReceipt} from './helpers.mjs';
import {channelReceipt, openListedChannel} from './channel-browser.mjs';

const text = value => typeof value === 'string' && value.length > 0;
const keyPattern = /^[\x21-\x7e]{1,128}$/;

export function validateOutboundSendFixture(lease) {
  const body = lease.outboundBody, expected = lease.expected, prepared = expected?.prepared;
  if (!text(lease.channelId) || !body?.source_id || !body.route_id || !body.message
    || !text(expected?.ownerSubject) || !prepared?.send?.delivery_id || !prepared?.cancel?.delivery_id
    || !prepared?.uncertain?.delivery_id || new Set([prepared.send.delivery_id, prepared.cancel.delivery_id,
      prepared.uncertain.delivery_id]).size !== 3 || !lease.keys || !keyPattern.test(lease.keys.send)
    || !keyPattern.test(lease.keys.uncertain) || !keyPattern.test(lease.keys.cancel)
    || !Array.isArray(expected.sendEffects) || !Array.isArray(expected.cancelEffects)
    || !Array.isArray(expected.uncertainEffects)) throw new Blocked('outbound_send_prepared_fixture_required');
  for (const method of ['request', 'dropOutboundSendResponse', 'outboundStatus', 'captureEffects', 'drain'])
    if (typeof lease[method] !== 'function') throw new Blocked('outbound_primitive_' + method.toLowerCase());
  return {body, expected, prepared};
}

export function assertCommittedReceipt(ctx, receipt, deliveryId, body, outcome = 'confirmed') {
  ctx.assert(receipt?.delivery_id === deliveryId && receipt.status === 'committed'
    && receipt.route_id === body.route_id && receipt.delivery?.outcome === outcome,
  'outbound_committed_receipt_identity');
}

export function assertCancelledReceipt(ctx, receipt, deliveryId, body) {
  ctx.assert(receipt?.delivery_id === deliveryId && receipt.status === 'cancelled'
    && receipt.route_id === body.route_id && receipt.delivery == null, 'outbound_cancelled_receipt_identity');
}

async function guardOperation(ctx, session, url, method, expectedBody, status, validate) {
  let attempts = 0, result, failure; const pending = new Set();
  const handler = route => {
    const operation = (async () => {
      try {
        const request = route.request();
        ctx.assert(++attempts === 1 && request.method() === method && request.url() === url,
          'outbound_operation_single_request');
        if (expectedBody !== undefined) ctx.assert(same(request.postDataJSON(), expectedBody), 'outbound_send_exact_body');
        const response = await route.fetch({maxRetries: 0, maxRedirects: 0,
          timeout: Math.max(1, ctx.remainingMs?.() ?? 30000)});
        ctx.assert(response.status() === status, 'outbound_operation_http_status');
        const value = await response.json(); validate(value);
        const requestId = response.headers()['x-request-id'];
        ctx.assert(text(requestId), 'outbound_operation_request_id');
        result = {value, requestId}; await route.fulfill({response});
      } catch (error) {failure ??= error; try {await route.abort('failed');} catch {} }
    })();
    pending.add(operation); return operation.finally(() => pending.delete(operation));
  };
  const close = async () => {
    const errors = [];
    try {await session.page.unroute(url, handler);} catch (error) {errors.push(error);}
    await Promise.allSettled([...pending]);
    if (failure) errors.push(failure);
    if (attempts !== 1) errors.push(Error('outbound_operation_missing_or_duplicate_request'));
    if (errors.length) throw new AggregateError(errors, 'outbound_operation_guard_cleanup_failed');
  };
  await session.page.route(url, handler); ctx.defer('remove_outbound_operation_guard', close);
  return {result: () => result, check: () => {if (failure) throw failure;}, close};
}

async function seedJournal(page, owner, recordId, draft, receipt, uncertain = false) {
  await page.evaluate(({owner, recordId, draft, receipt, uncertain}) => {
    const key = `cma:channel-outbound:${JSON.stringify([location.origin, owner, recordId])}`;
    sessionStorage.setItem(key, JSON.stringify({version: draft.body.message.native_attachments?.length ? 2 : 1,
      intent: {draft, receipt, uncertain}}));
  }, {owner, recordId, draft, receipt, uncertain});
}

async function composer(ctx, session, channel) {
  await openListedChannel(ctx, session, channel, () => goto(session.page, '/event-gateway'));
  await session.page.getByRole('button', {name: 'Send message', exact: true}).click();
  const value = session.page.getByRole('region', {name: 'Send to saved destination', exact: true});
  await visible(ctx, value, 'outbound_composer_visible'); return value;
}

async function browserReceipt(ctx, session, result, method, status) {
  ctx.assert(result?.value && text(result.requestId), 'outbound_browser_result');
  const evidence = await invoke(session, 'egEvidence');
  const receipt = channelReceipt(ctx, evidence, result.requestId, method, status);
  await ctx.observe('outbound_send_eg_receipt', async () => ({schema: 'multi-eg-request-v1', receipt}));
  return receipt;
}

export const outboundSendCase = {...define(30, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let lease, body, expected, prepared, session, channel, sendReceipt, cancelReceipt, uncertain;
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'outbound_send'); ({body, expected, prepared} = validateOutboundSendFixture(lease));
    channel = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}`});
    ctx.assert(channel.channel_id === lease.channelId, 'outbound_send_channel_identity');
    session = await openBrowser(ctx, 'user', {outboundSend: true});
    for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
      if (typeof session[method] !== 'function') throw new Blocked('outbound_send_actual_eg_browser_required');
    const draft = {key: lease.keys.send, body};
    await seedJournal(session.page, expected.ownerSubject, channel.record_id, draft, prepared.send);
    const sendUrl = new URL(`/api/event-gateway/v1/channels/${segment(lease.channelId)}/outbound-messages/${segment(prepared.send.delivery_id)}/send`, session.origin).href;
    const sendGuard = await guardOperation(ctx, session, sendUrl, 'POST', body.message, 200,
      value => assertCommittedReceipt(ctx, value, prepared.send.delivery_id, body));
    const value = await composer(ctx, session, channel);
    await value.getByRole('button', {name: 'Send message', exact: true}).click();
    await visible(ctx, value.getByText('The platform accepted the message.', {exact: true}), 'outbound_send_ui_confirmed');
    sendReceipt = sendGuard.result()?.value;
    await browserReceipt(ctx, session, sendGuard.result(), 'POST', 200); sendGuard.check();
    assertCommittedReceipt(ctx, sendReceipt, prepared.send.delivery_id, body);
    const cancelDraft = {key: lease.keys.cancel, body};
    await seedJournal(session.page, expected.ownerSubject, channel.record_id, cancelDraft, prepared.cancel);
    await session.page.reload(); await session.networkIdle();
    const cancelUrl = new URL(`/api/event-gateway/v1/channels/${segment(lease.channelId)}/outbound-messages/${segment(prepared.cancel.delivery_id)}`, session.origin).href;
    const cancelGuard = await guardOperation(ctx, session, cancelUrl, 'DELETE', undefined, 200,
      value => assertCancelledReceipt(ctx, value, prepared.cancel.delivery_id, body));
    const cancelComposer = await composer(ctx, session, channel);
    await cancelComposer.getByRole('button', {name: 'Cancel', exact: true}).click();
    await visible(ctx, cancelComposer.getByText('Cancelled before sending.', {exact: true}), 'outbound_cancel_ui_confirmed');
    cancelReceipt = cancelGuard.result()?.value;
    await browserReceipt(ctx, session, cancelGuard.result(), 'DELETE', 200); cancelGuard.check();
    assertCancelledReceipt(ctx, cancelReceipt, prepared.cancel.delivery_id, body);
  });
  await ctx.step('step_2', async () => {
    const committed = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}/outbound-messages/${segment(prepared.send.delivery_id)}`, replica: peer(ctx.am)});
    assertCommittedReceipt(ctx, committed, prepared.send.delivery_id, body);
    const cancelled = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}/outbound-messages/${segment(prepared.cancel.delivery_id)}`, replica: peer(ctx.am)});
    assertCancelledReceipt(ctx, cancelled, prepared.cancel.delivery_id, body);
    uncertain = await invoke(lease, 'dropOutboundSendResponse', {deliveryId: prepared.uncertain.delivery_id,
      body: body.message, key: lease.keys.uncertain});
    ctx.assert(uncertain?.outcome === 'unknown' && uncertain?.sendAttempts === 1, 'outbound_unknown_send_explicit');
    const recovered = await invoke(lease, 'outboundStatus', {deliveryId: prepared.uncertain.delivery_id});
    ctx.assert(recovered.outcome === 'confirmed' && recovered.sendAttempts === 1,
      'outbound_unknown_readback_without_resend');
    const effects = await invoke(lease, 'captureEffects', {deliveryId: prepared.uncertain.delivery_id});
    ctx.assert(Array.isArray(effects) && effects.length === 1, 'outbound_uncertain_one_effect');
  });
  await ctx.step('expected_result', async () => {
    const final = await invoke(lease, 'drain', {timeoutMs: Math.max(1, ctx.remainingMs?.() ?? 30000)});
    ctx.assert(final.pending === 0 && final.unknown === 0 && final.sendAttempts === 2,
      'outbound_send_finality_no_duplicate');
    await uiReceipt(ctx, session.page);
  });
}), evidence_scope: 'eg_only'};
