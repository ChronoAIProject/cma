import {
  absent,
  account,
  agentPath,
  button,
  define,
  go,
  observeDOM,
  peer,
  pick,
  readAgent,
  ready,
  setup,
  shown,
  text,
} from './helpers.mjs';

async function nodes(ctx, s, expected) {
  const ids = await s.page
    .locator('[role="treeitem"]:visible')
    .evaluateAll((rows) => rows.map((row) => row.getAttribute('data-node-id')));
  ctx.assert(new Set(ids).size === ids.length, 'navigation_no_duplicate_resources');
  if (expected)
    ctx.assert(
      JSON.stringify([...ids].sort()) === JSON.stringify([...expected].sort()),
      'navigation_scope_status_exact',
    );
  for (const name of s.data.forbiddenNames)
    await absent(
      ctx,
      s.page.getByRole('treeitem').filter({ hasText: name }),
      'unauthorized_inventory_absent',
    );
  await ctx.observe('navigation_nodes', async () => ids);
}
const tasks = define('navigation.tasks', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'navigation-hierarchy');
    await go(s.page, '/agent-manager');
    await shown(
      ctx,
      s.page.locator('.navigation-filters__heading').filter({ hasText: /^Tasks$/ }),
      'tasks_navigation_visible',
    );
    await shown(
      ctx,
      s.page.locator(`a[href="/agents/${s.data.agent.id}"]:visible`),
      'authorized_task_visible',
    );
    ctx.assert(
      (
        await s.page
          .locator(`a[href="/agents/${s.data.agent.id}"]:visible`)
          .getAttribute('aria-label')
      ).includes(s.data.agent.statusLabel),
      'task_lifecycle_state_visible',
    );
  },
  async (ctx, s) => {
    await button(s.page, 'Filter').click();
    await button(s.page, 'Manage environments').click();
    await shown(ctx, button(s.page, 'Back to tasks'), 'environments_navigation_active');
  },
  async (ctx, s) => {
    for (const resource of [s.data.sandbox, s.data.workspace]) {
      const collapse = button(s.page, `Collapse ${resource.name}`);
      if (!(await collapse.isVisible())) await button(s.page, `Expand ${resource.name}`).click();
      await shown(ctx, button(s.page, `Collapse ${resource.name}`), 'hierarchy_expanded');
    }
    await shown(
      ctx,
      s.page.locator(`a[href="/agents/${s.data.agent.id}"]:visible`),
      'workspace_agent_visible',
    );
    await nodes(ctx, s);
  },
  async (ctx, s) => {
    await button(s.page, 'Back to tasks').click();
    await s.data.faults.failNextNavigationRead();
    await s.page.reload();
    await shown(
      ctx,
      s.page.getByRole('alert').filter({ has: button(s.page, 'Try again') }),
      'navigation_failure_actionable',
    );
    await button(s.page, 'Try again').click();
    await shown(
      ctx,
      s.page.locator(`a[href="/agents/${s.data.agent.id}"]:visible`),
      'navigation_retry_restores_task',
    );
  },
  async (ctx, s) => {
    await peer(ctx, s);
    await nodes(ctx, s);
    ctx.assert(new URL(s.page.url()).pathname === '/agent-manager', 'task_selection_preserved');
  },
]);
const filters = define('navigation.filters', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'navigation-hierarchy');
    await go(s.page, '/agent-manager');
    await button(s.page, 'Filter').click();
    ctx.assert(s.data.navigationScopes.length >= 2, 'multiple_scope_fixture');
    await pick(s.page, 'Navigation scope', s.data.navigationScopes[0].label);
    ctx.assert(
      (await button(s.page, 'Navigation scope').innerText()) === s.data.navigationScopes[0].label,
      'authorized_scope_selected',
    );
  },
  async (ctx, s) => {
    for (const scope of s.data.navigationScopes) {
      await pick(s.page, 'Navigation scope', scope.label);
      ctx.assert(scope.statuses.length >= 2, 'scope_status_variants_available');
      for (const status of scope.statuses) {
        await pick(s.page, 'Agent status filter', status.label);
        await nodes(ctx, s, status.nodeIds);
        for (const label of scope.forbiddenWriteLabels)
          await absent(ctx, button(s.page, label), 'previous_scope_write_authority_removed');
      }
    }
  },
  async (ctx, s) => {
    // Navigation inventory paging is implemented by the Agents view, with the same selected scope.
    await go(s.page, s.data.navigationScopes.at(-1).inventoryPath);
    await inventoryRows(ctx, s, s.data.navigationScopes.at(-1).firstPageIds);
    await button(s.page, 'Next page').click();
    await shown(ctx, text(s.page, 'Page 2'), 'scope_next_page');
    await inventoryRows(ctx, s, s.data.navigationScopes.at(-1).secondPageIds);
    await button(s.page, 'Previous page').click();
    await shown(ctx, text(s.page, 'Page 1'), 'scope_previous_page');
  },
  async (ctx, s) => {
    await go(s.page, '/agent-manager');
    await button(s.page, 'Filter').click();
    const scope = s.data.navigationScopes.at(-1);
    await pick(s.page, 'Navigation scope', scope.label);
    await pick(s.page, 'Agent status filter', scope.statuses.at(-1).label);
    await button(s.page, 'Clear filters').click();
    ctx.assert(
      (await button(s.page, 'Navigation scope').innerText()) === 'Personal',
      'filters_reset_scope',
    );
    ctx.assert(
      (await button(s.page, 'Agent status filter').innerText()) === 'All statuses',
      'filters_reset_status',
    );
  },
  async (ctx, s) => {
    await peer(ctx, s);
    await nodes(ctx, s);
  },
]);
async function inventoryRows(ctx, s, expected) {
  const rows = s.page.getByRole('list', { name: 'Agents in this scope' }).locator('li');
  if (expected.length) await rows.first().waitFor({ state: 'visible' });
  else
    await shown(
      ctx,
      s.page.getByText(/No (Agents match|matches on this page)/),
      'empty_inventory_explicit',
    );
  const actual = await rows.evaluateAll((items) =>
    items.map((item) => ({
      id: decodeURIComponent(new URL(item.querySelector('a').href).pathname.split('/').at(-1)),
      text: item.innerText,
    })),
  );
  ctx.assert(
    JSON.stringify(actual.map((row) => row.id)) ===
      JSON.stringify(expected.map((row) => (typeof row === 'string' ? row : row.id))),
    'inventory_exact_ids_order',
  );
  for (const expectedRow of expected.filter((row) => typeof row !== 'string')) {
    const content = actual.find((row) => row.id === expectedRow.id).text;
    for (const value of [
      expectedRow.title,
      expectedRow.statusLabel,
      expectedRow.originLabel,
      expectedRow.channelLabel,
    ])
      ctx.assert(content.includes(value), 'inventory_exact_provenance');
  }
  await ctx.observe('inventory_readback', async () => actual);
}
const inventory = define('navigation.inventory', 'agent', [
  async (ctx, s) => {
    await setup(ctx, s, 'navigation-inventory');
    await go(s.page, '/agents');
    await shown(
      ctx,
      s.page.getByRole('heading', { name: 'Agents', exact: true }),
      'agents_inventory_open',
    );
  },
  async (ctx, s) => {
    ctx.assert(
      JSON.stringify(s.data.filterGroups.map((group) => group.label).sort()) ===
        JSON.stringify(['Agent status', 'Channel activity', 'Creation origin']),
      'all_inventory_filter_groups',
    );
    for (const group of s.data.filterGroups) {
      await button(s.page, group.label).click();
      const offered = await s.page
        .getByRole('menu', { name: group.label, exact: true })
        .getByRole('menuitemradio')
        .allTextContents();
      ctx.assert(
        JSON.stringify(offered.sort()) ===
          JSON.stringify(group.options.map((option) => option.label).sort()),
        'all_offered_inventory_filters_covered',
      );
      await s.page.keyboard.press('Escape');
      for (const option of group.options) {
        await pick(s.page, group.label, option.label);
        await inventoryRows(ctx, s, option.rows);
      }
      await pick(s.page, group.label, group.reset);
    }
  },
  async (ctx, s) => {
    await inventoryRows(ctx, s, s.data.firstPageIds);
    await button(s.page, 'Next page').click();
    await inventoryRows(ctx, s, s.data.secondPageIds);
    await button(s.page, 'Previous page').click();
    await inventoryRows(ctx, s, s.data.firstPageIds);
  },
  async (ctx, s) => {
    await s.page.getByRole('link', { name: s.data.agentName, exact: true }).click();
    await ready(ctx, s);
    ctx.assert(new URL(s.page.url()).pathname === agentPath(s.data.agentId), 'listed_agent_opened');
  },
  async (ctx, s) => {
    await peer(ctx, s);
    await readAgent(ctx, s);
    await ready(ctx, s);
  },
]);
async function commandMenu(s) {
  await s.page.keyboard.press('ControlOrMeta+k');
  await s.page.getByRole('combobox', { name: 'Search commands' }).waitFor();
}
const search = define('navigation.search', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'navigation-hierarchy');
    await go(s.page, '/agent-manager');
    await commandMenu(s);
    await shown(ctx, s.page.getByRole('listbox', { name: 'Results' }), 'command_menu_open');
  },
  async (ctx, s) => {
    for (const resource of [s.data.agent, s.data.sandbox, s.data.workspace]) {
      await s.page.getByRole('combobox', { name: 'Search commands' }).fill(resource.name);
      await shown(
        ctx,
        s.page.getByRole('option').filter({
          has: s.page.locator('.command-menu__label').filter({ hasText: resource.name }),
        }),
        'authorized_search_result',
      );
    }
    for (const forbidden of s.data.forbiddenNames) {
      await s.page.getByRole('combobox', { name: 'Search commands' }).fill(forbidden);
      await absent(
        ctx,
        s.page.getByRole('option').filter({ hasText: forbidden }),
        'deleted_retired_foreign_search_absent',
      );
    }
  },
  async (ctx, s) => {
    for (const [resource, route] of [
      [s.data.agent, '/agents/'],
      [s.data.sandbox, '/sandboxes/'],
      [s.data.workspace, '/workspaces/'],
    ]) {
      await s.page.getByRole('combobox', { name: 'Search commands' }).fill(resource.name);
      await s.page
        .getByRole('option')
        .filter({ has: s.page.locator('.command-menu__label').filter({ hasText: resource.name }) })
        .click();
      ctx.assert(
        new URL(s.page.url()).pathname === route + resource.id,
        'search_destination_identity',
      );
      await commandMenu(s);
    }
  },
  async (ctx, s) => {
    await s.page.getByRole('combobox', { name: 'Search commands' }).fill('');
    for (const label of s.data.allowedCommands)
      await shown(
        ctx,
        s.page.getByRole('option').filter({
          has: s.page
            .locator('.command-menu__label')
            .filter({ hasText: new RegExp('^' + label + '$') }),
        }),
        'permitted_command_visible',
      );
    for (const label of s.data.forbiddenCommands)
      await absent(
        ctx,
        s.page.getByRole('option').filter({ hasText: label }),
        'capability_gated_command_absent',
      );
  },
  async (ctx, s) => {
    await observeDOM(
      ctx,
      s,
      'authorized_command_menu',
      s.page.getByRole('listbox', { name: 'Results' }),
    );
  },
]);
const docs = define('navigation.docs', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'account-preferences');
    await go(s.page, '/agent-manager');
    await (await account(s.page)).getByRole('menuitem', { name: 'Docs', exact: true }).click();
    await shown(
      ctx,
      s.page.getByRole('heading', { name: 'Docs', exact: true }),
      'docs_index_visible',
    );
  },
  async (ctx, s) => {
    await s.page.getByRole('link', { name: /Event Gateway Protocol/ }).click();
    await shown(
      ctx,
      s.page.getByRole('heading', { name: 'Event Gateway Protocol', exact: true }),
      'protocol_documentation_visible',
    );
    ctx.assert(new URL(s.page.url()).pathname === '/docs/event-gateway', 'docs_view_route');
  },
  async (ctx, s) => {
    await s.page.goBack();
    await s.page.goBack();
    ctx.assert(
      new URL(s.page.url()).pathname === '/agent-manager',
      'docs_previous_resource_restored',
    );
    await shown(ctx, button(s.page, /^Account menu for /), 'navigation_still_usable');
  },
  async (ctx, s) => {
    await peer(ctx, s, '/docs/event-gateway');
    await observeDOM(ctx, s, 'documentation_content', s.page.locator('.gateway-docs'));
  },
]);
const legacy = define('navigation.legacy-routes', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'account-preferences');
    s.routes = [
      '/settings',
      '/settings/general',
      '/settings/usage',
      '/developer',
      '/profiles',
      '/profiles/agent-profiles/retired',
      '/marketplace',
      '/marketplace/agent/retired',
    ];
    await go(s.page, s.routes[0]);
    await shown(ctx, s.page.getByRole('dialog', { name: 'Your usage' }), 'legacy_settings_usage');
  },
  async (ctx, s) => {
    for (const path of s.routes) {
      await go(s.page, path);
      if (path.startsWith('/settings')) {
        await shown(ctx, s.page.getByRole('dialog', { name: 'Your usage' }), 'legacy_usage_dialog');
        ctx.assert(new URL(s.page.url()).pathname === '/usage', 'legacy_usage_redirect');
        if (path !== '/settings/usage')
          await shown(
            ctx,
            text(s.page, 'Appearance is now in your account menu.'),
            'appearance_migration_notice',
          );
      } else if (path === '/developer') {
        await shown(
          ctx,
          s.page.getByRole('heading', { name: 'Docs', exact: true }),
          'developer_docs_redirect',
        );
        ctx.assert(new URL(s.page.url()).pathname === '/docs', 'canonical_docs_route');
      } else
        await shown(
          ctx,
          s.page.getByRole('heading', { name: 'Configure Agents directly' }),
          'profile_retirement_notice',
        );
    }
  },
  async (ctx, s) => {
    for (const path of [
      '/image-manager',
      '/unknown-c275',
      '/agents/%ZZ',
      '/agents/too/many/segments',
      '/docs/',
    ]) {
      await go(s.page, path);
      await shown(
        ctx,
        s.page.getByRole('heading', { name: 'Page not found' }),
        'invalid_route_not_found',
      );
    }
  },
  async (ctx, s) => {
    await peer(ctx, s, '/developer');
    await observeDOM(
      ctx,
      s,
      'legacy_route_peer',
      s.page.getByRole('heading', { name: 'Docs', exact: true }),
    );
  },
]);
export const cases = [tasks, filters, inventory, search, docs, legacy];
