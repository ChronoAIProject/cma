import { registerDeletionConfirmationCases } from './redesign-deletion.cases';
import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import {
  environments,
  expand,
  insideViewport,
  navigation,
  noEffects,
  resourceRow,
} from './resource-lifecycle.fixture';

for (const width of [1440, 390]) {
  for (const kind of ['Sandbox', 'Workspace', 'Agent'] as const) {
    test(`LOCAL ${kind} deletion preserves its key through reload and keeps its sibling at ${width}px`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      const boundary = new LocalStartup();
      boundary.agent = { ...boundary.agent, status: 'stopped' };
      boundary.workspace = {
        ...boundary.workspace,
        agents: boundary.workspace.agents.map((agent) => ({ ...agent, status: 'stopped' })),
      };
      boundary.sandbox = {
        ...boundary.sandbox,
        lifecycle: { pause: { allowed: true }, revive: { allowed: false, reason: 'not_paused' } },
      };
      const siblingSandbox = {
        ...boundary.sandbox,
        id: 'sbx_sibling',
        name: 'Sibling Sandbox',
        workspaces: [],
      };
      const siblingWorkspace = {
        ...boundary.workspace,
        id: 'wks_sibling',
        name: 'Sibling Workspace',
        agents: [],
        worktrees: [],
        agent_count: 0,
        worktree_count: 0,
      };
      const siblingAgent = {
        ...boundary.workspace.agents[0]!,
        id: 'agt_sibling',
        worktree_id: 'wkt_sibling',
        title: 'Sibling Agent',
      };
      if (kind === 'Agent')
        boundary.workspace = {
          ...boundary.workspace,
          agents: [...boundary.workspace.agents, siblingAgent],
        };
      boundary.sandbox = {
        ...boundary.sandbox,
        workspaces: [boundary.workspace, ...(kind === 'Workspace' ? [siblingWorkspace] : [])],
      };
      await boundary.route(page, baseURL!);
      if (kind === 'Sandbox')
        await page.route(
          (url) => url.origin === baseURL && url.pathname === '/api/v1/sandboxes',
          (route) => {
            expect(route.request().method()).toBe('GET');
            return route.fulfill({ json: { items: [boundary.sandbox, siblingSandbox] } });
          },
        );
      const siblingReads: Record<string, () => unknown> = {
        '/api/v1/sandboxes/sbx_sibling': () => siblingSandbox,
        '/api/v1/sandboxes/sbx_sibling/workspaces': () => ({ items: [] }),
        '/api/v1/workspaces/wks_sibling': () => siblingWorkspace,
        '/api/v1/workspaces/wks_sibling/worktrees': () => ({ items: [] }),
        '/api/v1/agents/agt_sibling': () => ({
          ...boundary.agent,
          id: siblingAgent.id,
          title: siblingAgent.title,
          status: siblingAgent.status,
          worktree: {
            ...boundary.agent.worktree,
            id: siblingAgent.worktree_id,
            agent_id: siblingAgent.id,
            status: 'ready',
          },
        }),
        '/api/v1/agents/agt_sibling/channel-usage': () => ({
          agent_id: siblingAgent.id,
          channel_usage: {
            active: false,
            has_history: false,
            first_used_at: null,
            last_used_at: null,
          },
        }),
        '/api/v1/agents/agt_sibling/provenance': () => ({
          agent_id: siblingAgent.id,
          created_at: boundary.agent.created_at,
          creation_origin: 'legacy_unknown',
        }),
        '/api/v1/agents/agt_sibling/transcript': () => ({
          agent_id: siblingAgent.id,
          through_seq: 0,
          turns: [],
        }),
        '/api/v1/agents/agt_sibling/usage': () => ({
          agent_id: siblingAgent.id,
          through_seq: 0,
          input_tokens: 0,
          output_tokens: 0,
          context_used: 0,
        }),
      };
      for (const [path, body] of Object.entries(siblingReads))
        await page.route(`**${path}`, (route) => {
          expect(route.request().method()).toBe('GET');
          return route.fulfill({ json: body() });
        });
      const id =
        kind === 'Sandbox'
          ? boundary.sandbox.id
          : kind === 'Workspace'
            ? boundary.workspace.id
            : boundary.agent.id;
      const siblingId =
        kind === 'Sandbox'
          ? siblingSandbox.id
          : kind === 'Workspace'
            ? siblingWorkspace.id
            : siblingAgent.id;
      const collection =
        kind === 'Sandbox' ? 'sandboxes' : kind === 'Workspace' ? 'workspaces' : 'agents';
      const path = `/api/v1/${collection}/${id}`;
      const deletes: string[] = [];
      await page.route(`**${path}`, async (route) => {
        if (route.request().method() !== 'DELETE') return route.fallback();
        const key = route.request().headers()['idempotency-key'];
        expect(key).toBeTruthy();
        deletes.push(key!);
        if (kind === 'Sandbox') boundary.sandbox = { ...boundary.sandbox, status: 'retiring' };
        if (kind === 'Workspace')
          boundary.workspace = { ...boundary.workspace, status: 'removing' };
        return kind === 'Agent'
          ? route.fulfill({ status: 202, body: '' })
          : route.fulfill({
              status: 202,
              json: kind === 'Sandbox' ? boundary.sandbox : boundary.workspace,
            });
      });
      await page.goto('/agent-manager');
      await environments(page);
      if (kind !== 'Sandbox') await expand(resourceRow(page, 'sandbox', boundary.sandbox.id));
      if (kind === 'Agent') await expand(resourceRow(page, 'workspace', boundary.workspace.id));
      const target = resourceRow(page, kind.toLowerCase(), id);
      const sibling = resourceRow(page, kind.toLowerCase(), siblingId);
      await expect(sibling).toBeVisible();
      await target.hover();
      await target.getByRole('button', { name: /^Options for / }).click();
      await page
        .getByRole('menuitem', { name: kind === 'Workspace' ? 'Remove' : 'Delete', exact: true })
        .click();
      const confirm = page.getByRole('dialog', { name: `Delete ${kind}`, exact: true });
      await expect(confirm).toBeVisible();
      expect(deletes).toEqual([]);
      await confirm.getByRole('checkbox').check();
      await confirm
        .getByRole('button', {
          name: kind === 'Sandbox' ? 'Request deletion' : `Delete ${kind}`,
          exact: true,
        })
        .click();
      await expect(
        page.getByText(`Deleting 1 ${kind === 'Sandbox' ? 'environment' : kind}`, {
          exact: true,
        }),
      ).toBeVisible();
      await expect(confirm).toHaveCount(0);
      await expect(page.getByRole('dialog', { name: `${kind} deleted`, exact: true })).toHaveCount(
        0,
      );
      expect(deletes).toHaveLength(1);
      const stored = () =>
        page.evaluate(
          () =>
            JSON.parse(localStorage.getItem('cma:deletion:local-owner') ?? 'null') as {
              key: string;
              accepted: boolean;
            } | null,
        );
      await expect.poll(stored).toMatchObject({ key: deletes[0], accepted: true });
      await navigation(page, false);
      await page
        .getByRole('region', { name: 'Deletion request', exact: true })
        .getByRole('button', { name: 'View status', exact: true })
        .click();
      await page
        .getByRole('button', { name: `View ${kind} deletion status (1)`, exact: true })
        .click();
      const progress = page.getByRole('dialog', { name: `Deleting ${kind}`, exact: true });
      await expect(progress).toBeVisible();
      await expect(progress).toContainText('Delete request accepted');
      await expect(page.locator('.modal').filter({ has: progress })).toHaveCSS('opacity', '1');
      for (const dark of [false, true]) {
        await page.evaluate(
          (value) => document.body.toggleAttribute('data-ds-dark-theme', value),
          dark,
        );
        await insideViewport(progress);
        await page.screenshot({
          path: info.outputPath(`${kind}-${width}-${dark ? 'dark' : 'light'}-deleting.png`),
          fullPage: true,
        });
      }
      await page.reload();
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await expect.poll(stored).toMatchObject({ key: deletes[0], accepted: true });
      await navigation(page, false);
      await page.getByRole('button', { name: 'View deletion status', exact: true }).click();
      await page
        .getByRole('button', { name: `View ${kind} deletion status (1)`, exact: true })
        .click();
      await expect(progress).toBeVisible();
      expect(deletes).toHaveLength(1);
      if (kind === 'Sandbox') boundary.sandbox = { ...boundary.sandbox, status: 'retired' };
      if (kind === 'Workspace') {
        boundary.workspace = { ...boundary.workspace, status: 'removed' };
        boundary.sandbox = {
          ...boundary.sandbox,
          workspaces: [boundary.workspace, siblingWorkspace],
        };
      }
      if (kind === 'Agent') {
        boundary.agent = {
          ...boundary.agent,
          status: 'deleted',
          worktree: { ...boundary.agent.worktree, status: 'removed' },
        };
        boundary.workspace = { ...boundary.workspace, agents: [siblingAgent] };
      }
      const complete = page.getByRole('dialog', { name: `${kind} deleted`, exact: true });
      await expect(complete).toBeVisible({ timeout: 10000 });
      await complete.getByRole('button', { name: 'Done', exact: true }).click();
      await page.reload();
      await environments(page);
      if (kind !== 'Sandbox') await expand(resourceRow(page, 'sandbox', boundary.sandbox.id));
      if (kind === 'Agent') await expand(resourceRow(page, 'workspace', boundary.workspace.id));
      await expect(target).toHaveCount(0);
      await expect(sibling).toBeVisible();
      expect(deletes).toHaveLength(1);
      noEffects(boundary);
    });
  }
}

registerDeletionConfirmationCases();
