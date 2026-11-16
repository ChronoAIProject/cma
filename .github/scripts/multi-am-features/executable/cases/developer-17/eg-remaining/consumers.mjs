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
  same,
  hash,
  unique,
  requireInput,
  poll,
  openChannel,
} from './core.mjs';
import { consumerDraft } from './forms.mjs';
const base = '/api/v1/bots';
const alias = '/api/agent-manager/v1/bots';
async function submit(ctx, s) {
  s.created = await uiMutation(ctx, s.fx, {
    method: 'POST',
    path: alias,
    action: () => button(s.page, 'Create consumer').click(),
    allowed: [201],
    kind: 'bot',
  });
  ctx.assert(
    s.created.body.bot_id && s.created.body.owner === s.fx.authority.ownerSubject,
    'eg20_consumer_owner_receipt',
  );
  const agent = s.created.submitted.agent;
  ctx.assert(
    agent.model === s.fx.authority.consumer.agent.model &&
      agent.system_prompt ===
        (s.fx.name === 'father-profile'
          ? s.fx.authority.fatherDraft.instructions
          : s.fx.authority.consumer.agent.system_prompt) &&
      agent.reasoning_effort === s.fx.authority.consumer.agent.reasoning_effort &&
      agent.permission_mode === s.fx.authority.consumer.agent.permission_mode,
    'eg20_consumer_exact_inline_agent',
  );
  ctx.assert(
    same(agent.permissions.services, s.fx.authority.consumer.agent.permissions.services) &&
      same(agent.skills, s.fx.authority.consumer.agent.skills),
    'eg20_consumer_exact_services_skills',
  );
}
async function waitActive(ctx, s) {
  const id = s.created.body.bot_id;
  const active = await poll(
    ctx,
    async () => {
      const response = await s.fx.am({
        method: 'GET',
        am: other(ctx.am),
        path: `${base}/${enc(id)}`,
      });
      ctx.assert(response.body.state !== 'failed', 'eg20_consumer_provisioning_failed');
      return response.body;
    },
    (value) => value.state === 'active',
    'eg20_consumer_active_deadline',
  );
  ctx.assert(
    active.bot_id === id &&
      active.owner === s.fx.authority.ownerSubject &&
      active.channel?.record_id,
    'eg20_active_consumer_channel',
  );
  return active;
}
const inventory = define(47, [
  async (ctx, s) => {
    await begin(ctx, s, 'consumers');
    await go(s.fx, '/bots');
    await visible(
      ctx,
      s.page.getByRole('heading', { name: 'Consumers', exact: true }),
      'eg20_consumer_list',
    );
    s.pages = [];
    let after;
    for (let index = 0; index < 20; index++) {
      const response = await s.fx.am({
        method: 'GET',
        path: base + (after ? `?after=${enc(after)}` : ''),
      });
      s.pages.push(response.body);
      after = response.body.next_after;
      if (!after) break;
      await button(s.page, 'Next page').click();
    }
    ctx.assert(s.pages.length >= 2 && !after, 'eg20_consumer_pagination_complete');
    s.items = s.pages.flatMap((page) => page.items);
    ctx.assert(
      new Set(s.items.map((bot) => bot.bot_id)).size === s.items.length &&
        s.items.every((bot) => bot.owner === s.fx.authority.ownerSubject),
      'eg20_consumer_own_inventory',
    );
    s.bot = s.items.find((bot) => bot.bot_id === s.fx.authority.consumerId);
    requireInput(s.bot, 'eg20_seed_consumer_required');
    await go(s.fx, '/bots/' + enc(s.bot.bot_id));
    await visible(
      ctx,
      s.page.getByRole('heading', { name: s.bot.name, exact: true }),
      'eg20_consumer_exact_detail',
    );
  },
  async (ctx, s) => {
    await button(s.page, 'Refresh status').click();
    s.read = (
      await s.fx.am({ method: 'GET', path: base + '/' + enc(s.bot.bot_id), am: other(ctx.am) })
    ).body;
    ctx.assert(s.read.bot_id === s.bot.bot_id && s.read.state, 'eg20_consumer_current_state');
    for (const heading of [
      'Last test',
      s.read.connection_ref ? 'Event Gateway connection' : 'Callback URL',
    ])
      if (heading !== 'Callback URL' || s.read.channel)
        await visible(
          ctx,
          s.page.getByRole('heading', { name: heading, exact: true }),
          'eg20_consumer_status_metadata_visible',
        );
    if (s.read.failure)
      await visible(
        ctx,
        s.page.getByText(s.read.failure, { exact: false }),
        'eg20_consumer_failure_explicit',
      );
    const choices = (
      await s.fx.am({ method: 'GET', path: base + '/connections', am: other(ctx.am) })
    ).body;
    ctx.assert(
      Array.isArray(choices) && choices.every((row) => row.connection_ref && row.label),
      'eg20_am_connection_choices',
    );
    const legacy = (await s.fx.am({ method: 'GET', path: base + '/nyxid-bots' })).body;
    ctx.assert(Array.isArray(legacy), 'eg20_legacy_connection_list');
  },
  async (ctx, s) => {
    const foreign = await s.fx.am({
      method: 'GET',
      path: base + '/' + enc(s.bot.bot_id),
      actor: 'outsider',
      allowed: [403, 404],
    });
    ctx.assert(!foreign.body?.bot_id, 'eg20_consumer_foreign_denied');
    const missing = await s.fx.am({
      method: 'GET',
      path: base,
      actor: 'no_manage_bots',
      allowed: [403],
    });
    ctx.assert(missing.status === 403, 'eg20_manage_bots_capability_required');
  },
]);
const creation = define(48, [
  async (ctx, s) => {
    await begin(ctx, s, 'consumer-create');
    await consumerDraft(ctx, s.fx);
    ctx.assert(await button(s.page, 'Create consumer').isEnabled(), 'eg20_consumer_review_ready');
  },
  async (ctx, s) => {
    await submit(ctx, s);
    ctx.assert(
      s.created.key && s.created.submitted.connection_ref === s.fx.authority.consumer.connectionRef,
      'eg20_consumer_original_intent_connection',
    );
    ctx.assert(
      s.created.submitted.reply.auto_final === true &&
        s.created.submitted.reply.ack_after_ms === 2000,
      'eg20_consumer_reply_policy',
    );
  },
  async (ctx, s) => {
    s.active = await waitActive(ctx, s);
    await s.fx.session.switchAM(other(ctx.am));
    await go(s.fx, '/bots/' + enc(s.active.bot_id));
    await visible(
      ctx,
      s.page.getByRole('heading', { name: s.active.name, exact: true }),
      'eg20_consumer_peer_browser',
    );
    await button(s.page, /Open.*channel|Open Event Gateway/).click();
    const channel = (
      await s.fx.eg({
        path: '/v1/channel-records/' + enc(s.active.channel.record_id),
        replica: other(ctx.am),
      })
    ).body;
    ctx.assert(
      channel.channel_id === s.active.channel.channel_id,
      'eg20_consumer_opens_exact_eg_channel',
    );
    await openChannel(ctx, s.fx, channel);
  },
  async (ctx, s) => {
    ctx.assert(
      s.fx.browserIntents.filter((row) => row.method === 'POST' && row.path === alias).length === 1,
      'eg20_single_consumer_ui_submission',
    );
    const sameId = (
      await s.fx.am({ method: 'GET', path: base + '/' + enc(s.active.bot_id), am: ctx.am })
    ).body;
    ctx.assert(
      sameId.bot_id === s.active.bot_id && sameId.channel.record_id === s.active.channel.record_id,
      'eg20_consumer_stable_durable_result',
    );
  },
]);
const fatherProfile = define(49, [
  async (ctx, s) => {
    await begin(ctx, s, 'father-profile');
    const draft = requireInput(s.fx.authority.fatherDraft, 'eg20_verified_father_draft_required');
    ctx.assert(draft.v === 1 && draft.name && draft.instructions, 'eg20_father_draft_shape');
    await go(s.fx, '/bots/new?setup=' + enc(JSON.stringify(draft)));
    await visible(
      ctx,
      s.page.getByRole('heading', { name: 'Create your Bot with CMA Bot Father', exact: true }),
      'eg20_father_profile_review',
    );
    ctx.assert(
      (await input(s.page, 'Bot name').inputValue()) === draft.name &&
        (await input(s.page, 'Agent instructions').inputValue()) === draft.instructions,
      'eg20_father_original_draft',
    );
  },
  async (ctx, s) => {
    await s.page
      .getByRole('checkbox', {
        name: 'I reviewed these instructions and want to create this profile in my CMA account.',
        exact: true,
      })
      .check();
    const published = s.page.waitForResponse(
      (response) =>
        response.request().method() === 'POST' &&
        /\/agent-profiles\/[^/]+\/publish$/.test(new URL(response.url()).pathname),
    );
    await button(s.page, 'Create my agent profile').click();
    s.profileResponse = await published;
    ctx.assert(
      s.profileResponse.status() === 200 || s.profileResponse.status() === 201,
      'eg20_profile_published',
    );
    s.profile = await s.profileResponse.json();
    s.retained = await s.page.evaluate(() =>
      Object.entries(sessionStorage)
        .filter(([key]) => key.startsWith('cma:bot-father:'))
        .map(([, value]) => JSON.parse(value)),
    );
    ctx.assert(
      s.retained.length === 1 &&
        s.retained[0].owner === s.fx.authority.ownerSubject &&
        s.retained[0].published.digest,
      'eg20_profile_receipt_bound_owner',
    );
    await consumerDraft(ctx, s.fx, { preserveProfile: true });
    await submit(ctx, s);
    s.active = await waitActive(ctx, s);
  },
  async (ctx, s) => {
    const denied = await s.fx.am({
      method: 'GET',
      path: '/api/v1/agent-profiles/' + enc(s.retained[0].document.id),
      actor: 'outsider',
      allowed: [403, 404],
    });
    ctx.assert(denied.status !== 200, 'eg20_father_private_profile_foreign_denied');
    requireInput(
      typeof s.fx.lease.identities?.switchBrowserActor === 'function',
      'eg20_same_browser_actor_switch_required',
    );
    await s.fx.lease.identities.switchBrowserActor({
      session: s.fx.session,
      actor: 'outsider',
      preserveStorage: true,
    });
    await go(s.fx, '/bots/new');
    await absent(
      ctx,
      s.page.getByText('Recover profile creation', { exact: true }),
      'eg20_changed_login_cannot_resume_owner_receipt',
    );
    ctx.assert(
      s.active.owner === s.fx.authority.ownerSubject,
      'eg20_father_consumer_owner_retained',
    );
  },
]);
const fatherActions = define(
  55,
  [
    async (ctx, s) => {
      await begin(ctx, s, 'father-actions');
      const event = s.fx.originEvent;
      s.view = (
        await s.fx.am({
          method: 'POST',
          path: '/api/v1/bot-setups/advance',
          actor: 'bound_consumer',
          body: { event_ref: event.event_ref, activity: event.activity },
        })
      ).body;
      ctx.assert(
        s.view.identity.subject === s.fx.authority.ownerSubject &&
          s.view.setup_id &&
          s.view.revision > 0,
        'eg20_attested_setup_identity',
      );
      ctx.assert(
        Array.isArray(s.view.choices) && Array.isArray(s.view.services) && s.view.configuration,
        'eg20_setup_configuration_choices',
      );
    },
    async (ctx, s) => {
      const act = async (request, allowed = [200]) =>
        s.fx.am({
          method: 'POST',
          path: '/api/v1/bot-setups/actions',
          actor: 'bound_consumer',
          body: { event_ref: s.fx.originEvent.event_ref, request },
          allowed,
        });
      const choice = s.view.choices.find((choice) => choice.id === s.fx.authority.setupChoiceId);
      requireInput(choice, 'eg20_setup_choice_fixture_required');
      const chosen = (
        await act({
          request_id: unique('choose'),
          expected: { setup_id: s.view.setup_id, revision: s.view.revision },
          action: { type: 'choose', choice_id: choice.id },
        })
      ).body;
      const customized = (
        await act({
          request_id: unique('customize'),
          expected: { setup_id: chosen.setup_id, revision: chosen.revision },
          action: {
            type: 'customize',
            customization: requireInput(
              s.fx.authority.setupCustomization,
              'eg20_real_setup_customization_required',
            ),
          },
        })
      ).body;
      requireInput(
        typeof s.fx.platform.attestConsent === 'function',
        'eg20_exact_plan_attested_consent_required',
      );
      const consent = await s.fx.platform.attestConsent({
        eventRef: s.fx.originEvent.event_ref,
        setupId: customized.setup_id,
        revision: customized.revision,
        planDigest: customized.confirmation_digest,
      });
      ctx.assert(
        consent.planDigest === customized.confirmation_digest &&
          consent.owner === s.fx.authority.ownerSubject,
        'eg20_exact_server_plan_consent',
      );
      s.request = {
        request_id: unique('confirm'),
        expected: { setup_id: customized.setup_id, revision: customized.revision },
        action: { type: 'confirm', plan_digest: customized.confirmation_digest },
      };
      s.confirmed = (await act(s.request)).body;
      s.replay = (await act(s.request)).body;
      ctx.assert(
        s.confirmed.result?.bot_id && same(s.confirmed.result, s.replay.result),
        'eg20_confirm_exact_replay',
      );
      s.act = act;
    },
    async (ctx, s) => {
      const altered = await s.act(
        { ...s.request, action: { type: 'confirm', plan_digest: '0'.repeat(64) } },
        [400, 403, 409, 422],
      );
      ctx.assert(altered.status !== 200, 'eg20_changed_confirmation_refused');
      const stale = await s.act(
        {
          ...s.request,
          request_id: unique('stale'),
          expected: { setup_id: s.view.setup_id, revision: s.view.revision },
        },
        [403, 409, 422],
      );
      ctx.assert(stale.status !== 200, 'eg20_stale_confirmation_refused');
      const bot = (
        await s.fx.am({
          method: 'GET',
          path: base + '/' + enc(s.confirmed.result.bot_id),
          am: other(ctx.am),
        })
      ).body;
      await s.fx.capture({ status: 200, body: bot });
      ctx.assert(bot.owner === s.fx.authority.ownerSubject, 'eg20_father_provisioned_owner');
      await ctx.observe('eg20_father_exact_plan', async () => ({
        setup_sha256: hash(s.view.setup_id),
        bot_sha256: hash(bot.bot_id),
        plan_sha256: s.request.action.plan_digest,
      }));
    },
  ],
  'eg',
);
export const cases = [inventory, creation, fatherProfile, fatherActions];
