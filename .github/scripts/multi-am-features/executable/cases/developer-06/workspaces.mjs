import {
  define,
  setup,
  api,
  button,
  pick,
  visible,
  contains,
  capture,
  poll,
  enc,
  key,
  peer,
} from './helpers.mjs';
const wp = (id) => `/api/v1/workspaces/${enc(id)}`;
export const cases = ['empty', 'git_repository'].map((type) =>
  define('workspace.create-' + type, [
    async (ctx) => {
      const { page, origin, f } = await setup(ctx, type === 'empty' ? 'workspace-empty' : 'workspace-git');
      await page.goto(origin + '/sandboxes/' + enc(f.sandboxId));
      await button(page, 'More actions').click();
      await page.getByRole('menuitem', { name: 'New workspace', exact: true }).click();
      await pick(
        page,
        'Workspace source',
        type === 'empty' ? 'Empty workspace' : 'Clone a GitHub repository',
      );
      ctx.state.name = key('workspace');
      await page.getByLabel('Workspace name', { exact: true }).fill(ctx.state.name);
      if (type === 'git_repository') {
        await page.getByLabel('GitHub repository URL', { exact: true }).fill(f.repositoryUrl);
        await page.getByLabel('Branch or ref (optional)', { exact: true }).fill(f.ref);
      }
      ctx.assert(
        (await page.getByLabel('Workspace name', { exact: true }).inputValue()) === ctx.state.name,
        'workspace_name_entered',
      );
    },
    async (ctx) => {
      const r = await capture(
        ctx.state.b.page,
        'POST',
        `/api/v1/sandboxes/${enc(ctx.state.f.sandboxId)}/workspaces/inline`,
        () => button(ctx.state.b.page, 'Create workspace').click(),
      );
      ctx.assert(
        [200, 201, 202].includes(r.status) && r.body.id && r.request.source.type === type,
        'workspace_admission_exact_source',
      );
      ctx.state.created = r.body;
      // Custody is passed to the setup's public cleanup ledger before further checks.
      await ctx.state.f.registerWorkspace(r.body.id);
    },
    async (ctx) => {
      ctx.state.ready = await poll(ctx, wp(ctx.state.created.id), (w) =>
        ['ready', 'failed'].includes(w.status),
      );
      ctx.assert(ctx.state.ready.status === 'ready', 'workspace_preparation_ready');
      await ctx.state.b.switchAM(peer(ctx));
      await ctx.state.b.page.goto(ctx.state.b.origin + '/workspaces/' + enc(ctx.state.created.id));
      await contains(
        ctx,
        ctx.state.b.page.getByRole('heading', { level: 1 }),
        ctx.state.name,
        'workspace_open',
      );
    },
    async (ctx) => {
      const w = ctx.state.ready,
        f = ctx.state.f;
      ctx.assert(
        w.sandbox_id === f.sandboxId && w.source.type === type && w.name === ctx.state.name,
        'workspace_requested_parent',
      );
      if (type === 'git_repository')
        ctx.assert(
          w.source.url === f.repositoryUrl && w.source.reference === f.ref && w.resolved_commit === f.commit,
          'workspace_resolved_source',
        );
      const requests = (await ctx.state.b.traffic.flush()).filter(
        (r) =>
          r.method === 'POST' &&
          r.path === `/api/v1/sandboxes/${enc(ctx.state.f.sandboxId)}/workspaces/inline`,
      );
      ctx.assert(requests.length === 1, 'workspace_single_submit');
    },
  ]),
);
cases.push(
  define('workspace.inspect', [
    async (ctx) => {
      const { page, origin, f } = await setup(ctx, 'workspace-inspect');
      await page.goto(origin + '/workspaces/' + enc(f.workspaceId));
      await contains(ctx, page.getByRole('heading', { level: 1 }), f.workspaceName, 'workspace_identity');
    },
    async (ctx) => {
      const w = (await api(ctx, wp(ctx.state.f.workspaceId))).body;
      ctx.state.workspace = w;
      for (const value of [w.source.url, w.source.reference, w.resolved_commit, w.path])
        await contains(ctx, ctx.state.b.page.locator('main'), value, 'workspace_source_visible');
    },
    async (ctx) => {
      const w = await api(ctx, wp(ctx.state.f.workspaceId) + '/worktrees');
      ctx.assert(
        w.body.items.some((t) => t.agent_id === ctx.state.f.agentId),
        'workspace_worktree_agent',
      );
      await visible(
        ctx,
        ctx.state.b.page.getByRole('region', { name: 'Agents and Worktrees' }),
        'workspace_children_visible',
      );
    },
    async (ctx) => {
      await ctx.state.b.page.locator(`a[href="/agents/${ctx.state.f.agentId}"]`).click();
      await contains(
        ctx,
        ctx.state.b.page.locator('main'),
        ctx.state.f.historyMarker,
        'workspace_child_authorized',
      );
    },
    async (ctx) => {
      ctx.assert(ctx.state.workspace.sandbox_id === ctx.state.f.sandboxId, 'workspace_parent_correct');
      const denied = await api(ctx, wp(ctx.state.f.workspaceId), { actor: 'outsider', allowed: [403, 404] });
      ctx.assert([403, 404].includes(denied.status), 'workspace_isolation');
    },
  ]),
);
cases.push(
  define('workspace.delete', [
    async (ctx) => {
      const { page, origin, f } = await setup(ctx, 'workspace-delete');
      ctx.assert(f.disposable === true, 'workspace_disposable');
      await page.goto(origin + '/workspaces/' + enc(f.workspaceId));
      await contains(
        ctx,
        page.getByRole('heading', { level: 1 }),
        f.workspaceName,
        'workspace_delete_identity',
      );
    },
    async (ctx) => {
      const { page } = ctx.state.b;
      await button(page, 'More actions').click();
      await page.getByRole('menuitem', { name: 'Delete workspace', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await contains(ctx, dialog, ctx.state.f.workspaceName, 'workspace_delete_confirmation');
      await dialog.getByRole('checkbox').check();
      const r = await capture(page, 'DELETE', wp(ctx.state.f.workspaceId), () =>
        button(dialog, /^Delete/).click(),
      );
      ctx.assert([200, 202, 204].includes(r.status), 'workspace_delete_accepted');
    },
    async (ctx) => {
      const after = await poll(ctx, wp(ctx.state.f.workspaceId), (w) => w.status === 'removed');
      ctx.assert(after.status === 'removed', 'workspace_removed');
      const sibling = (await api(ctx, wp(ctx.state.f.siblingWorkspaceId))).body;
      ctx.assert(sibling.status === ctx.state.f.siblingWorkspaceStatus, 'workspace_sibling_unchanged');
    },
    async (ctx) => {
      const finality = await ctx.state.f.resourceFinality();
      ctx.assert(
        finality.workspace_id === ctx.state.f.workspaceId && finality.children_remaining === 0,
        'workspace_children_removed',
      );
      await ctx.observe('workspace_finality', async () => finality);
    },
  ]),
);
