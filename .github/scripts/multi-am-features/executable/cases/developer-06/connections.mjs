import {
  define,
  setup,
  api,
  button,
  visible,
  contains,
  capture,
  enc,
  hash,
  until,
  peer,
} from './helpers.mjs';
const ap = (ctx) => `/api/v1/agents/${enc(ctx.state.f.agentId)}`;
async function entry(ctx, scenario) {
  const { page, origin, f } = await setup(ctx, scenario);
  await page.goto(origin + f.entryPath);
  await button(page, f.entryAction).click();
  await visible(
    ctx,
    page.getByRole('region', { name: 'Required connections', exact: true }),
    'connection_gate_visible',
  );
}
async function check(ctx, active) {
  return api(ctx, '/auth/connections/check', {
    method: 'POST',
    body: { target: ctx.state.f.target, ...(active ? { active } : {}) },
    agentId: ctx.state.f.agentId,
  });
}
async function authorizePopup(ctx, account) {
  const { page } = ctx.state.b;
  const pending = page.waitForEvent('popup');
  await button(
    page,
    /^(Connect all|Open sign-in window|Sign in to your ChatGPT subscription( again)?)$/,
  ).click();
  const popup = await pending;
  ctx.defer('close_connection_popup', async () => {
    if (!popup.isClosed()) await popup.close();
  });
  // The local synthetic provider exposes this test-only authorization form.
  await popup.getByLabel('Test account', { exact: true }).fill(account);
  await button(popup, 'Authorize local connection').click();
}
async function noEffects(ctx) {
  const journal = await ctx.state.f.providerJournal();
  ctx.assert(journal.service_effects === 0 && journal.admissions === 0, 'unverified_connection_no_effect');
}
export const cases = [
  define('connections.check', [
    (ctx) => entry(ctx, 'connection-check'),
    async (ctx) => {
      const { page } = ctx.state.b;
      for (const label of ['NyxID services', 'Model subscription'])
        await visible(
          ctx,
          page.getByRole('heading', { name: label, exact: true }),
          'connection_kinds_visible',
        );
      const r = await check(ctx);
      ctx.assert(
        r.body.services.some((s) => s.kind === 'nyxid') && r.body.services.some((s) => s.kind === 'codex'),
        'required_service_states',
      );
      ctx.assert(
        r.body.services.some((s) => s.state !== 'satisfied'),
        'unsatisfied_precondition',
      );
    },
    async (ctx) => {
      await ctx.state.f.setProviderState('valid');
      await button(ctx.state.b.page, 'Verify again').click();
      const r = await check(ctx);
      ctx.assert(
        r.body.services.every((s) => s.state === 'satisfied') && r.body.checked_at,
        'verified_durable_services',
      );
    },
    async (ctx) => {
      for (const state of ['revoked', 'wrong-account', 'unavailable']) {
        await ctx.state.f.setProviderState(state);
        const r = await check(ctx);
        ctx.assert(
          r.body.services.some((s) => s.state !== 'satisfied'),
          'invalid_connection_not_satisfied',
        );
      }
      await noEffects(ctx);
    },
  ]),
  define('connections.connect', [
    (ctx) => entry(ctx, 'connection-connect'),
    async (ctx) => {
      await ctx.state.f.blockNextPopup(ctx.state.b.page);
      await button(ctx.state.b.page, 'Connect all').click();
      await contains(
        ctx,
        ctx.state.b.page.locator('main'),
        'Open the sign-in window to continue',
        'blocked_popup_recovery',
      );
      await authorizePopup(ctx, ctx.state.f.principal);
      const r = await check(ctx);
      ctx.assert(
        r.body.services.every((s) => s.state === 'satisfied'),
        'hosted_device_durable_verification',
      );
    },
    async (ctx) => {
      const r = await check(ctx);
      ctx.assert(
        r.body.owner === ctx.state.f.principal && r.body.services.every((s) => s.state === 'satisfied'),
        'same_user_durable_connection',
      );
    },
    async (ctx) => {
      await until(
        async () => new URL(ctx.state.b.page.url()).pathname === ctx.state.f.destinationPath,
        120000,
      );
      ctx.assert(
        new URL(ctx.state.b.page.url()).pathname === ctx.state.f.destinationPath,
        'original_operation_resumed',
      );
    },
    async (ctx) => {
      const j = await ctx.state.f.providerJournal();
      ctx.assert(
        j.admissions === 1 &&
          j.operation_id === ctx.state.f.operationId &&
          j.principal === ctx.state.f.principal,
        'original_intent_exactly_once',
      );
      await ctx.observe('connection_journal', async () => ({
        operation_sha256: hash(j.operation_id),
        admissions: j.admissions,
        principal_sha256: hash(j.principal),
      }));
    },
  ]),
  define('connections.cancel-reconnect', [
    async (ctx) => {
      await entry(ctx, 'connection-cancel');
      const r = await capture(ctx.state.b.page, 'POST', '/auth/connections/start', () =>
        button(ctx.state.b.page, 'Connect all').click(),
      );
      ctx.state.cancelled = { service_id: r.body.service_id, intent_id: r.body.intent_id };
      const cancelled = await capture(ctx.state.b.page, 'POST', '/auth/connections/cancel', () =>
        button(ctx.state.b.page, 'Cancel').click(),
      );
      ctx.assert(cancelled.status === 200 || cancelled.status === 204, 'handoff_cancelled');
    },
    async (ctx) => {
      const r = await check(ctx, ctx.state.cancelled);
      ctx.assert(r.body.active_state === 'cancelled', 'cancelled_handoff_not_authority');
      await noEffects(ctx);
      await ctx.state.f.setProviderState('expired');
      await ctx.state.b.page.goto(ctx.state.b.origin + ctx.state.f.entryPath);
      await button(ctx.state.b.page, ctx.state.f.entryAction).click();
      await visible(
        ctx,
        ctx.state.b.page.getByRole('region', { name: 'Required connections' }),
        'expired_flow_reopened',
      );
    },
    async (ctx) => {
      await authorizePopup(ctx, ctx.state.f.principal);
      const r = await check(ctx);
      ctx.assert(
        r.body.owner === ctx.state.f.principal && r.body.services.every((s) => s.state === 'satisfied'),
        'reconnected_same_account',
      );
    },
    async (ctx) => {
      const j = await ctx.state.f.providerJournal();
      ctx.assert(
        j.cancelled_intent_effects === 0 && j.expired_intent_effects === 0 && j.admissions === 1,
        'cancel_expiry_no_authorization',
      );
    },
  ]),
  define('connections.credential-recovery', [
    async (ctx) => {
      const { page, origin, f } = await setup(ctx, 'connection-credential');
      await page.goto(origin + f.entryPath);
      await button(page, f.entryAction).click();
      await visible(
        ctx,
        page.getByRole('region', { name: 'CMA sign-in required' }),
        'credential_gate_visible',
      );
    },
    async (ctx) => {
      const { page } = ctx.state.b;
      const pending = page.waitForEvent('popup');
      await button(page, 'Sign in to CMA').click();
      ctx.state.popup = await pending;
      ctx.defer('close_signin_popup', async () => {
        if (!ctx.state.popup.isClosed()) await ctx.state.popup.close();
      });
      await visible(ctx, ctx.state.popup.getByLabel('Test account', { exact: true }), 'signin_popup_real');
    },
    async (ctx) => {
      const { page } = ctx.state.b;
      await ctx.state.popup.getByLabel('Test account', { exact: true }).fill(ctx.state.f.outsiderPrincipal);
      await button(ctx.state.popup, 'Sign in locally').click();
      await button(page, 'Check signed-in account').click();
      await visible(ctx, page.getByRole('alert'), 'changed_account_refused');
      await noEffects(ctx);
      await ctx.state.f.restoreOwnerSession();
      await button(page, 'Check signed-in account').click();
      await button(page, ctx.state.f.retryAction).click();
      ctx.assert(
        (await ctx.state.f.providerJournal()).operation_id === ctx.state.f.operationId,
        'credential_recovery_preserves_intent',
      );
    },
    async (ctx) => {
      const j = await ctx.state.f.providerJournal();
      ctx.assert(
        j.admissions === 1 && j.principal === ctx.state.f.principal,
        'credential_owner_one_admission',
      );
    },
  ]),
  define('connections.scope', [
    async (ctx) => {
      const { page, origin, f } = await setup(ctx, 'connection-scope');
      await page.goto(origin + '/sandboxes/' + enc(f.sandboxId) + '?tab=access');
      ctx.state.before = (await api(ctx, ap(ctx), { agentId: f.agentId })).body;
      await visible(ctx, page.getByRole('region', { name: 'Service access' }), 'effective_scope_visible');
    },
    async (ctx) => {
      await ctx.state.b.page.getByRole('checkbox', { name: ctx.state.f.serviceLabel, exact: true }).check();
      ctx.assert(
        await ctx.state.b.page
          .getByRole('checkbox', { name: ctx.state.f.serviceLabel, exact: true })
          .isChecked(),
        'approved_service_selected',
      );
    },
    async (ctx) => {
      const r = await check(ctx);
      ctx.assert(
        r.body.services.every((s) => s.state === 'satisfied'),
        'scope_connections_verified',
      );
      ctx.state.saved = await capture(ctx.state.b.page, 'PUT', ap(ctx) + '/service-access', () =>
        button(ctx.state.b.page, 'Save scope').click(),
      );
      ctx.assert(
        ctx.state.saved.status === 200 && ctx.state.saved.request.services.includes(ctx.state.f.serviceId),
        'scope_change_admitted',
      );
    },
    async (ctx) => {
      await ctx.state.b.switchAM(peer(ctx));
      await ctx.state.b.page.reload();
      ctx.assert(
        await ctx.state.b.page
          .getByRole('checkbox', { name: ctx.state.f.serviceLabel, exact: true })
          .isChecked(),
        'scope_persisted_peer',
      );
      const r = (await api(ctx, ap(ctx), { agentId: ctx.state.f.agentId })).body;
      ctx.assert(r.reach.includes(ctx.state.f.serviceId), 'agent_effective_scope_updated');
    },
    async (ctx) => {
      await ctx.state.b.page.getByRole('checkbox', { name: ctx.state.f.serviceLabel, exact: true }).uncheck();
      await capture(ctx.state.b.page, 'PUT', ap(ctx) + '/service-access', () =>
        button(ctx.state.b.page, 'Save scope').click(),
      );
      await ctx.state.f.setProviderState('revoked');
      await ctx.state.b.page.goto(ctx.state.b.origin + '/agents/' + enc(ctx.state.f.agentId));
      await ctx.state.b.page
        .getByRole('textbox', { name: 'Message', exact: true })
        .fill(ctx.state.f.serviceUsePrompt);
      await button(ctx.state.b.page, 'Send message').click();
      const j = await ctx.state.f.providerJournal();
      ctx.assert(j.revoked_service_effects === 0, 'revoked_service_not_used');
    },
  ]),
  ...['approve', 'deny'].map((decision) =>
    define('connections.access-' + decision, [
      async (ctx) => {
        const { page, origin, f } = await setup(ctx, 'connection-access-' + decision);
        await page.goto(origin + '/agents/' + enc(f.agentId));
        await page.getByRole('textbox', { name: 'Message', exact: true }).fill(f.accessPrompt);
        await button(page, 'Send message').click();
        await visible(
          ctx,
          page.getByRole('dialog', { name: 'Agent key access', exact: true }),
          'access_request_visible',
        );
      },
      async (ctx) => {
        const r = await api(ctx, ap(ctx) + '/access-requests', { agentId: ctx.state.f.agentId });
        ctx.state.request = r.body.items.find((x) => x.status === 'pending');
        ctx.assert(ctx.state.request?.services.includes(ctx.state.f.serviceId), 'exact_requested_service');
        await contains(
          ctx,
          ctx.state.b.page.getByRole('dialog', { name: 'Agent key access' }),
          ctx.state.f.requestReason,
          'request_reason',
        );
      },
      async (ctx) => {
        const r = await capture(
          ctx.state.b.page,
          'POST',
          ap(ctx) + `/access-requests/${enc(ctx.state.request.id)}/${decision}`,
          () => button(ctx.state.b.page, decision === 'approve' ? 'Approve' : 'Deny').click(),
        );
        ctx.assert(r.status === 200 && r.body.id === ctx.state.request.id, 'exact_request_decided');
      },
      async (ctx) => {
        await until(async () => {
          const r = await api(ctx, ap(ctx) + '/access-requests', { agentId: ctx.state.f.agentId });
          ctx.state.decided = r.body.items.find((x) => x.id === ctx.state.request.id);
          return ctx.state.decided?.status === (decision === 'approve' ? 'succeeded' : 'denied');
        }, 120000);
        ctx.assert(ctx.state.decided.id === ctx.state.request.id, 'decision_identity');
      },
      async (ctx) => {
        const j = await ctx.state.f.providerJournal();
        ctx.assert(
          decision === 'approve'
            ? j.durable_verified === true && j.service_granted === true
            : j.service_granted === false,
          'decision_effective_authority',
        );
        ctx.assert(j.other_request_changes === 0, 'unrelated_request_unchanged');
      },
    ]),
  ),
];
