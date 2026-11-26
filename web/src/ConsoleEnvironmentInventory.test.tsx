import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createClient } from '@cma/client';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import type { AppRoute } from './routing/routes';
import { agentClient, AGENT_DETAIL } from './test/agent';
import { sandboxFixture, workspaceFixture } from './test/hierarchy';
import { WIRE_AGENT, WIRE_SANDBOX_DETAIL } from './test/rest';
import './test/connectedAccounts';

const live = sandboxFixture();
const changing = {
  ...sandboxFixture('rotating'),
  id: 'sbx_updating',
  name: 'Updating Sandbox',
  agentCount: 0,
  workspaceCount: 0,
  worktreeCount: 0,
  workspaces: [],
};

it.each(['personal', 'shared'] as const)(
  'keeps both owned Sandboxes reachable from Environments opened from a %s task',
  async (scope) => {
    const taskId = scope === 'personal' ? AGENT_DETAIL.id : 'agt_shared';
    const taskRoute = { kind: 'agent', agentId: taskId } as const;
    const onNavigate = vi.fn();
    const client = agentClient({
      listSandboxes: async () => ({ items: [live, changing] }),
      readSandbox: async (id) => (id === live.id ? live : changing),
      readWorkspace: async () => workspaceFixture('failed'),
      readAgent: async () => ({ ...AGENT_DETAIL, status: 'failed' }),
      resourceViews: createClient({
        baseUrl: 'https://fixture.invalid',
        fetch: async (input) => {
          const path = new URL(String(input)).pathname;
          if (path.includes('/resource-access/'))
            return Response.json({
              owner: { kind: 'person', is_current_person: !path.endsWith('/agt_shared') },
              capabilities: ['view_details'],
              policy_revision: 1,
              revalidate_at: null,
            });
          if (path.endsWith('/agents/provenance'))
            return Response.json({
              items: [
                {
                  agent_id: 'agt_shared',
                  title: 'Shared Agent',
                  status: 'ready',
                  created_at: live.createdAt,
                  updated_at: live.updatedAt,
                  creation_origin: 'manual',
                },
              ],
            });
          if (path === '/api/v1/sandboxes' || path.endsWith('/organizations'))
            return Response.json({ items: [] });
          if (path.endsWith(`/sandboxes/${live.id}`))
            return Response.json({ ...WIRE_SANDBOX_DETAIL, id: live.id });
          if (path.endsWith(`/agents/${taskId}`))
            return Response.json({ ...WIRE_AGENT, id: taskId, status: 'failed' });
          return Response.json({ code: 'not_found' }, { status: 404 });
        },
      }),
    });
    function Navigation() {
      const [route, setRoute] = useState<AppRoute>(taskRoute);
      return (
        <CmaClientProvider client={client}>
          <Console
            user={{
              id: 'owner',
              handle: 'Owner',
              visitor: false,
              capabilities: ['use_own_sandboxes'],
            }}
            route={route}
            onSignOut={vi.fn()}
            onNavigate={(next) => {
              onNavigate(next);
              setRoute(next);
            }}
          />
        </CmaClientProvider>
      );
    }
    const user = userEvent.setup();
    render(<Navigation />);
    await screen.findByRole('tree', {
      name: scope === 'personal' ? 'Personal Agents' : 'Shared Agents',
    });
    await user.click(screen.getByRole('button', { name: 'Search' }));
    const search = await screen.findByRole('group', { name: 'Sandboxes' });
    expect(
      within(search)
        .getAllByRole('option')
        .map((row) => row.textContent),
    ).toEqual([expect.stringContaining(live.name), expect.stringContaining(changing.name)]);
    await user.keyboard('{Escape}');

    await user.click(screen.getByRole('button', { name: 'Environments' }));
    const liveGroup = await screen.findByRole('tree', { name: 'Live sandboxes' });
    const liveLink = within(liveGroup).getByRole('link', { name: /^Atlas/ });
    expect(liveLink).toHaveAttribute('href', `/sandboxes/${live.id}`);
    expect(
      within(screen.getByRole('tree', { name: 'Changing sandboxes' })).getByRole('link', {
        name: /^Updating Sandbox/,
      }),
    ).toHaveAttribute('href', `/sandboxes/${changing.id}`);
    await user.click(liveLink);
    expect(onNavigate).toHaveBeenLastCalledWith({ kind: 'sandbox', sandboxId: live.id });
    expect(liveLink).toHaveAttribute('aria-current', 'page');
    await user.click(screen.getByRole('button', { name: 'Tasks' }));
    expect(onNavigate).toHaveBeenLastCalledWith(taskRoute);
    await user.click(screen.getByRole('button', { name: 'Environments' }));
    expect(
      within(screen.getByRole('tree', { name: 'Live sandboxes' })).getByRole('link', {
        name: /^Atlas/,
      }),
    ).toBeVisible();
  },
);
