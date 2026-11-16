import {documentationCase} from './documentation.mjs';
import {define, openBrowser, account, visible, uiReceipt, invoke, digest, goto} from './helpers.mjs';

const accountMenu = page => page.getByRole('menu', {name: 'Account', exact: true});
const gateway = page => page.getByRole('navigation', {name: 'Event Gateway navigation'});
const dashboard = page => page.locator('[data-usage-requested-range]');

function reportCase(number, range, label, seconds) {
  return define(number, 'browser', ['browser', 'eg_browser'], async ctx => {
    let session, page, start, report;
    const responses = [], pending = [];
    await ctx.step('step_1', async () => {
      session = await openBrowser(ctx, 'user', {usage: 'populated'}); page = session.page;
      page.on('response', response => {
        const url = new URL(response.url());
        if (/\/api\/v1\/usage\/metrics\/(view|reports)/.test(url.pathname)
          && response.status() === 200 && (response.headers()['content-type'] ?? '').includes('application/json')) {
          pending.push(response.json().then(body => responses.push({url: url.pathname + url.search, body}), () => responses.push({url: url.pathname + url.search, body: {transportFailed: true}})));
        }
      });
      await account(page); await accountMenu(page).getByRole('menuitem', {name: 'Usage', exact: true}).click();
      await visible(ctx, page.getByRole('dialog', {name: 'Your usage'}), 'personal_usage_dialog');
      await visible(ctx, gateway(page), 'usage_keeps_gateway_context');
    });
    await ctx.step('step_2', async () => {
      await page.getByRole('button', {name: 'Usage time range', exact: true}).click();
      await page.getByRole('menuitemradio', {name: label, exact: true}).click();
      if (range === 'custom') {
        ctx.assert(Date.parse(session.usage.to) > Date.parse(session.usage.from)
          && Date.parse(session.usage.to) - Date.parse(session.usage.from) <= 30 * 86400000,
        'custom_range_order_bound');
        await page.getByLabel('Usage range from').fill(session.usage.from);
        await page.getByLabel('Usage range to').fill(session.usage.to);
      }
      start = performance.now();
      await page.getByRole('button', {name: 'Apply usage filters', exact: true}).click();
      ctx.assert(await dashboard(page).getAttribute('data-usage-requested-range') === range, 'usage_requested_range');
    });
    await ctx.step('step_3', async () => {
      await page.waitForFunction(expected => {
        const node = document.querySelector('[data-usage-requested-range]');
        return node?.getAttribute('data-usage-requested-range') === expected
          && node.getAttribute('data-usage-state') === 'committed'
          && !node.querySelector('[aria-busy="true"]')
          && !node.querySelector('.usage-report-time')?.textContent?.includes('refresh');
      }, range, {timeout: 90000});
      await Promise.all(pending);
      const matching = responses.filter(({body}) => {
        const r = body.report ?? body;
        return r.status === 'ready' && r.summary && r.scope?.admin === false
          && r.scope.principal === session.identity.subject
          && (range === 'custom'
            ? Number(r.query.from) === Date.parse(session.usage.fromWithZone) / 1000
              && Number(r.query.to) === Date.parse(session.usage.toWithZone) / 1000
            : Number(r.query.to) - Number(r.query.from) === seconds);
      });
      ctx.assert(matching.length > 0, 'usage_ready_exact_range_owner');
      report = matching.at(-1).body.report ?? matching.at(-1).body;
      const inventory = responses.find(r => r.url.includes(`/reports/${encodeURIComponent(report.report_id)}/sandboxes`)
        && r.body.items?.length > 0);
      ctx.assert(inventory && inventory.body.items.every(item => item.owner.subject === session.identity.subject),
        'usage_completed_report_owned_rows');
      await visible(ctx, page.getByRole('region', {name: 'Allocated resource-hours'}), 'usage_allocated_totals_visible');
      const totals = page.getByRole('region', {name: 'Allocated resource-hours'}).locator('dd');
      const values = await totals.allTextContents();
      for (const [index, field] of ['cpu_core_hours', 'memory_gib_hours', 'disk_gib_hours'].entries()) {
        const value = report.summary.allocated_period[field];
        const expected = typeof value === 'string' ? BigInt(value).toLocaleString('en-SG')
          : Number(value).toLocaleString('en-SG', {maximumFractionDigits: 3});
        ctx.assert(values[index]?.trim() === expected, 'usage_total_' + field);
      }
      await ctx.observe('usage_completed_report', async () => ({range, report_sha256: digest(report),
        report_id_sha256: digest(report.report_id), row_count: inventory.body.items.length,
        elapsed_ms: performance.now() - start}));
    });
    await ctx.step('expected_result', async () => {
      ctx.assert(await dashboard(page).getAttribute('data-usage-state') === 'committed', 'usage_not_spinner_or_preview');
      ctx.assert(report.scope.admin === false && report.scope.principal === session.identity.subject,
        'usage_personal_scope_final');
      ctx.assert(await page.getByRole('button', {name: 'Usage Sandbox status', exact: true}).count() === 0,
        'usage_admin_filter_absent');
      await uiReceipt(ctx, page);
    });
  });
}

