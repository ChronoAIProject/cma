import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import type { Agent, SandboxDetail, WorkspaceDetail } from '../src/api-client/index';

test('LOCAL Agent deletion retains quota until separately confirmed Sandbox retirement', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const parents: SandboxDetail[] = [];
  const workspaces: WorkspaceDetail[] = [];
  const agents: Agent[] = [];
  const writes: string[] = [];
  const unexpected: string[] = [];
  let sandboxDeletionKey: string | undefined;
  let committedRetirement = false;
  for (let index = 0; index < 10; index++) {
    const id = String(index).padStart(2, '0');
    const sandboxId = `sbx_quota_${id}`;
    const workspaceId = `wks_quota_${id}`;
    const agentId = `agt_quota_${id}`;
    const worktreeId = `wkt_quota_${id}`;
    const status = index < 8 ? 'active' : 'paused';
    const agent: Agent = {
      ...structuredClone(boundary.agent),
      id: agentId,
      title: `Agent ${id}`,
      status: 'stopped',
      worktree: { ...boundary.agent.worktree, id: worktreeId },
      workspace: { ...boundary.agent.workspace, id: workspaceId },
      sandbox: {
        ...boundary.agent.sandbox,
        id: sandboxId,
        name: `Environment ${id}`,
        status,
      },
    };
    const workspace: WorkspaceDetail = {
      ...structuredClone(boundary.workspace),
      id: workspaceId,
      sandbox_id: sandboxId,
      agents: [
        {
          ...boundary.workspace.agents[0]!,
          id: agentId,
          title: agent.title,
          status: 'stopped',
          sandbox_id: sandboxId,
          workspace_id: workspaceId,
          worktree_id: worktreeId,
        },
      ],
      worktrees: [
        {
          ...boundary.workspace.worktrees[0]!,
          id: worktreeId,
          agent_id: agentId,
          workspace_id: workspaceId,
          sandbox_id: sandboxId,
        },
      ],
    };
    parents.push({
      ...structuredClone(boundary.sandbox),
      id: sandboxId,
      name: `Environment ${id}`,
      status,
      lifecycle: { pause: { allowed: false }, revive: { allowed: true } },
      workspaces: [{ ...boundary.sandbox.workspaces[0]!, id: workspaceId, sandbox_id: sandboxId }],
    });
    agents.push(agent);
    workspaces.push(workspace);
  }
  await page.route('**/api/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const reply = (json: unknown, status = 200) => route.fulfill({ json, status });
    if (path === '/api/v2/ui/task-experience')
      return reply({
        object: 'cma.ui.task_experience',
        schema_version: 1,
        account_id: 'local-owner',
        capabilities: {
          task_home_admission: true,
          desktop_enable: false,
          suggested_followup_admission: false,
          browser_takeover: false,
          replay_presentation: false,
        },
      });
    if (path === '/api/v1/sandboxes/capacity') {
      const counted = parents.filter((parent) => parent.status !== 'retired');
      const live = counted.length;
      const counts: Record<string, number> = {};
      for (const parent of counted) counts[parent.status] = (counts[parent.status] ?? 0) + 1;
      return reply({
        resource: 'sandbox',
        limit: 10,
        live,
        available: 10 - live,
        counts_by_status: counts,
        observed_at: new Date().toISOString(),
      });
    }
    if (path === '/api/v1/sandboxes') {
      const items = parents.filter((parent) => parent.status !== 'retired');
      return reply(
        url.searchParams.has('cursor')
          ? { items: items.slice(5) }
          : { items: items.slice(0, 5), next: 'page-two' },
      );
    }
    const sandbox = parents.find((parent) => path === `/api/v1/sandboxes/${parent.id}`);
    if (sandbox) {
      if (request.method() === 'DELETE') {
        writes.push(`DELETE ${sandbox.id}`);
        sandboxDeletionKey = request.headers()['idempotency-key'];
        const next = { ...sandbox, status: 'retiring' as const };
        parents[parents.indexOf(sandbox)] = next;
        return reply(next, 202);
      }
      if (sandbox.status === 'retiring' && committedRetirement) {
        const next = { ...sandbox, status: 'retired' as const };
        parents[parents.indexOf(sandbox)] = next;
        return reply(next);
      }
      return reply(sandbox);
    }
    if (/\/sandboxes\/sbx_quota_\d+\/events$/.test(path)) return reply([]);
    if (/\/resource-access\/(sandbox|agent)\/(sbx|agt)_quota_\d+$/.test(path))
      return reply({
        capabilities: ['view_details', 'view_history', 'manage_sharing'],
        owner: { kind: 'person', is_current_person: true },
        revalidate_at: null,
        policy_revision: 1,
      });
    const workspace = workspaces.find((item) => path === `/api/v1/workspaces/${item.id}`);
    if (workspace) return reply(workspace);
    const agent = agents.find((item) => path === `/api/v1/agents/${item.id}`);
    if (agent) {
      if (request.method() === 'DELETE') {
        writes.push(`DELETE ${agent.id}`);
        const deleted = {
          ...agent,
          status: 'deleted' as const,
          worktree: { ...agent.worktree, status: 'removed' as const },
        };
        agents[agents.indexOf(agent)] = deleted;
        const workspace = workspaces.find((item) => item.id === agent.workspace.id)!;
        workspaces[workspaces.indexOf(workspace)] = {
          ...workspace,
          agents: [],
          worktrees: [],
          agent_count: 0,
          worktree_count: 0,
        };
        const parent = parents.find((item) => item.id === agent.sandbox.id)!;
        parents[parents.indexOf(parent)] = { ...parent, agent_count: 0, worktree_count: 0 };
        return reply(deleted, 202);
      }
      return reply(agent);
    }
    if (/\/agents\/agt_quota_\d+\/events$/.test(path)) return reply({ items: [] });
    if (/\/agents\/agt_quota_\d+\/provenance$/.test(path))
      return reply({
        agent_id: path.split('/')[4],
        created_at: localTime,
        creation_origin: 'legacy_unknown',
      });
    if (/\/agents\/agt_quota_\d+\/transcript$/.test(path))
      return reply({ agent_id: path.split('/')[4], through_seq: 0, turns: [] });
    if (/\/agents\/agt_quota_\d+\/usage$/.test(path))
      return reply({
        agent_id: path.split('/')[4],
        through_seq: 0,
        input_tokens: 0,
        output_tokens: 0,
        context_used: 0,
      });
    if (/\/agents\/agt_quota_\d+\/channel-usage$/.test(path))
      return reply({
        agent_id: path.split('/')[4],
        channel_usage: {
          active: false,
          has_history: false,
          first_used_at: null,
          last_used_at: null,
        },
      });
    if (path === '/api/agent-manager/v1/agents') {
      expect(request.method()).toBe('POST');
      expect(parents.filter((parent) => parent.status !== 'retired')).toHaveLength(9);
      writes.push('POST task');
      return reply({ id: 'new-capacity-task', status: 'provisioning' }, 202);
    }
    if (path === '/api/agent-manager/v1/agent-lifecycle/new-capacity-task')
      return reply({ id: 'new-capacity-task', status: 'queued', phase: 'queued', events: [] });
    if (/\/agents\/agt_quota_\d+\/reply-preview$/.test(path))
      return reply({ code: 'not_found' }, 404);
    if (path.includes('quota_')) {
      unexpected.push(`${request.method()} ${path}`);
      return reply({ code: 'unavailable' }, 503);
    }
    return route.fallback();
  });
  await page.goto('/');
  const capacity = page.getByRole('region', { name: 'Sandbox capacity' });
  await expect(capacity.getByText('Environment limit reached')).toBeVisible();
  await page.getByLabel('Your task').fill('Use the next available environment');
  await expect(page.getByRole('button', { name: 'Start task' })).toBeDisabled();
  await page.screenshot({ path: info.outputPath('full-capacity.png'), fullPage: true });
  for (const id of ['00', '01']) {
    await page.getByRole('treeitem', { name: new RegExp(`^Agent ${id},`) }).hover();
    await page.getByRole('button', { name: `Options for Agent ${id}` }).click();
    await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
    const confirmation = page.getByRole('dialog', { name: 'Delete Agent', exact: true });
    await expect(confirmation.getByText(/Its Sandbox environment remains/)).toBeVisible();
    await confirmation.getByLabel('I understand this Agent will be deleted.').check();
    await confirmation.getByRole('button', { name: 'Delete Agent', exact: true }).click();
    const completed = page.getByRole('dialog', { name: 'Agent deleted', exact: true });
    await expect(completed.getByText(/Its Sandbox environment is retained/)).toBeVisible();
    await completed.getByRole('button', { name: 'Done', exact: true }).click();
  }
  expect(agents.filter((agent) => agent.status !== 'deleted')).toHaveLength(8);
  expect(parents).toHaveLength(10);
  await page.reload();
  await expect(capacity.getByText('Environment limit reached')).toBeVisible();
  await expect(
    page.getByRole('tree', { name: 'Personal Agents' }).getByRole('treeitem'),
  ).toHaveCount(8);
  await expect(page.getByLabel('Your task')).toHaveValue('Use the next available environment');
  await capacity.getByRole('button', { name: 'Environments', exact: true }).click();
  const inventory = page.getByRole('complementary', { name: 'Application navigation' });
  await expect(inventory.getByRole('treeitem', { level: 1 })).toHaveCount(10);
  await page.screenshot({ path: info.outputPath('environment-inventory.png'), fullPage: true });
  await inventory.getByRole('link', { name: /^Environment 00 Live$/ }).click();
  await page.getByRole('button', { name: 'More actions', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Delete Sandbox', exact: true }).click();
  const confirmation = page.getByRole('dialog', { name: 'Delete Sandbox', exact: true });
  await confirmation.getByLabel('I understand this Sandbox will be deleted.').check();
  await confirmation.getByRole('button', { name: 'Request deletion', exact: true }).click();
  await expect.poll(() => writes.includes('DELETE sbx_quota_00')).toBe(true);
  expect(sandboxDeletionKey).toBeTruthy();
  const retainedDeletion = () =>
    page.evaluate(
      (key) =>
        JSON.parse(
          localStorage.getItem(`cma:deletion-requests:local-owner:${encodeURIComponent(key)}`) ??
            'null',
        ),
      sandboxDeletionKey!,
    );
  const acceptedDeletion = {
    action: { kind: 'delete_sandbox', sandboxId: 'sbx_quota_00' },
    key: sandboxDeletionKey,
    accepted: true,
  };
  await expect.poll(retainedDeletion).toEqual(acceptedDeletion);
  await page.goto('/');
  await expect(capacity.getByText('Environment limit reached')).toBeVisible();
  await expect(page.getByRole('region', { name: 'Deletion request', exact: true })).toHaveCount(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.getByRole('button', { name: 'View deletion status', exact: true }).click();
  const progress = page.getByRole('dialog', { name: 'Deletion progress', exact: true });
  await expect(progress.getByRole('listitem')).toHaveCount(1);
  await expect(progress.getByRole('code')).toHaveText(acceptedDeletion.action.sandboxId);
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await retainedDeletion()).toEqual(acceptedDeletion);
  await expect(page.getByLabel('Your task')).toHaveValue('Use the next available environment');
  await expect(page.getByRole('button', { name: 'Start task' })).toBeDisabled();
  await expect(capacity).toHaveAttribute('data-state', 'full');
  expect(parents.filter((parent) => parent.status !== 'retired')).toHaveLength(10);
  committedRetirement = true;
  parents[0] = { ...parents[0]!, status: 'retired' };
  const committedAt = Date.now();
  await expect(capacity.getByText('1 slot left')).toBeVisible({
    timeout: 15_000,
  });
  const elapsed = Date.now() - committedAt;
  expect(elapsed).toBeLessThan(15_000);
  await expect(page.getByRole('button', { name: 'Start task' })).toBeEnabled();
  await page.getByRole('button', { name: 'Start task' }).click();
  await expect(page.getByRole('region', { name: 'Original request', exact: true })).toBeVisible();
  expect(writes).toEqual([
    'DELETE agt_quota_00',
    'DELETE agt_quota_01',
    'DELETE sbx_quota_00',
    'POST task',
  ]);
  expect(unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
  await info.attach('retirement-observation', {
    body: JSON.stringify({
      elapsed_ms: elapsed,
      source: 'synthetic public boundary',
      observed_at: localTime,
    }),
    contentType: 'application/json',
  });
  console.log(`Committed retirement observed after ${elapsed} ms (synthetic public boundary).`);
});
