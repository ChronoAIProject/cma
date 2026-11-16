import {randomUUID} from 'node:crypto';
import {define, acquire, openBrowser, visible, uiReceipt, egRequest, invoke, peer, segment,
  pages, same, digest, goto} from './helpers.mjs';
import {Blocked} from '../../runner.mjs';
import {configureChannelForm} from './channel-creation.mjs';
import {channelReceipt, openListedChannel} from './channel-browser.mjs';

const connection = ref => `/v1/channel-connections/${segment(ref)}`;
const channel = channelId => `/v1/channels/${segment(channelId)}`;
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

const pickLabel = (label, value) => label === 'Connection admission policy'
  ? ({all: 'All messages', allowlist: 'Allowed senders', scoped: 'Sender, chat and group rules'}[value]
    ?? String(value).replaceAll('_', ' '))
  : label === 'Messages from bots' && value === 'native_mentions'
    ? 'Verified mentions only' : String(value).replaceAll('_', ' ');

async function choose(page, label, value) {
  await page.getByRole('button', {name: label, exact: true}).click();
  await page.getByRole('menuitemradio', {name: pickLabel(label, value), exact: true}).click();
}

async function fillList(page, label, values) {
  const current = Array.isArray(values) ? values : [];
  for (let index = 0; index < current.length; index++) {
    await page.getByRole('button', {name: `Add ${label}`, exact: true}).click();
    await page.getByLabel(`${label} ${index + 1}`, {exact: true}).fill(String(current[index]));
  }
}

async function fillIdentity(page, label, value) {
  if (!value || typeof value !== 'object') return;
  await choose(page, `${label} policy`, value.type);
  if (value.type === 'allowlist') await fillList(page, `${label} IDs`, value.ids);
}

async function fillConnectionAdmission(page, admission) {
  if (!admission || typeof admission !== 'object' || typeof admission.type !== 'string')
    throw new Blocked('managed_connection_admission_fixture_required');
  await choose(page, 'Connection admission policy', admission.type);
  if (admission.type === 'allowlist') await fillList(page, 'Allowed sender IDs', admission.sender_ids);
  if (admission.type === 'scoped') {
    await fillIdentity(page, 'Senders', admission.senders);
    await fillIdentity(page, 'Chats', admission.chats);
    if (admission.groups !== undefined) await choose(page, 'Group messages', admission.groups);
    if (admission.bot_messages !== undefined) await choose(page, 'Messages from bots', admission.bot_messages);
  }
}

function hasKey(value, key) {
  if (Array.isArray(value)) return value.some(item => hasKey(item, key));
  if (!value || typeof value !== 'object') return false;
  return Object.entries(value).some(([name, child]) => name === key || hasKey(child, key));
}

export function assertManagedConnectionIntent(ctx, body, key, expected, desired, credentialHashes) {
  ctx.assert(uuid.test(body?.record_id ?? '') && typeof key === 'string' && uuid.test(key)
    && key !== body.record_id, 'managed_connection_generated_record_and_key');
  const {record_id, credentials, ...configuration} = body;
  ctx.assert(same(configuration, desired), 'managed_connection_exact_ui_configuration');
  ctx.assert(configuration.connection_ref === expected.connectionRef
    && same(configuration.connection_admission, expected.connectionAdmission)
    && Array.isArray(configuration.sources) && configuration.sources.length === 0,
  'managed_connection_exact_reference_admission');
  ctx.assert(!hasKey(configuration, 'route_id') && !hasKey(configuration, 'route_ids'),
    'managed_connection_no_nyxid_route_substitution');
  ctx.assert(credentials && same(Object.keys(credentials), ['agent_key'])
    && typeof credentials.agent_key === 'string' && credentials.agent_key.length > 0
    && `sha256:${digest(credentials.agent_key)}` === credentialHashes.agent,
  'managed_connection_provider_credential_hash');
}

export function assertManagedChannel(ctx, record, intent, expected) {
  const definition = record?.definition;
  ctx.assert(record?.record_id === intent.recordId && typeof record.channel_id === 'string'
    && record.channel_id.length > 0 && Number.isSafeInteger(record.version) && record.version > 0,
  'managed_connection_created_identity');
  ctx.assert(definition?.connection_ref === expected.connectionRef
    && definition.profile?.metadata?.owner?.subject === expected.ownerSubject,
  'managed_connection_created_owner_reference');
  ctx.assert(Array.isArray(definition.sources) && definition.sources.length === 1
    && definition.sources[0].type === 'telegram'
    && same(definition.sources[0].admission, expected.connectionAdmission),
  'managed_connection_created_managed_source');
  ctx.assert(!hasKey(definition, 'route_id') && !hasKey(definition, 'route_ids')
    && !Object.hasOwn(definition, 'credentials') && !Object.hasOwn(record, 'credentials'),
  'managed_connection_created_no_route_or_private_values');
}

