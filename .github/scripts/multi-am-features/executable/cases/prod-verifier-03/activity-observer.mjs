import {randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {same} from './helpers.mjs';

const requireValue = (condition, code) => {if (!condition) throw Error(code);};
const text = value => typeof value === 'string' && value.length > 0;
const header = (headers, name) => Object.entries(headers ?? {})
  .find(([key]) => key.toLowerCase() === name)?.[1];
const states = {received: 'Received', queued: 'Queued', dispatching: 'Dispatching to provider',
  accepted: 'Accepted by agent', produced: 'Response produced', completed: 'Agent completed',
  refused: 'Refused', failed: 'Agent response failed', unknown: 'Outcome unknown'};

export function assertActivityPage(ctx, page, expectedIds, forbidden = []) {
  ctx.assert(Array.isArray(page?.items) && text(page.cursor) && page.cursor.length <= 1024
    && Number.isSafeInteger(page.retention_days) && page.retention_days > 0
    && Number.isSafeInteger(page.limit) && page.limit >= page.items.length
    && typeof page.has_more === 'boolean', 'activity_complete_snapshot_required');
  const ids = page.items.map(item => item.event_id);
  ctx.assert(ids.every(text) && new Set(ids).size === ids.length, 'activity_duplicate_event');
  if (expectedIds) ctx.assert(same([...ids].sort(), [...expectedIds].sort()) && !page.has_more,
    'activity_retained_identity_set');
  for (const item of page.items) {
    ctx.assert(text(item.conversation?.id) && item.source && item.actor
      && Number.isFinite(Date.parse(item.received_at)) && Number.isFinite(Date.parse(item.occurred_at))
      && Array.isArray(item.consumers), 'activity_event_metadata');
    const consumers = item.consumers.map(c => c.consumer_id);
    ctx.assert(consumers.every(text) && new Set(consumers).size === consumers.length, 'activity_duplicate_consumer');
    for (const consumer of item.consumers) ctx.assert(Object.hasOwn(states, consumer.state)
      && Array.isArray(consumer.deliveries) && consumer.deliveries.every(d => text(d.step)
        && text(d.outcome) && Number.isFinite(Date.parse(d.observed_at))), 'activity_consumer_metadata');
  }
  const serialized = JSON.stringify(page);
  ctx.assert(forbidden.every(marker => !serialized.includes(marker)), 'activity_unrelated_transcript_in_snapshot');
}

// Use the server's actual record and versions. Only the day changes, producing
// a well-formed expired cursor rather than exercising the malformed-cursor 400.
export function expiredActivityCursor(page) {
  requireValue(text(page?.cursor) && /^[A-Za-z0-9_-]+$/.test(page.cursor)
    && page.cursor.length <= 1024, 'activity_cursor_encoding');
  let value;
  try {value = JSON.parse(Buffer.from(page.cursor, 'base64url').toString('utf8'));}
  catch {throw Error('activity_cursor_encoding');}
  requireValue(text(value?.record) && Number.isSafeInteger(value.day)
    && Number.isSafeInteger(page.retention_days) && page.retention_days > 0
    && Array.isArray(value.versions) && value.versions.length === page.retention_days,
  'activity_cursor_shape');
  return Buffer.from(JSON.stringify({...value, day: value.day - page.retention_days})).toString('base64url');
}

const timestamp = value => value && Number.isFinite(Date.parse(value)) ? value : null;
const deliveryLabel = d => {
  if (['platform_status', 'platform_failure'].includes(d.step)) {
    const label = {sent: 'Sent by WhatsApp', delivered: 'Delivered to recipient', read: 'Read by recipient',
      played: 'Voice message played', failed: 'WhatsApp could not deliver this message'}[d.outcome];
    if (label) return label;
  }
  if (d.outcome === 'confirmed') return d.recipients?.some(r => r.kind.startsWith('whatsapp'))
    ? 'Accepted by WhatsApp' : 'Delivery confirmed';
  return {buffered: 'Buffered locally', unavailable: 'Delivery unavailable'}[d.outcome]
    ?? d.outcome.replaceAll('_', ' ');
};

// Project the independently received snapshot into values visible in the full
// inspector. Locale-formatted time text is compared through its datetime value.
export function activityProjection(page) {
  return page.items.map(item => ({eventId: item.event_id, conversation: item.conversation.id,
    receivedAt: timestamp(item.received_at), eventTime: timestamp(item.kind === 'event'
      ? item.source_occurred_at : item.occurred_at),
    consumers: item.consumers.map(c => ({name: c.display_name ?? c.consumer_id, state: states[c.state],
      dispatchedAt: timestamp(c.dispatched_at), acceptedAt: timestamp(c.accepted_at),
      agent: c.agent_id ?? 'Not reported', provider: c.provider ?? null,
      conversation: c.conversation_id ?? null, producedAt: timestamp(c.produced_at),
      deliveries: c.deliveries.map(d => ({description: d.step.replaceAll('_', ' ') + ' · '
        + (d.recipients?.length ? d.recipients.map(r => `${r.kind}: ${r.id}${r.thread_id ? ` · Thread ${r.thread_id}` : ''}`)
          .join(', ') : 'Recipient not reported'), label: deliveryLabel(d),
      time: timestamp(d.outcome === 'confirmed' || d.step.startsWith('platform_') ? d.confirmed_at : d.observed_at)}))}))}));
}

export function assertAtomicActivity(ctx, observations, frames) {
  ctx.assert(observations.length > 0 && frames.length > 0, 'activity_dom_observation_required');
  const projections = frames.map(frame => activityProjection(frame.page));
  let position = 0;
  for (const observation of observations) {
    ctx.assert(!observation.leak, 'activity_unrelated_transcript_in_dom');
    const ids = observation.rows.map(row => row.eventId);
    ctx.assert(new Set(ids).size === ids.length, 'activity_duplicate_rendered_event');
    const next = projections.findIndex((rows, index) => index >= position && same(rows, observation.rows));
    ctx.assert(next >= 0, 'activity_non_atomic_or_regressed_snapshot');
    position = next;
  }
}

export async function activityWait(ctx, read, code, milliseconds = 15000) {
  const deadline = performance.now() + Math.min(milliseconds, ctx.remainingMs?.() ?? milliseconds);
  for (;;) {
    ctx.assert(!ctx.signal?.aborted, 'activity_cancelled');
    const value = await read();
    if (value) return value;
    ctx.assert(performance.now() < deadline, code);
    await new Promise(resolve => setTimeout(resolve, 25));
  }
}

// Observe native EventSource, without replacing it or injecting server frames.
// Only exact target requests and cursor headers are retained; auth headers never
// enter this record. Listener failures are sticky and also checked at cleanup.
export async function observeActivityStream(ctx, page, url, validate) {
  if (typeof page.context?.().newCDPSession !== 'function') throw new Blocked('activity_existing_chromium_cdp_required');
  const cdp = await page.context().newCDPSession(page);
  const connections = [], frames = [], current = new Map(), failures = [], extra = new Map();
  let bytes = 0, stopped = false, closing;
  const check = () => {if (failures.length) throw failures[0];};
  const listen = operation => event => {
    try {operation(event);} catch {failures.push(Error('activity_native_stream_observation_failed'));}
  };
  const handlers = {
    'Network.requestWillBeSent': listen(event => {
      if (event.request.url !== url) return;
      requireValue(connections.length < 32 && event.request.method === 'GET'
        && !event.redirectResponse, 'activity_stream_request');
      const connection = {index: connections.length, cdpId: event.requestId,
        lastEventId: extra.get(event.requestId) ?? header(event.request.headers, 'last-event-id') ?? null};
      extra.delete(event.requestId); connections.push(connection); current.set(event.requestId, connection);
    }),
    'Network.requestWillBeSentExtraInfo': listen(event => {
      const cursor = header(event.headers, 'last-event-id');
      if (cursor === undefined) return;
      const connection = current.get(event.requestId);
      if (connection) connection.lastEventId = cursor;
      else {requireValue(extra.size < 128, 'activity_header_buffer'); extra.set(event.requestId, cursor);}
    }),
    'Network.responseReceived': listen(event => {
      const connection = current.get(event.requestId); if (!connection) return;
      requireValue(event.response.url === url && event.response.status === 200
        && (header(event.response.headers, 'content-type') ?? event.response.mimeType ?? '')
          .toLowerCase().startsWith('text/event-stream'), 'activity_stream_response');
      const requestId = header(event.response.headers, 'x-request-id');
      requireValue(text(requestId), 'activity_stream_request_id');
      Object.assign(connection, {requestId, status: event.response.status});
    }),
    'Network.eventSourceMessageReceived': listen(event => {
      const connection = current.get(event.requestId); if (!connection) return;
      bytes += Buffer.byteLength(event.data);
      requireValue(frames.length < 256 && bytes <= 8 * 1024 * 1024
        && ['activity', 'reset'].includes(event.eventName), 'activity_stream_frame');
      let snapshot; try {snapshot = JSON.parse(event.data);} catch {throw Error('activity_stream_json');}
      validate(snapshot);
      requireValue(event.eventId === snapshot.cursor, 'activity_stream_cursor_join');
      frames.push({index: frames.length, connection, type: event.eventName, page: snapshot});
    }),
  };
  const close = () => closing ??= (async () => {
    stopped = true;
    const errors = [];
    for (const [name, handler] of Object.entries(handlers)) cdp.off(name, handler);
    try {await cdp.detach();} catch (error) {errors.push(error);}
    errors.push(...failures);
    if (errors.length) throw new AggregateError(errors, 'activity_stream_cleanup_failed');
  })();
  ctx.defer('detach_activity_cdp', close);
  for (const [name, handler] of Object.entries(handlers)) cdp.on(name, handler);
  await cdp.send('Network.enable');
  return {frames, connections, check, close, wait: (predicate, code) => activityWait(ctx, () => {
    check(); ctx.assert(!stopped, 'activity_observer_stopped');
    return frames.find(frame => frame.connection.requestId && predicate(frame));
  }, code)};
}

// Executed in the existing page. The observer only reads the rendered tree and
// keeps bounded projections in memory; it never changes DOM or product state.
export function installActivityDomObserver(root, {key, forbidden}) {
  const snapshots = [];
  // Keep this function self-contained: Playwright serializes its source into
  // the page and does not carry module lexical bindings with it.
  const readValue = (node, selector) => node.querySelector(selector)?.textContent?.trim() ?? null;
  const readTime = (node, selector) => node.querySelector(selector)?.getAttribute('datetime') ?? null;
  const readFields = node => Object.fromEntries([...node.querySelectorAll(':scope > dl > div')].map(div => [
    readValue(div, 'dt'), {value: readValue(div, 'dd'), time: readTime(div, 'dd time')} ]));
  let overflow = false;
  const capture = () => {
    if (snapshots.length >= 512) {overflow = true; return;}
    const rows = [...root.querySelectorAll(':scope > li > article.channel-activity__event')].map(article => {
      const f = readFields(article);
      return {eventId: readValue(article, ':scope > p > code'), conversation: f.Conversation?.value ?? null,
        receivedAt: readTime(article, ':scope > header > span time'),
        eventTime: (f['Source event time'] ?? f['Recorded event time'])?.time ?? null,
        consumers: [...article.querySelectorAll(':scope > .channel-activity__consumer')].map(consumer => {
          const d = readFields(consumer);
          return {name: readValue(consumer, ':scope > header > strong'), state: readValue(consumer, ':scope > header > span'),
            dispatchedAt: d['Dispatch started']?.time ?? null, acceptedAt: d['Accepted by agent']?.time ?? null,
            agent: d['Produced by']?.value ?? null, provider: d.Provider?.value ?? null,
            conversation: d['Agent conversation']?.value ?? null, producedAt: d['Response produced']?.time ?? null,
            deliveries: [...consumer.querySelectorAll(':scope > ul > li')].map(li => ({
              description: readValue(li, ':scope > span'), label: readValue(li, ':scope > strong'), time: readTime(li, ':scope > time')}))};
        })};
    });
    const next = {rows, leak: forbidden.some(marker => root.textContent.includes(marker))};
    if (JSON.stringify(next) !== JSON.stringify(snapshots.at(-1))) snapshots.push(next);
  };
  const observer = new MutationObserver(capture);
  observer.observe(root, {subtree: true, childList: true, characterData: true, attributes: true});
  capture();
  globalThis[key] = {read: () => {capture(); return {snapshots, overflow, connected: root.isConnected};},
    close: () => observer.disconnect()};
}

export async function observeActivityDom(ctx, locator, page, forbidden) {
  const key = '__c275_activity_' + randomUUID().replaceAll('-', '');
  // Custody precedes evaluation, including an installation whose reply is lost.
  ctx.defer('remove_activity_dom_observer', () => page.evaluate(key => {
    globalThis[key]?.close(); delete globalThis[key];
  }, key));
  await locator.evaluate(installActivityDomObserver, {key, forbidden});
  return {read: async () => {
    const value = await page.evaluate(key => globalThis[key]?.read(), key);
    ctx.assert(value && !value.overflow && value.connected, 'activity_dom_observer_missing_or_overflow');
    return value.snapshots;
  }};
}

// One request-header fault only. No fetch/fulfill of an infinite SSE body, no
// response rewrite and no retry if dispatch has an unknown outcome.
export async function overrideNextActivityCursor(ctx, page, url, cursor) {
  let attempted = 0, observedOriginal, failure;
  const pending = new Set();
  const handler = route => {
    const operation = (async () => {
      try {
        const request = route.request();
        ctx.assert(++attempted === 1 && request.method() === 'GET', 'activity_cursor_override_once');
        const headers = await request.allHeaders();
        observedOriginal = header(headers, 'last-event-id') ?? null;
        for (const name of Object.keys(headers)) if (name.toLowerCase() === 'last-event-id') delete headers[name];
        ctx.assert(!ctx.signal?.aborted, 'activity_cancelled');
        await route.continue({headers: {...headers, 'last-event-id': cursor}});
      } catch (error) {
        failure ??= error;
        try {await route.abort('failed');} catch {failure ??= Error('activity_cursor_abort_failed');}
      }
    })();
    pending.add(operation); return operation.finally(() => pending.delete(operation));
  };
  const close = async () => {
    const errors = [];
    try {await page.unroute(url, handler);} catch (error) {errors.push(error);}
    await Promise.allSettled([...pending]);
    if (failure) errors.push(failure);
    if (errors.length) throw new AggregateError(errors, 'activity_cursor_override_cleanup_failed');
  };
  ctx.defer('remove_activity_cursor_override', close);
  await page.route(url, handler);
  return {check: expected => {
    if (failure) throw failure;
    ctx.assert(attempted === 1 && observedOriginal === expected, 'activity_cursor_override_native_original');
  }, close};
}
