import {
  define,
  begin,
  button,
  input,
  go,
  uiMutation,
  visible,
  absent,
  other,
  enc,
  hash,
  unique,
  requireInput,
  poll,
  assertNoSecret,
} from './core.mjs';
import { channelDraft } from './forms.mjs';

const base = '/v1/channel-connections';
async function newConnection(ctx, fx) {
  const event = fx.originEvent;
  const response = await fx.call({
    method: 'POST',
    path: base,
    actor: 'originating_agent',
    allowed: [201],
    body: {
      record_id: event.record_id,
      event_ref: event.event_ref,
      suggested_name: unique('Connection'),
    },
  });
  ctx.assert(
    response.body.connection_ref && response.body.status === 'draft' && response.body.setup_url,
    'eg20_connection_admission',
  );
  return response.body;
}
async function setupPage(ctx, fx, connection) {
  const target = new URL(connection.setup_url, fx.session.origin);
  requireInput(
    ['127.0.0.1', 'localhost', '[::1]'].includes(target.hostname),
    'eg20_connection_local_url_required',
  );
  await go(fx, target.pathname + target.search);
  await visible(ctx, input(fx.session.page, 'Bot token'), 'eg20_real_setup_form');
  const cookies = await fx.session.page.context().cookies();
  ctx.assert(
    cookies.some((cookie) => cookie.httpOnly && cookie.expires > Date.now() / 1000),
    'eg20_setup_cookie_httponly_bounded',
  );
  ctx.assert(
    await fx.session.page.locator('input[name=csrf_token]').inputValue(),
    'eg20_setup_form_csrf',
  );
}
export const cases = [
  define(44, [
    async (ctx, s) => {
      await begin(ctx, s, 'connections');
      s.items = [];
      let after;
      const seen = new Set();
      let pages = 0;
      do {
        ctx.assert(pages++ < 20, 'eg20_connection_page_bound');
        const response = await s.fx.eg({ path: base + (after ? `?after=${enc(after)}` : '') });
        ctx.assert(Array.isArray(response.body.items), 'eg20_connection_page_shape');
        for (const item of response.body.items) {
          ctx.assert(!seen.has(item.connection_ref), 'eg20_connection_no_duplicate');
          seen.add(item.connection_ref);
          s.items.push(item);
        }
        after = response.body.next_after;
      } while (after);
      ctx.assert(pages >= 2, 'eg20_connection_pagination_exercised');
      s.ref = requireInput(
        s.fx.authority.connectionRef,
        'eg20_ready_owned_connection_fixture_required',
      );
      s.ready = (await s.fx.eg({ path: base + '/' + enc(s.ref) })).body;
      ctx.assert(
        s.ready.status === 'ready' && s.items.some((item) => item.connection_ref === s.ref),
        'eg20_ready_owned_connection',
      );
      await channelDraft(ctx, s.fx, { connectionRef: s.ref });
    },
    async (ctx, s) => {
      s.created = await uiMutation(ctx, s.fx, {
        method: 'POST',
        path: '/api/event-gateway/v1/channels',
        action: () => button(s.page, 'Create channel').click(),
        allowed: [201],
        kind: 'channel',
      });
      ctx.assert(
        s.created.body.definition.connection_ref === s.ref &&
          s.created.submitted.connection_ref === s.ref,
        'eg20_ui_attaches_exact_connection',
      );
      s.fx.channel = s.created.body;
      const peer = await s.fx.read();
      ctx.assert(peer.definition.connection_ref === s.ref, 'eg20_bound_connection_peer');
      for (const ref of [
        s.ref,
        requireInput(
          s.fx.authority.notReadyConnectionRef,
          'eg20_not_ready_connection_fixture_required',
        ),
        requireInput(s.fx.authority.nyxidRouteId, 'eg20_real_nyxid_route_fixture_required'),
      ]) {
        const body = {
          ...s.created.submitted,
          record_id: unique('').slice(1),
          connection_ref: ref,
        };
        const denied = await s.fx.call({
          method: 'POST',
          path: '/v1/channels',
          body,
          key: unique('refused'),
          allowed: [400, 403, 404, 409, 422],
        });
        ctx.assert(denied.status !== 201, 'eg20_ineligible_or_route_id_not_connection');
      }
    },
    async (ctx, s) => {
      const read = await s.fx.eg({ path: base + '/' + enc(s.ref), replica: other(ctx.am) });
      ctx.assert(
        read.body.connection_ref === s.ref && read.body.status === 'ready',
        'eg20_connection_retains_identity',
      );
      assertNoSecret(ctx, [s.items, read.body, s.created.body], s.fx.secrets);
    },
  ]),
  define(45, [
    async (ctx, s) => {
      await begin(ctx, s, 'connection-create');
      s.created = await newConnection(ctx, s.fx);
      ctx.assert(
        s.fx.originEvent.owner === s.fx.authority.ownerSubject,
        'eg20_origin_event_owner_binding',
      );
    },
    async (ctx, s) => {
      await setupPage(ctx, s.fx, s.created);
      const token = requireInput(
        s.fx.authority.syntheticTelegramToken,
        'eg20_local_telegram_token_required',
      );
      s.fx.secrets.push(token);
      await input(s.page, /Connection name/).fill('Local coverage connection');
      await input(s.page, 'Bot token').fill(token);
      const post = s.page.waitForResponse(
        (r) => r.request().method() === 'POST' && new URL(r.url()).pathname === '/connect/submit',
      );
      await button(s.page, 'Connect bot').click();
      const response = await post;
      ctx.assert([200, 303].includes(response.status()), 'eg20_browser_connection_submitted');
      s.ready = await poll(
        ctx,
        async () =>
          (
            await s.fx.eg({
              path: base + '/' + enc(s.created.connection_ref),
              replica: other(ctx.am),
            })
          ).body,
        (value) => value.status === 'ready',
        'eg20_connection_ready_deadline',
      );
      await visible(
        ctx,
        s.page.getByText('Connected', { exact: true }),
        'eg20_browser_connection_ready',
      );
    },
    async (ctx, s) => {
      ctx.assert(
        s.ready.connection_ref === s.created.connection_ref,
        'eg20_same_connection_completed',
      );
      const denied = await s.fx.eg({
        path: base + '/' + enc(s.ready.connection_ref),
        actor: 'outsider',
        allowed: [403, 404],
      });
      ctx.assert(denied.status !== 200, 'eg20_connection_foreign_owner_denied');
      const metadata = await s.fx.platform.journal({ connectionRef: s.ready.connection_ref });
      ctx.assert(
        metadata.filter((row) => row.kind === 'set_webhook').length === 1,
        'eg20_webhook_activated_once',
      );
      assertNoSecret(ctx, s.ready, s.fx.secrets);
    },
  ]),
  define(46, [
    async (ctx, s) => {
      await begin(ctx, s, 'connection-cancel');
      s.created = await newConnection(ctx, s.fx);
      const result = await s.fx.call({
        method: 'DELETE',
        path: base + '/' + enc(s.created.connection_ref),
        allowed: [204],
      });
      ctx.assert(result.status === 204 && result.body === null, 'eg20_cancel_empty_204');
    },
    async (ctx, s) => {
      const target = new URL(s.created.setup_url, s.fx.session.origin);
      requireInput(
        ['127.0.0.1', 'localhost', '[::1]'].includes(target.hostname),
        'eg20_connection_local_url_required',
      );
      await s.fx.session.switchEG(other(ctx.am));
      const response = await go(s.fx, target.pathname + target.search);
      ctx.assert([400, 404, 410].includes(response.status()), 'eg20_stale_connection_link_refused');
      await absent(ctx, input(s.page, 'Bot token'), 'eg20_cancelled_setup_cannot_activate');
    },
    async (ctx, s) => {
      for (const replica of [ctx.am, other(ctx.am)]) {
        const read = await s.fx.eg({
          path: base + '/' + enc(s.created.connection_ref),
          replica,
          allowed: [200, 404],
        });
        ctx.assert(
          read.status === 404 || read.body.status === 'cancelled',
          'eg20_cancel_durable_both_egs',
        );
      }
      const journal = await s.fx.platform.journal({ connectionRef: s.created.connection_ref });
      ctx.assert(
        !journal.some((row) => row.kind === 'set_webhook'),
        'eg20_cancelled_draft_never_activated',
      );
      await ctx.observe('eg20_cancelled_connection', async () => ({
        connection_sha256: hash(s.created.connection_ref),
      }));
    },
  ]),
];
