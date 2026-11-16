import {define, acquire, openBrowser, visible, uiReceipt, egRequest, invoke, peer, segment, pages, same, digest, goto} from './helpers.mjs';

const connection = ref => `/v1/channel-connections/${segment(ref)}`;
const channel = lease => `/v1/channels/${segment(lease.channelId)}`;

export const connectionCases = [
  define(44, 'browser', ['browser', 'eg'], async ctx => {
    let lease, session, items, attached;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'managed_connection_inventory'); session = await openBrowser(ctx);
      await goto(session.page, '/event-gateway');
      await visible(ctx, session.page.getByRole('heading', {name: 'Event Gateway', exact: true}), 'connection_gateway_open');
      const page = await pages(ctx, lease, '/v1/channel-connections', 'connection_ref', {replica: ctx.am}); items = page;
      ctx.assert(items.length >= 2 && items.every(item => item.connection_ref && item.status), 'connection_page_exact_items');
      const ready = items.find(item => item.connection_ref === lease.expected.connectionRef);
      ctx.assert(ready?.status === 'ready' && !ready.url?.includes('token'), 'connection_ready_nonsecret_view');
      const detail = await egRequest(ctx, lease, {path: connection(ready.connection_ref)});
      ctx.assert(detail.connection_ref === ready.connection_ref && detail.status === 'ready', 'connection_detail_exact_ref');
    });
    await ctx.step('step_2', async () => {
      await session.page.getByRole('button', {name: 'New channel', exact: true}).click();
      await session.page.getByRole('switch', {name: 'Use an existing CMA-managed Telegram connection', exact: true}).click();
      await visible(ctx, session.page.getByText(/Choose a ready, unbound Telegram connection/, {exact: false}), 'connection_attach_explanation');
      await session.page.getByLabel('Saved Telegram connection', {exact: true}).selectOption(lease.expected.connectionRef);
      const created = await egRequest(ctx, lease, {method: 'POST', path: '/v1/channels', body: lease.connectionBody,
        key: lease.keys.create, allowed: [201]}); attached = created;
      ctx.assert(created.definition.connection_ref === lease.expected.connectionRef && created.definition.sources.length === 0,
        'connection_attached_replaces_manual_source');
      const bound = await egRequest(ctx, lease, {path: channel(lease), replica: peer(ctx.am)});
      ctx.assert(bound.definition.connection_ref === lease.expected.connectionRef, 'connection_peer_bound');
      const refused = await egRequest(ctx, lease, {method: 'POST', path: '/v1/channels', body: {...lease.connectionBody,
        record_id: lease.expected.boundRecordId, connection_ref: lease.expected.connectionRef}, key: lease.keys.bound, allowed: [409]});
      ctx.assert(refused.code || refused.type, 'already_bound_connection_refused');
    });
    await ctx.step('expected_result', async () => {
      ctx.assert(items.some(item => item.connection_ref === attached.definition.connection_ref), 'connection_inventory_retained');
      await ctx.observe('connection_attachment', async () => ({connection_sha256: digest(attached.definition.connection_ref), status: attached.definition.connection_ref && 'ready'})); await uiReceipt(ctx, session.page);
    });
  }),
  define(45, 'browser', ['browser', 'eg', 'eg_browser'], async ctx => {
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
  define(46, 'browser', ['browser', 'eg', 'eg_browser'], async ctx => {
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
      const read = await egRequest(ctx, lease, {path: connection(created.connection_ref), allowed: [404]});
      ctx.assert(read.status === 404, 'connection_cancel_peer_absent');
    });
    await ctx.step('expected_result', async () => { const count = await invoke(lease, 'connectionRecords');
      ctx.assert(!count.some(item => item.connection_ref === created.connection_ref), 'connection_cancel_no_retained_active_record'); await uiReceipt(ctx, session.page); });
  }),
];
