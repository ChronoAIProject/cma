import { randomUUID } from 'node:crypto';
import { Blocked } from '../../runner.mjs';
import { prepareBrowser, requireBrowserInputs } from './setup-browser.mjs';
import { requireResourceInputs, resources } from './setup-resources.mjs';

async function taskDraft(ctx, primitives) {
  requireBrowserInputs(primitives, ['user', 'outsider']);
  requireResourceInputs(primitives);
  const user = await prepareBrowser(ctx, primitives);
  const outsider = await prepareBrowser(ctx, primitives, { actor: 'outsider' });
  ctx.assert(
    user.capabilities.includes('create_sandbox') &&
      outsider.capabilities.includes('create_sandbox'),
    'draft_create_capabilities',
  );
  const owned = await resources(ctx, primitives);
  const sandbox = await owned.sandbox();
  return {
    ...user,
    draft: 'Draft only ' + randomUUID(),
    otherLocationPath: '/sandboxes/' + encodeURIComponent(sandbox.id),
    otherActor: 'outsider',
    otherSession: outsider.session,
    cleanup: async () => {
      await owned.cleanup();
      await outsider.cleanup();
      await user.cleanup();
    },
  };
}

async function publicAgent(ctx, primitives) {
  requireBrowserInputs(primitives);
  requireResourceInputs(primitives);
  const verified = await prepareBrowser(ctx, primitives);
  ctx.assert(verified.capabilities.includes('create_sandbox'), 'agent_create_capability');
  const owned = await resources(ctx, primitives);
  const hierarchy = await owned.hierarchy();
  // Bind a fresh browser proxy to the actual Agent for serving/owner receipts.
  const browser = await prepareBrowser(ctx, primitives, { agentId: hierarchy.agentId });
  await verified.cleanup();
  return {
    ...hierarchy,
    ...browser,
    cleanup: async () => {
      await owned.cleanup();
      await browser.cleanup();
    },
  };
}

async function navigationTasks(ctx, primitives) {
  requireBrowserInputs(primitives, ['user', 'outsider']);
  const config = requireResourceInputs(primitives);
  if (typeof config.navigationReadFault !== 'function')
    throw new Blocked('navigation_read_fault_required');
  const foreignBrowser = await prepareBrowser(ctx, primitives, { actor: 'outsider' });
  const fixture = await publicAgent(ctx, primitives);
  const owned = await resources(ctx, primitives);
  const foreign = await owned.sandbox('outsider');
  // A distinct actor is not necessarily private: refuse admin visibility here.
  if (fixture.capabilities.includes('read_all_sandboxes'))
    throw new Blocked('navigation_unprivileged_user_required');
  const denied = await ctx.request({
    method: 'GET',
    am: ctx.am,
    path: '/api/v1/sandboxes/' + encodeURIComponent(foreign.id),
    allowed: [403, 404],
  });
  ctx.assert([403, 404].includes(denied.status), 'navigation_foreign_private');
  const workspace = await owned.read(
    '/api/v1/workspaces/' + encodeURIComponent(fixture.workspace.id),
  );
  const projected = workspace.agents?.find((agent) => agent.id === fixture.agentId);
  ctx.assert(projected?.task_status === 'completed', 'navigation_completed_task_projection');
  let fault, restoration;
  const restore = () => (fault ? (restoration ??= fault.restore()) : Promise.resolve());
  ctx.defer('restore_navigation_read_fault', restore);
  return {
    ...fixture,
    agent: { ...fixture.agent, statusLabel: 'Completed' },
    forbiddenNames: [foreign.name],
    faults: {
      async failNextNavigationRead() {
        if (fault) throw Error('navigation_fault_already_armed');
        fault = await config.navigationReadFault({
          page: fixture.session.page,
          method: 'GET',
          count: 1,
          status: 503,
          path: '/api/v1/sandboxes',
        });
        ctx.assert(typeof fault?.restore === 'function', 'navigation_fault_cleanup_required');
      },
    },
    cleanup: async () => {
      await restore();
      await owned.cleanup();
      await fixture.cleanup();
      await foreignBrowser.cleanup();
    },
  };
}

export const setups = {
  'account-preferences': (ctx, primitives) => prepareBrowser(ctx, primitives, { clipboard: true }),
  'task-draft': taskDraft,
  'agent-public-ready': publicAgent,
  'navigation-tasks': navigationTasks,
};