export const moreBrowserCases = [
  define(2, 'browser', ['browser', 'eg_browser'], async ctx => {
    let owner;
    await ctx.step('step_1', async () => {
      owner = await openBrowser(ctx); await visible(ctx, gateway(owner.page), 'owner_gateway_visible');
      const actual = await invoke(owner, 'permissions');
      ctx.assert(actual.authenticated === true && actual.subject === owner.identity.subject,
        'owner_session_current_identity');
      ctx.assert(await owner.page.getByRole('button', {name: 'New channel', exact: true}).isVisible()
        === actual.canCreateChannels, 'owner_channel_action_permission');
    });
    await ctx.step('step_2', async () => {
      for (const state of ['absent', 'denied', 'expired']) {
        const session = await openBrowser(ctx, 'user', {session: state});
        const page = session.page;
        if (state === 'denied') {
          await visible(ctx, page.getByText("Your account doesn't have Event Gateway permissions.", {exact: true}),
            'session_permission_refusal');
          ctx.assert(await page.getByRole('link', {name: 'Sign in with NyxID'}).count() === 0,
            'permission_refusal_not_signin');
        } else await visible(ctx, page.getByRole('link', {name: 'Sign in with NyxID'}), 'session_requires_signin');
        ctx.assert(!await page.getByRole('button', {name: 'New channel', exact: true}).isVisible(), 'session_blocks_mutations');
      }
      for (const refusal of ['logout', 'denial']) {
        const session = await openBrowser(ctx, 'user', {session: 'delayed_success'});
        await invoke(session, 'waitForHeldSession');
        if (refusal === 'logout') {
          await account(session.page); await accountMenu(session.page).getByRole('menuitem', {name: 'Sign out', exact: true}).click();
        } else await invoke(session, 'denyPendingAction');
        await invoke(session, 'releaseHeldSession');
        await invoke(session, 'networkIdle');
        ctx.assert(!await session.page.getByRole('button', {name: 'New channel', exact: true}).isVisible(),
          'late_session_cannot_restore_access');
      }
    });
    await ctx.step('expected_result', async () => {
      await visible(ctx, gateway(owner.page), 'owner_session_remains_usable');
      await uiReceipt(ctx, owner.page);
    });
  }),
  documentationCase,
  define(9, 'browser', ['browser', 'eg_browser'], async ctx => {
    let session, page;
    await ctx.step('step_1', async () => {
      session = await openBrowser(ctx, 'user', {identityIsolation: true}); page = session.page;
      await invoke(session, 'seedVisibleDrafts');
      await account(page); await accountMenu(page).getByRole('menuitem', {name: 'Sign out', exact: true}).click();
      await visible(ctx, page.getByRole('button', {name: 'Sign in', exact: true}), 'logout_closes_authenticated_shell');
      ctx.assert(await page.getByRole('button', {name: /^Account menu for /}).count() === 0, 'logout_removes_identity');
    });
    await ctx.step('step_2', async () => {
      await goto(page, '/event-gateway');
      ctx.assert(!await page.getByRole('button', {name: 'New channel', exact: true}).isVisible(), 'logout_gateway_requires_signin');
      await goto(page, '/usage');
      ctx.assert(await dashboard(page).count() === 0, 'logout_no_usage_report');
      await invoke(session, 'signIn', {actor: 'outsider'}); await goto(page, '/event-gateway');
      await visible(ctx, page.getByRole('button', {name: `Account menu for ${session.otherIdentity.displayName}`, exact: true}),
        'new_identity_authenticated');
    });
    await ctx.step('expected_result', async () => {
      for (const marker of session.previousIdentityMarkers) {
        ctx.assert(await page.getByText(marker, {exact: true}).count() === 0, 'previous_identity_marker_hidden');
        ctx.assert(await page.locator('input').evaluateAll((nodes, value) => nodes.every(node => node.value !== value), marker),
          'previous_identity_draft_cleared');
      }
      await account(page); await accountMenu(page).getByRole('menuitem', {name: 'Usage', exact: true}).click();
      await invoke(session, 'networkIdle');
      for (const marker of session.previousIdentityMarkers) {
        ctx.assert(await page.getByText(marker, {exact: true}).count() === 0, 'previous_identity_usage_hidden');
      }
      await uiReceipt(ctx, page);
    });
  }),
  reportCase(10, '24h', 'Last 24 hours', 86400),
  reportCase(11, '7d', 'Last 7 days', 604800),
  reportCase(12, '30d', 'Last 30 days', 2592000),
  reportCase(13, '1h', 'Last hour', 3600),
  reportCase(14, 'custom', 'Custom (up to 30 days)'),
];
