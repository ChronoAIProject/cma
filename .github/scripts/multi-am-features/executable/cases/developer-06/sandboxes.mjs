import {
  define,
  setup,
  api,
  button,
  visible,
  contains,
  absent,
  capture,
  poll,
  enc,
  hash,
  key,
  peer,
} from './helpers.mjs';
const path = (ctx) => `/api/v1/sandboxes/${enc(ctx.state.f.sandboxId)}`;
async function enter(ctx, scenario) {
  const { page, origin, f } = await setup(ctx, scenario);
  await page.goto(origin + '/sandboxes/' + enc(f.sandboxId));
  await contains(ctx, page.getByRole('heading', { level: 1 }), f.sandboxName, 'sandbox_identity');
  ctx.state.before = (await api(ctx, path(ctx))).body;
}
async function retained(ctx) {
  const f = ctx.state.f;
  const after = await f.readRetained();
  ctx.assert(
    after.home_id === f.retained.home_id &&
      after.file_sha256 === f.retained.file_sha256 &&
      after.history_sha256 === f.retained.history_sha256,
    'retained_home_files_history',
  );
  await ctx.observe('retained_state', async () => ({
    before_sha256: hash(f.retained),
    after_sha256: hash(after),
  }));
}
async function menu(page, label) {
  await button(page, 'More actions').click();
  await page.getByRole('menuitem', { name: label, exact: true }).click();
}
export const cases = [
  define('sandbox.overview', [
    (ctx) => enter(ctx, 'sandbox-overview'),
    async (ctx) => {
      const { page } = ctx.state.b,
        s = ctx.state.before;
      await page.getByRole('tab', { name: 'Overview', exact: true }).click();
      for (const text of [
        'Machine resources',
        'Environment',
        `${s.workspaces.filter((w) => w.status !== 'removed').length} of ${s.limits.max_workspaces}`,
        `Up to ${s.limits.max_agents_per_workspace}`,
      ])
        await contains(ctx, page.locator('main'), text, 'sandbox_capacity_display');
      ctx.assert(s.id === ctx.state.f.sandboxId && s.status === 'active', 'sandbox_live_state');
    },
    async (ctx) => {
      const { page } = ctx.state.b;
      await page.getByRole('tab', { name: 'Workspaces', exact: true }).click();
      await page.locator(`a[href="/workspaces/${ctx.state.f.workspaceId}"]`).click();
      ctx.assert(
        new URL(page.url()).pathname === `/workspaces/${ctx.state.f.workspaceId}`,
        'workspace_navigation',
      );
    },
    async (ctx) => {
      const f = ctx.state.f;
      const w = await api(ctx, `/api/v1/workspaces/${enc(f.workspaceId)}`);
      ctx.assert(w.body.sandbox_id === f.sandboxId, 'child_parent_relationship');
      await ctx.state.b.page.goto(ctx.state.b.origin + '/sandboxes/' + enc(f.sandboxId));
      const fault = await f.failNextRead('sandbox-settings');
      ctx.defer('remove_read_fault', () => fault.close());
      await ctx.state.b.page.reload();
      await visible(ctx, button(ctx.state.b.page, 'Try again'), 'settings_failure_visible');
    },
  ]),
  define('sandbox.pause', [
    async (ctx) => {
      await enter(ctx, 'sandbox-idle');
      ctx.assert(ctx.state.before.status === 'active', 'pause_active_precondition');
    },
    async (ctx) => {
      ctx.state.accepted = await capture(ctx.state.b.page, 'POST', path(ctx) + '/pause', () =>
        button(ctx.state.b.page, 'Pause').click(),
      );
      ctx.assert([200, 202].includes(ctx.state.accepted.status), 'pause_accepted');
    },
    async (ctx) => {
      ctx.state.after = await poll(ctx, path(ctx), (s) => s.status === 'paused');
      await ctx.state.b.switchAM(peer(ctx));
      await ctx.state.b.page.reload();
      await contains(ctx, ctx.state.b.page.locator('main'), 'Paused', 'terminal_pause_visible');
      await retained(ctx);
    },
    async (ctx) => {
      ctx.assert(
        ctx.state.after.id === ctx.state.before.id && ctx.state.after.status === 'paused',
        'pause_terminal_same_sandbox',
      );
      const sibling = await api(ctx, `/api/v1/sandboxes/${enc(ctx.state.f.siblingId)}`);
      ctx.assert(sibling.body.status === ctx.state.f.siblingStatus, 'pause_sibling_unchanged');
    },
  ]),
  define('sandbox.revive', [
    async (ctx) => {
      await enter(ctx, 'sandbox-paused');
      ctx.assert(ctx.state.before.status === 'paused', 'revive_paused_precondition');
    },
    async (ctx) => {
      const r = await capture(ctx.state.b.page, 'POST', path(ctx) + '/revive', () =>
        button(ctx.state.b.page, 'Resume').click(),
      );
      ctx.assert([200, 202].includes(r.status), 'revive_accepted');
    },
    async (ctx) => {
      ctx.state.after = await poll(ctx, path(ctx), (s) => s.status === 'active');
      await ctx.state.b.switchAM(peer(ctx));
      await ctx.state.b.page.goto(ctx.state.b.origin + '/agents/' + enc(ctx.state.f.agentId));
      await contains(
        ctx,
        ctx.state.b.page.locator('main'),
        ctx.state.f.historyMarker,
        'original_conversation_visible',
      );
    },
    async (ctx) => {
      await retained(ctx);
      ctx.assert(ctx.state.after.id === ctx.state.before.id, 'revive_same_sandbox');
      const counts = await ctx.state.f.resourceIds();
      ctx.assert(hash(counts) === hash(ctx.state.f.originalResourceIds), 'revive_no_replacement_resource');
    },
  ]),
  define('sandbox.always-on-off', [
    async (ctx) => {
      await enter(ctx, 'sandbox-always-on');
      ctx.assert(ctx.state.before.always_on === true, 'approved_exception_precondition');
      await contains(
        ctx,
        ctx.state.b.page.locator('main'),
        'administrator approved Always on',
        'approved_exception_display',
      );
    },
    async (ctx) => {
      const r = await capture(ctx.state.b.page, 'POST', path(ctx) + '/always-on', () =>
        button(ctx.state.b.page, 'Turn off Always on').click(),
      );
      ctx.assert(
        r.status === 200 &&
          r.request.always_on === false &&
          r.request.expected_revision === ctx.state.before.revision,
        'always_on_expected_revision',
      );
    },
    async (ctx) => {
      await ctx.state.b.switchAM(peer(ctx));
      await ctx.state.b.page.reload();
      await contains(ctx, ctx.state.b.page.locator('main'), 'Always on is off', 'always_on_off_persisted');
    },
    async (ctx) => {
      const s = (await api(ctx, path(ctx))).body;
      ctx.assert(s.always_on === false, 'always_on_off_durable');
      await absent(
        ctx,
        button(ctx.state.b.page, /Turn on Always on|Enable Always on/),
        'owner_cannot_enable_exception',
      );
    },
  ]),
  define('sandbox.runtime-update', [
    async (ctx) => {
      await enter(ctx, 'sandbox-runtime-update');
      const f = ctx.state.f;
      ctx.assert(ctx.state.before.runtime_update.latest_version === f.targetImage, 'offered_exact_target');
      const card = ctx.state.b.page.getByRole('region', { name: 'Runtime', exact: true });
      await contains(ctx, card, f.targetImage, 'runtime_target_visible');
      await contains(ctx, card, 'when idle', 'runtime_schedule_visible');
    },
    async (ctx) => {
      const r = await capture(ctx.state.b.page, 'POST', path(ctx) + '/runtime-update', () =>
        button(ctx.state.b.page, 'Update runtime').click(),
      );
      ctx.assert(
        [200, 202].includes(r.status) && r.request.target_version === ctx.state.f.targetImage,
        'update_exact_target_admission',
      );
    },
    async (ctx) => {
      const queued = await poll(ctx, path(ctx), (s) => s.runtime_update?.state === 'queued');
      ctx.assert(queued.runtime_version === ctx.state.before.runtime_version, 'busy_work_not_replaced');
      await contains(ctx, ctx.state.b.page.locator('main'), 'Runtime update queued', 'queued_visible');
      await ctx.state.f.releaseBusy();
      const updating = await poll(ctx, path(ctx), (s) => s.runtime_update?.state === 'updating');
      ctx.assert(
        updating.runtime_update.target_version === ctx.state.f.targetImage,
        'updating_original_target',
      );
      ctx.state.after = await poll(
        ctx,
        path(ctx),
        (s) => s.runtime_update?.state === 'up_to_date' && !!s.runtime_update.completed_at,
      );
    },
    async (ctx) => {
      await retained(ctx);
      const actual = await ctx.state.f.runtimeReadback();
      ctx.assert(actual.image === ctx.state.f.targetImage, 'runtime_actual_digest');
      await ctx.observe('runtime_readback', async () => actual);
    },
    async (ctx) => {
      ctx.assert(
        ctx.state.after.runtime_version === ctx.state.f.targetImage &&
          ctx.state.after.runtime_update.completed_at,
        'runtime_completed_target',
      );
    },
  ]),
  define('sandbox.retained-recovery', [
    async (ctx) => {
      await enter(ctx, 'sandbox-retained');
      ctx.assert(ctx.state.f.retained.image === ctx.state.before.runtime_version, 'recovery_original_image');
    },
    async (ctx) => {
      const assessment = await capture(ctx.state.b.page, 'GET', path(ctx) + '/runtime-recovery', () =>
        button(ctx.state.b.page, 'Recover on the same image').click(),
      );
      ctx.assert(
        assessment.status === 200 &&
          assessment.body.admission_enabled === true &&
          ['recorded_boot_present', 'exact_boot_replacement'].includes(assessment.body.disposition),
        'fresh_recovery_assessment',
      );
      await visible(
        ctx,
        ctx.state.b.page.getByRole('dialog', { name: 'Recover on the same image' }),
        'recovery_confirmation',
      );
    },
    async (ctx) => {
      const r = await capture(ctx.state.b.page, 'POST', path(ctx) + '/runtime-recovery', () =>
        button(ctx.state.b.page, 'Recover my Sandbox').click(),
      );
      ctx.assert(
        [200, 202].includes(r.status) && r.request.expected_revision === ctx.state.before.revision,
        'recovery_exact_revision',
      );
    },
    async (ctx) => {
      ctx.state.after = await poll(ctx, path(ctx), (s) => s.status === 'active');
      await ctx.state.b.switchAM(peer(ctx));
      await ctx.state.b.page.goto(ctx.state.b.origin + '/agents/' + enc(ctx.state.f.agentId));
      await contains(
        ctx,
        ctx.state.b.page.locator('main'),
        ctx.state.f.historyMarker,
        'recovery_original_history',
      );
    },
    async (ctx) => {
      await retained(ctx);
      ctx.assert(ctx.state.after.runtime_version === ctx.state.f.retained.image, 'recovery_same_image');
      const stale = await api(ctx, path(ctx) + '/runtime-recovery', {
        method: 'POST',
        body: ctx.state.f.staleRecoveryBody,
        key: key('stale-recovery'),
        allowed: [409, 412],
      });
      ctx.assert([409, 412].includes(stale.status), 'stale_recovery_refused');
      const outsider = await api(ctx, path(ctx) + '/runtime-recovery', {
        method: 'POST',
        actor: 'outsider',
        body: ctx.state.f.currentRecoveryBody,
        key: key('unauthorized-recovery'),
        allowed: [403, 404],
      });
      ctx.assert([403, 404].includes(outsider.status), 'unauthorized_recovery_refused');
      const effects = await ctx.state.f.nativeEffects();
      ctx.assert(effects.uncertain_replayed === 0, 'uncertain_work_not_replayed');
    },
  ]),
  define('sandbox.activity', [
    async (ctx) => {
      await enter(ctx, 'sandbox-activity');
      await ctx.state.b.page.getByRole('tab', { name: 'Activity', exact: true }).click();
      await contains(
        ctx,
        ctx.state.b.page.getByRole('region', { name: 'Recorded activity' }),
        ctx.state.f.lastEventMessage,
        'activity_caught_up',
      );
      await ctx.state.b.page.reload();
      await contains(
        ctx,
        ctx.state.b.page.getByRole('region', { name: 'Recorded activity' }),
        ctx.state.f.lastEventMessage,
        'activity_refresh',
      );
    },
    async (ctx) => {
      await contains(
        ctx,
        ctx.state.b.page.locator('main'),
        ctx.state.f.progressMessage,
        'recorded_progress_visible',
      );
      const command = await api(ctx, `/api/v1/sandbox-commands/${enc(ctx.state.f.commandId)}`);
      ctx.assert(command.body.id === ctx.state.f.commandId, 'progress_operation_identity');
    },
    async (ctx) => {
      await menu(ctx.state.b.page, 'Technical details');
      const dialog = ctx.state.b.page.getByRole('dialog', { name: 'Technical details' });
      await contains(ctx, dialog, ctx.state.f.sandboxId, 'technical_sandbox_identity');
      await contains(ctx, dialog, 'Persistent storage', 'storage_fact_visible');
      await button(dialog, 'Close').click();
      await button(ctx.state.b.page, 'Dismiss recovery status').click();
      await absent(
        ctx,
        ctx.state.b.page.getByText(ctx.state.f.noticeText, { exact: true }),
        'local_notice_dismissed',
      );
    },
    async (ctx) => {
      const events = await api(ctx, path(ctx) + '/events');
      ctx.assert(
        Array.isArray(events.body) && events.body.every((e) => e.sandbox_id === ctx.state.f.sandboxId),
        'events_scoped',
      );
      const s = (await api(ctx, path(ctx))).body;
      ctx.assert(s.status === ctx.state.before.status, 'dismiss_not_recovery');
      ctx.assert(ctx.state.f.eventPages > 1, 'activity_pagination_fixture');
    },
  ]),
  define('sandbox.delete', [
    async (ctx) => {
      await enter(ctx, 'sandbox-delete');
      ctx.assert(ctx.state.f.disposable === true, 'delete_run_owned_only');
    },
    async (ctx) => {
      await menu(ctx.state.b.page, 'Delete Sandbox');
      const dialog = ctx.state.b.page.getByRole('dialog');
      await contains(ctx, dialog, ctx.state.f.sandboxName, 'delete_exact_confirmation');
      await contains(ctx, dialog, 'contained runtime resources', 'delete_affected_children');
      await dialog.getByRole('checkbox').check();
      ctx.state.accepted = await capture(ctx.state.b.page, 'DELETE', path(ctx), () =>
        button(dialog, /^Delete/).click(),
      );
      ctx.assert([200, 202, 204].includes(ctx.state.accepted.status), 'delete_accepted');
    },
    async (ctx) => {
      ctx.state.after = await poll(ctx, path(ctx), (s) => s.status === 'retired');
      const receipt = await ctx.state.f.resourceFinality();
      ctx.assert(
        receipt.sandbox_id === ctx.state.f.sandboxId &&
          receipt.children_remaining === 0 &&
          receipt.cleanup_complete === true,
        'sandbox_cleanup_terminal',
      );
      await ctx.observe('resource_finality', async () => receipt);
    },
    async (ctx) => {
      ctx.assert(ctx.state.after.status === 'retired', 'sandbox_retired');
      const sibling = (await api(ctx, `/api/v1/sandboxes/${enc(ctx.state.f.siblingId)}`)).body;
      ctx.assert(
        sibling.status === ctx.state.f.siblingStatus && sibling.id === ctx.state.f.siblingId,
        'delete_sibling_unchanged',
      );
    },
  ]),
];