function assertRefusal(ctx, response, detail, code) {
  const body = response?.body ?? {};
  const serialized = JSON.stringify(body);
  ctx.assert(response?.status === 400 && (body.code === 'invalid_request'
    || String(body.type ?? '').includes('invalid_request') || body.status === 400), code + '_status');
  ctx.assert(body.detail === `/connection_ref: ${detail}` || body.detail === detail
    || serialized.includes(`"pointer":"/connection_ref"`) && serialized.includes(`"message":"${detail}"`),
  code + '_detail');
}

// Guard the real browser create request. The UI owns both UUIDs; custody is
// established from the observed request before route.fetch, and the accepted
// response is returned to the page exactly once. No direct API call can satisfy
// this helper.
export async function armManagedConnectionAttachment(ctx, page, lease, expected, desired, credentialHashes) {
  let attempts = 0, resolve;
  const completed = new Promise(done => {resolve = done;});
  const pending = new Set(), failures = [];
  const signal = ctx.signal;
  const cancelled = () => {if (!attempts) resolve({error: signal.reason ?? Error('managed_connection_cancelled')});};
  signal?.addEventListener('abort', cancelled, {once: true});
  if (signal?.aborted) cancelled();
  const origin = new URL(page.url()).origin;
  const matcher = url => url.origin === origin && url.pathname === '/api/event-gateway/v1/channels';
  const dispatch = async route => {
    const request = route.request(), method = request.method();
    if (['GET', 'HEAD', 'OPTIONS'].includes(method)) return route.fallback();
    const attempt = ++attempts, errors = []; let fulfilled = false, outcome;
    try {
      ctx.assert(attempt === 1 && method === 'POST', 'managed_connection_unexpected_mutation');
      const body = request.postDataJSON(), headers = await request.allHeaders();
      const key = headers['idempotency-key'];
      assertManagedConnectionIntent(ctx, body, key, expected, desired, credentialHashes);
      const custody = await invoke(lease, 'retainChannelIntent', {recordId: body.record_id, key, body});
      ctx.assert(custody?.retained === true && custody.record_sha256 === digest(body.record_id)
        && custody.key_sha256 === digest(key) && custody.body_sha256 === digest(body),
      'managed_connection_cleanup_custody');
      const intent = {recordId: body.record_id, key, body_sha256: digest(body), configuration: desired, body};
      await ctx.observe('managed_connection_retained_intent', async () => ({
        record_sha256: digest(intent.recordId), key_sha256: digest(key), body_sha256: intent.body_sha256,
      }));
      const remaining = ctx.remainingMs?.() ?? 60000;
      ctx.assert(!signal?.aborted && remaining > 0, 'managed_connection_cancelled');
      const response = await route.fetch({maxRetries: 0, maxRedirects: 0, timeout: remaining});
      try {
        ctx.assert(response.status() === 201, 'managed_connection_create_accepted');
        const record = await response.json(), requestId = response.headers()['x-request-id'];
        assertManagedChannel(ctx, record, intent, expected);
        ctx.assert(typeof requestId === 'string' && requestId.length > 0,
          'managed_connection_actual_request_id');
        outcome = {record, intent, requestId};
        await route.fulfill({response}); fulfilled = true;
      } catch (error) {errors.push(error);}
      try {await response.dispose();} catch (error) {errors.push(error);}
    } catch (error) {errors.push(error);}
    if (!fulfilled) try {await route.abort('failed');} catch (error) {errors.push(error);}
    if (errors.length) {
      const error = errors.length === 1 ? errors[0] : new AggregateError(errors, 'managed_connection_route_failed');
      failures.push(error); outcome = {error};
    }
    if (attempt === 1) resolve(outcome);
  };
  const handler = route => {
    const work = dispatch(route); pending.add(work);
    return work.finally(() => pending.delete(work));
  };
  await page.route(matcher, handler);
  ctx.defer('remove_managed_connection_route', async () => {
    signal?.removeEventListener('abort', cancelled);
    const errors = [];
    try {await page.unroute(matcher, handler);} catch (error) {errors.push(error);}
    const drained = await Promise.allSettled([...pending]);
    errors.push(...drained.filter(r => r.status === 'rejected').map(r => r.reason), ...failures);
    if (errors.length) throw new AggregateError(errors, 'managed_connection_route_cleanup_failed');
  });
  return {attempts: () => attempts, async result() {
    const value = await completed; if (value.error) throw value.error; return value;
  }};
}

