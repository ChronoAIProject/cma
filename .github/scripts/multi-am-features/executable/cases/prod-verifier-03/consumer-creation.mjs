import {define, openBrowser, acquire, invoke, egRequest, visible, uiReceipt, goto, peer, segment, same, digest, poll} from './helpers.mjs';

const bot = lease => `/api/v1/bots/${segment(lease.botId)}`;
const browserBot = async (ctx, lease) => {
  const s = await openBrowser(ctx, 'user', {agentId: lease.agentId}); await goto(s.page, '/bots');
  await visible(ctx, s.page.getByRole('heading', {name: 'Consumers', exact: true}), 'consumer_list_open'); return s;
};

export const consumerCreationCases = [
  define(47, 'browser', ['browser', 'eg'], async ctx => {
    let lease, session, list;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'consumer_inventory'); session = await browserBot(ctx, lease);
      list = await egRequest(ctx, lease, {path: '/api/v1/bots'});
      ctx.assert(Array.isArray(list.items) && list.items.length >= 2 && list.items.every(item => item.bot_id && item.name && item.state), 'consumer_inventory_shape');
      const own = await egRequest(ctx, lease, {path: bot(lease)}); ctx.assert(own.bot_id === lease.botId && own.owner === lease.expected.ownerSubject, 'consumer_detail_owner');
    });
    await ctx.step('step_2', async () => {
      for (const replica of [ctx.am, peer(ctx.am)]) {
        const detail = await egRequest(ctx, lease, {path: bot(lease), replica});
        ctx.assert(detail.bot_id === lease.botId && detail.state && detail.channel, 'consumer_status_current');
        ctx.assert(detail.channel.level && detail.connection, 'consumer_connection_metadata');
      }
      const connections = await egRequest(ctx, lease, {path: '/api/v1/bots/connections'}); ctx.assert(Array.isArray(connections.items), 'consumer_connection_choices');
      const forbidden = await egRequest(ctx, lease, {path: '/api/v1/bots/' + segment(lease.expected.foreignBotId), actor: 'outsider', allowed: [403, 404]}); ctx.assert(!forbidden.bot_id, 'consumer_foreign_denied');
    });
    await ctx.step('expected_result', async () => { ctx.assert(list.items.some(item => item.bot_id === lease.botId), 'consumer_own_list_membership'); await uiReceipt(ctx, session.page); });
  }),
  define(48, 'browser', ['browser', 'eg'], async ctx => {
    let lease, session, created;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'consumer_create'); session = await openBrowser(ctx); await goto(session.page, '/bots/new');
      await visible(ctx, session.page.getByRole('heading', {name: 'New consumer', exact: true}), 'consumer_create_open');
      created = await egRequest(ctx, lease, {method: 'POST', path: '/api/v1/bots', body: lease.createBotBody, key: lease.keys.create, allowed: [201, 202]});
      ctx.assert(created.bot_id === lease.expected.createdBotId && created.state && created.channel, 'consumer_create_durable_id');
    });
    await ctx.step('step_2', async () => {
      const replay = await egRequest(ctx, lease, {method: 'POST', path: '/api/v1/bots', body: lease.createBotBody, key: lease.keys.create, replica: peer(ctx.am), allowed: [201, 202]}); ctx.assert(replay.bot_id === created.bot_id && replay.version === created.version, 'consumer_create_replay');
      const active = await poll(ctx, () => egRequest(ctx, lease, {path: bot({...lease, botId: created.bot_id}), replica: peer(ctx.am)}), value => ['active','ready'].includes(value.state), 'consumer_provision_timeout');
      ctx.assert(active.bot_id === created.bot_id && active.channel.channel_id === lease.expected.channelId, 'consumer_provision_active_channel');
      ctx.assert(active.profile.metadata.name === lease.expected.profileName && active.owner === lease.expected.ownerSubject, 'consumer_provision_profile_owner');
    });
    await ctx.step('expected_result', async () => { const counts = await invoke(lease, 'counters'); ctx.assert(counts.createAttempts === 1 && counts.channelsCreated === 1, 'consumer_create_single_allocation'); await uiReceipt(ctx, session.page); });
  }),
  define(49, 'browser', ['browser', 'eg', 'eg_browser'], async ctx => {
    let lease, session, draft, created;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'bot_father_profile'); session = await openBrowser(ctx, 'user', {botFather: true}); await goto(session.page, '/bots/new');
      draft = await invoke(lease, 'resumeBotFatherDraft', {actor: 'owner'}); ctx.assert(draft.setup_id && draft.identity.subject === lease.expected.ownerSubject && draft.revision > 0, 'bot_father_draft_identity');
      await visible(ctx, session.page.getByRole('heading', {name: /Create your Bot with CMA Bot Father/}), 'bot_father_profile_open');
    });
    await ctx.step('step_2', async () => {
      const profile = await invoke(lease, 'publishBotFatherProfile', {setupId: draft.setup_id, revision: draft.revision, body: lease.profileBody, key: lease.keys.profile}); ctx.assert(profile.status === 'published' && profile.profile_digest === lease.expected.profileDigest, 'bot_father_profile_published');
      created = await egRequest(ctx, lease, {method: 'POST', path: '/api/v1/bots', body: {...lease.createBotBody, profile_digest: profile.profile_digest}, key: lease.keys.create, allowed: [201, 202]}); ctx.assert(created.bot_id === lease.expected.createdBotId, 'bot_father_consumer_created');
    });
    await ctx.step('expected_result', async () => { const other = await invoke(lease, 'resumeBotFatherDraft', {actor: 'outsider'}); ctx.assert(other.status === 403 || other.status === 404, 'bot_father_changed_identity_denied'); await uiReceipt(ctx, session.page); });
  }),
  define(55, 'eg', ['eg'], async ctx => {
    let lease, view, confirmed;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'bot_father_actions'); view = await invoke(lease, 'botSetupAdvance', {eventRef: lease.event.eventRef, activity: lease.expected.attestedActivity});
      ctx.assert(view.setup_id && view.identity.subject === lease.expected.ownerSubject && view.revision > 0, 'bot_setup_attested_identity');
      const inspect = await invoke(lease, 'botSetupAction', {eventRef: lease.event.eventRef, request: {request_id: lease.keys.view, action: {type: 'view'}}});
      ctx.assert(inspect.setup_id === view.setup_id && inspect.revision === view.revision, 'bot_setup_view_exact_revision');
    });
    await ctx.step('step_2', async () => {
      const choose = await invoke(lease, 'botSetupAction', {eventRef: lease.event.eventRef, request: {request_id: lease.keys.choose, expected: {setup_id: view.setup_id, revision: view.revision}, action: {type: 'choose', choice_id: lease.expected.choiceId}}});
      const customized = await invoke(lease, 'botSetupAction', {eventRef: lease.event.eventRef, request: {request_id: lease.keys.customize, expected: {setup_id: choose.setup_id, revision: choose.revision}, action: {type: 'customize', customization: lease.expected.customization}}});
      confirmed = await invoke(lease, 'botSetupAction', {eventRef: lease.event.eventRef, request: {request_id: lease.keys.confirm, expected: {setup_id: customized.setup_id, revision: customized.revision}, action: {type: 'confirm', plan_digest: customized.confirmation_digest}}});
      ctx.assert(confirmed.result?.bot_id === lease.expected.botId && confirmed.result.state, 'bot_setup_confirmed_exact_plan');
      const replay = await invoke(lease, 'botSetupAction', {eventRef: lease.event.eventRef, request: {request_id: lease.keys.confirm, expected: {setup_id: customized.setup_id, revision: customized.revision}, action: {type: 'confirm', plan_digest: customized.confirmation_digest}}});
      ctx.assert(replay.revision === confirmed.revision && replay.result.bot_id === confirmed.result.bot_id, 'bot_setup_confirm_replay');
    });
    await ctx.step('expected_result', async () => { const altered = await invoke(lease, 'botSetupAction', {eventRef: lease.event.eventRef, request: {request_id: lease.keys.confirm, expected: {setup_id: view.setup_id, revision: view.revision}, action: {type: 'confirm', plan_digest: lease.expected.wrongPlanDigest}}}); ctx.assert(altered.status === 409 || altered.failure, 'bot_setup_stale_plan_refused'); await ctx.observe('bot_setup_receipt', async () => ({setup_sha256: digest(view.setup_id), bot_sha256: digest(confirmed.result.bot_id), revision: confirmed.revision})); });
  }),
];