export const connectionCases = [
  {...define(44, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
    let lease, session, items, ready, configured, desired, submission, accepted, receipt;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'managed_connection_inventory');
      const expected = lease.expected;
      for (const field of ['ownerSubject', 'connectionRef', 'connectionLabel', 'connectionAdmission',
        'boundConnectionRef', 'boundRecordId', 'notReadyConnectionRef', 'foreignConnectionRef', 'routeId']) {
        if (expected?.[field] === undefined || expected?.[field] === null)
          throw new Blocked('managed_connection_' + field + '_fixture_required');
      }
      session = await openBrowser(ctx, 'user', {channelCreation: true});
      ctx.assert(session.identity?.subject === expected.ownerSubject, 'managed_connection_browser_owner_identity');
      for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
        if (typeof session[method] !== 'function') throw new Blocked('managed_connection_actual_eg_browser_required');
      await goto(session.page, '/event-gateway');
      await visible(ctx, session.page.getByRole('heading', {name: 'Event Gateway', exact: true}),
        'connection_gateway_open');
      items = await pages(ctx, lease, '/v1/channel-connections', 'connection_ref',
        {replica: ctx.am, cursorField: 'next_after'});
      ctx.assert(items.length >= 1 && items.every(item => item.connection_ref && item.status === 'ready'
        && !item.url?.includes('token')), 'connection_page_owned_ready_items');
      ready = items.find(item => item.connection_ref === expected.connectionRef);
      ctx.assert(ready?.label === expected.connectionLabel || ready?.connection_ref === expected.connectionLabel,
        'connection_ready_expected_label');
      const detail = await egRequest(ctx, lease, {path: connection(ready.connection_ref)});
      ctx.assert(detail.connection_ref === expected.connectionRef && detail.status === 'ready'
        && !JSON.stringify(detail).includes('token'), 'connection_detail_exact_ref');
      const bound = await egRequest(ctx, lease, {path: connection(expected.boundConnectionRef)});
      ctx.assert(bound.connection_ref === expected.boundConnectionRef, 'connection_bound_fixture_visible');
      const notReady = await egRequest(ctx, lease, {path: connection(expected.notReadyConnectionRef)});
      ctx.assert(notReady.connection_ref === expected.notReadyConnectionRef && notReady.status !== 'ready',
        'connection_not_ready_fixture_visible');
      const foreign = await egRequest(ctx, lease, {path: connection(expected.foreignConnectionRef),
        allowed: [404], returnResponse: true});
      ctx.assert(foreign.status === 404, 'connection_foreign_owner_hidden');
    });
    await ctx.step('step_2', async () => {
      configured = await configureChannelForm(ctx, session, lease);
      const page = session.page;
      const managed = page.getByRole('checkbox',
        {name: 'Use an existing CMA-managed Telegram connection', exact: true});
      const browserPages = [];
      const listResponse = page.waitForResponse(response => response.request().method() === 'GET'
        && new URL(response.url()).pathname === '/api/event-gateway/v1/channel-connections',
      {timeout: Math.max(1, Math.min(ctx.remainingMs?.() ?? 15000, 15000))});
      await managed.check();
      let browserResponse = await listResponse;
      ctx.assert(browserResponse.status() === 200, 'connection_browser_list_status');
      browserPages.push(await browserResponse.json());
      await invoke(session, 'networkIdle');
      const browserReceipt = channelReceipt(ctx, await invoke(session, 'egEvidence'),
        browserResponse.headers()['x-request-id'], 'GET', 200);
      await ctx.observe('connection_browser_list_receipt', async () => ({schema: 'multi-eg-request-v1', receipt: browserReceipt}));
      while (browserPages.at(-1)?.next_after) {
        const after = browserPages.at(-1).next_after;
        const nextResponse = page.waitForResponse(response => response.request().method() === 'GET'
          && new URL(response.url()).pathname === '/api/event-gateway/v1/channel-connections'
          && new URL(response.url()).searchParams.get('after') === after,
        {timeout: Math.max(1, Math.min(ctx.remainingMs?.() ?? 15000, 15000))});
        await visible(ctx, page.getByRole('button', {name: 'Load more connections', exact: true}),
          'connection_browser_load_more');
        await page.getByRole('button', {name: 'Load more connections', exact: true}).click();
        browserResponse = await nextResponse;
        ctx.assert(browserResponse.status() === 200, 'connection_browser_page_status');
        browserPages.push(await browserResponse.json());
        await invoke(session, 'networkIdle');
        const receipt = channelReceipt(ctx, await invoke(session, 'egEvidence'),
          browserResponse.headers()['x-request-id'], 'GET', 200);
        await ctx.observe('connection_browser_page_receipt', async () => ({schema: 'multi-eg-request-v1', receipt}));
        ctx.assert(browserPages.length <= 20, 'connection_browser_page_budget');
      }
      const browserItems = browserPages.flatMap(value => value.items ?? []);
      ctx.assert(browserItems.some(item => item.connection_ref === expected.connectionRef)
        && browserItems.every(item => item.connection_ref && item.status === 'ready'
          && !item.url?.includes('token')), 'connection_browser_owner_inventory');
      await visible(ctx, page.getByText(/Choose a ready, unbound Telegram connection/, {exact: false}),
        'connection_attach_explanation');
      const saved = page.getByRole('button', {name: 'Saved Telegram connection', exact: true});
      await visible(ctx, saved, 'connection_saved_picker'); await saved.click();
      await page.getByRole('menuitemradio', {name: expected.connectionLabel, exact: true}).click();
      await fillConnectionAdmission(page, expected.connectionAdmission);
      ctx.assert(await saved.innerText() === expected.connectionLabel, 'connection_ui_selected_exact_ref');
      const {durable_custom_execution: _ignored, ...baseDesired} = configured.desired;
      desired = {...baseDesired, sources: [], connection_ref: expected.connectionRef,
        connection_admission: expected.connectionAdmission};
      submission = await armManagedConnectionAttachment(ctx, page, lease, expected, desired,
        configured.credentialHashes);
      await page.getByRole('button', {name: 'Create channel', exact: true}).click();
      accepted = await submission.result();
      await visible(ctx, page.getByRole('heading', {name: desired.name, exact: true}),
        'connection_channel_created_ui');
      await invoke(session, 'networkIdle');
      receipt = channelReceipt(ctx, await invoke(session, 'egEvidence'), accepted.requestId, 'POST', 201);
      await ctx.observe('managed_connection_create_receipt', async () => ({schema: 'multi-eg-request-v1', receipt}));
      const peerRecord = await egRequest(ctx, lease, {path: channel(accepted.record.channel_id),
        replica: peer(receipt.replica)});
      assertManagedChannel(ctx, peerRecord, accepted.intent, expected);
      await session.switchEG(peer(receipt.replica));
      const selected = await openListedChannel(ctx, session, peerRecord,
        () => goto(page, '/event-gateway'));
      assertManagedChannel(ctx, selected.body, accepted.intent, expected);
      ctx.assert(selected.receipt.replica === peer(receipt.replica), 'connection_peer_ui_actual_replica');

      const refused = async (connectionRef, recordId, detail, code, allowed = [400]) => {
        const body = structuredClone(accepted.intent.body);
        body.record_id = recordId; body.connection_ref = connectionRef;
        const response = await egRequest(ctx, lease, {method: 'POST', path: '/v1/channels', body,
          key: randomUUID(), allowed, returnResponse: true});
        if (allowed.length === 1 && allowed[0] === 400) assertRefusal(ctx, response, detail, code);
        return response;
      };
      await refused(expected.boundConnectionRef, expected.boundRecordId,
        'connection is already bound to another channel', 'connection_bound_refused');
      await refused(expected.notReadyConnectionRef, randomUUID(), 'connection is not ready',
        'connection_not_ready_refused');
      const foreign = await refused(expected.routeId, randomUUID(), '', 'connection_route_id_refused', [404]);
      ctx.assert(foreign.status === 404, 'connection_route_id_refused_status');
    });
    await ctx.step('expected_result', async () => {
      const records = await invoke(lease, 'channelCreationRecords',
        {recordId: accepted.intent.recordId, key: accepted.intent.key});
      ctx.assert(submission.attempts() === 1 && records.createAttempts === 1
        && same(records.recordIds, [accepted.intent.recordId])
        && same(records.channelIds, [accepted.record.channel_id])
        && records.retained_body_sha256 === accepted.intent.body_sha256
        && records.retained_key_sha256 === digest(accepted.intent.key),
      'connection_attach_exactly_one_record');
      ctx.assert(records.providerTurns === 0 && records.deliveries === 0
        && records.pending === 0 && records.unknown === 0,
      'connection_attach_no_provider_or_unresolved_work');
      const after = await pages(ctx, lease, '/v1/channel-connections', 'connection_ref',
        {replica: peer(receipt.replica), cursorField: 'next_after'});
      ctx.assert(!after.some(item => item.connection_ref === lease.expected.connectionRef),
        'connection_attached_removed_from_available_inventory');
      await ctx.observe('connection_attachment', async () => ({record_sha256: digest(accepted.intent.recordId),
        channel_sha256: digest(accepted.record.channel_id), connection_sha256: digest(lease.expected.connectionRef),
        request_sha256: digest(accepted.requestId), status: 'attached'}));
      await uiReceipt(ctx, session.page);
    });
  }), evidence_scope: 'eg_only'},
  define(45, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
    let lease, session, created, status;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'connection_browser_setup'); session = await openBrowser(ctx, 'user', {connectionSetup: true});
      const event = lease.event; const result = await egRequest(ctx, lease, {method: 'POST', path: '/v1/channel-connections',
        body: {record_id: lease.recordId, event_ref: event.eventRef, suggested_name: lease.expected.suggestedName}, key: lease.keys.create});
      created = result; ctx.assert(result.connection_ref && result.setup_url && result.status === 'draft', 'connection_setup_reserved');
    });
    await ctx.step('step_2', async () => {
      const setup = await session.page.context().request.get(new URL(created.setup_url, session.origin).href);
      ctx.assert(setup.status() === 200 && (await setup.text()).includes('csrf_token'), 'connection_one_use_setup_page');
      const submitted = await invoke(lease, 'submitConnectionBrowserForm', {setupRef: created.setup_url,
        actor: 'owner', label: lease.expected.connectionLabel});
      ctx.assert(submitted.status === 'accepted' && submitted.connection_ref === created.connection_ref,
        'connection_browser_form_submitted');
      status = await egRequest(ctx, lease, {path: connection(created.connection_ref), replica: peer(ctx.am)});
      ctx.assert(status.status === 'ready' && status.connection_ref === created.connection_ref
        && !JSON.stringify(status).includes(lease.expected.secretMarker), 'connection_ready_scoped_nonsecret');
    });
    await ctx.step('expected_result', async () => { const after = await invoke(lease, 'connectionStatus', {connectionRef: created.connection_ref});
      ctx.assert(after.status === 'ready' && after.owner === lease.expected.ownerSubject, 'connection_final_owner_status'); await uiReceipt(ctx, session.page); });
  }),
  define(46, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
    let lease, session, created;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'connection_cancel'); session = await openBrowser(ctx, 'user', {connectionSetup: true});
      created = await egRequest(ctx, lease, {method: 'POST', path: '/v1/channel-connections', body: {
        record_id: lease.recordId, event_ref: lease.event.eventRef, suggested_name: lease.expected.suggestedName}, key: lease.keys.create});
      ctx.assert(created.status === 'draft' && created.setup_url, 'connection_cancel_draft_created');
      const cancelled = await egRequest(ctx, lease, {method: 'DELETE', path: connection(created.connection_ref), replica: ctx.am, allowed: [204], returnResponse: true});
      ctx.assert(cancelled.status === 204, 'connection_cancel_confirmed');
    });
    await ctx.step('step_2', async () => {
      const stale = await invoke(lease, 'openCancelledConnection', {setupUrl: created.setup_url});
      ctx.assert(stale.status === 404 && stale.active === false, 'connection_cancel_old_link_expired');
      const read = await egRequest(ctx, lease, {path: connection(created.connection_ref), replica: peer(ctx.am), allowed: [404], returnResponse: true});
      ctx.assert(read.status === 404, 'connection_cancel_peer_absent');
    });
    await ctx.step('expected_result', async () => { const count = await invoke(lease, 'connectionRecords');
      ctx.assert(!count.some(item => item.connection_ref === created.connection_ref), 'connection_cancel_no_retained_active_record'); await uiReceipt(ctx, session.page); });
  }),
];
