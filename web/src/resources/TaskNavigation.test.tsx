import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createClient } from '@cma/client';
import { expect, it, vi } from 'vitest';
import { SharedAgentNavigation } from './SharedAgentNavigation';

const task = {
  agent_id: 'agt_shared_task',
  title: 'Shared research',
  status: 'ready',
  task_status: 'completed',
  created_at: '2026-11-03T00:00:00Z',
  updated_at: '2026-11-03T00:00:00Z',
  creation_origin: 'manual',
};

it('removes retained task rows when a refresh confirms access was revoked', async () => {
  let revoked = false;
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      if (new URL(String(input)).pathname !== '/api/v1/agents/provenance')
        return Response.json({ code: 'temporarily_unavailable' }, { status: 503 });
      return revoked
        ? Response.json({ code: 'forbidden', detail: 'Access revoked' }, { status: 403 })
        : Response.json({ items: [task] });
    },
  });
  render(<SharedAgentNavigation client={client} onNavigate={vi.fn()} />);
  await screen.findByRole('treeitem', { name: /^Shared research/ });
  revoked = true;
  await userEvent.setup().click(screen.getByRole('button', { name: 'Refresh Agents' }));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.queryByRole('treeitem', { name: /^Shared research/ })).not.toBeInTheDocument();
});

it.each([true, false])(
  'reads visible task outcomes and filters through the authorized list (shared=%s)',
  async (shared) => {
    const requests: URL[] = [];
    const client = createClient({
      baseUrl: 'https://fixture.invalid',
      fetch: async (input) => {
        const url = new URL(String(input));
        requests.push(url);
        if (url.pathname === '/api/v1/agents/provenance') return Response.json({ items: [task] });
        return Response.json({ code: 'temporarily_unavailable' }, { status: 503 });
      },
    });
    const onNavigate = vi.fn();
    render(
      <SharedAgentNavigation
        client={client}
        shared={shared}
        organization={shared ? undefined : 'research'}
        statusFilter="completed"
        onNavigate={onNavigate}
      />,
    );
    expect(
      within(await screen.findByRole('treeitem', { name: /^Shared research/ })).getByText(
        'Turn ended',
      ),
    ).toBeVisible();
    expect(requests[0]?.searchParams.get('task_status')).toBe('completed');
    expect(requests.some((url) => /transcript|conversation|events/.test(url.pathname))).toBe(false);
    await userEvent.setup().click(screen.getByRole('link', { name: /^Shared research/ }));
    expect(onNavigate).toHaveBeenCalledWith({ kind: 'agent', agentId: 'agt_shared_task' });
  },
);

it('retains shared tasks on a transient refresh failure, disables stale navigation, and recovers on retry', async () => {
  let failing = false;
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      if (new URL(String(input)).pathname !== '/api/v1/agents/provenance')
        return Response.json({ code: 'temporarily_unavailable' }, { status: 503 });
      if (failing) throw new TypeError('Connection lost');
      return Response.json({ items: [task] });
    },
  });
  const onNavigate = vi.fn();
  const user = userEvent.setup();
  render(<SharedAgentNavigation client={client} onNavigate={onNavigate} />);
  await screen.findByRole('treeitem', { name: /^Shared research/ });
  failing = true;
  await user.click(screen.getByRole('button', { name: 'Refresh Agents' }));
  expect(await screen.findByRole('alert')).toBeVisible();
  const retained = screen.getByRole('treeitem', { name: /^Shared research/ });
  expect(retained).toBeVisible();
  expect(retained).toHaveAttribute('aria-disabled', 'true');
  expect(retained.querySelector('.statedot')).toBeNull();
  retained.focus();
  await user.keyboard('{Enter}');
  expect(onNavigate).not.toHaveBeenCalled();
  failing = false;
  await user.click(screen.getByRole('button', { name: 'Refresh Agents' }));
  await waitFor(() =>
    expect(screen.getByRole('treeitem', { name: /^Shared research/ })).not.toHaveAttribute(
      'aria-disabled',
      'true',
    ),
  );
  await user.click(screen.getByRole('link', { name: /^Shared research/ }));
  expect(onNavigate).toHaveBeenCalledWith({ kind: 'agent', agentId: 'agt_shared_task' });
});

it('uses the authorized detail title even when the inventory retains a creation placeholder', async () => {
  let title: string | undefined = 'Current shared title';
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const path = new URL(String(input)).pathname;
      if (path === '/api/v1/agents/provenance')
        return Response.json({ items: [{ ...task, title: 'New Agent' }] });
      if (path === '/api/v1/agents/agt_shared_task')
        return Response.json({
          access: 'shared',
          id: task.agent_id,
          status: task.status,
          title,
          created_at: task.created_at,
          updated_at: task.updated_at,
        });
      return Response.json({ code: 'not_found' }, { status: 404 });
    },
  });
  const user = userEvent.setup();
  render(<SharedAgentNavigation client={client} onNavigate={vi.fn()} />);
  expect(await screen.findByRole('treeitem', { name: /^Current shared title/ })).toBeVisible();
  title = 'Renamed shared title';
  await user.click(screen.getByRole('button', { name: 'Refresh Agents' }));
  expect(await screen.findByRole('treeitem', { name: /^Renamed shared title/ })).toBeVisible();
  title = undefined;
  await user.click(screen.getByRole('button', { name: 'Refresh Agents' }));
  expect(await screen.findByRole('treeitem', { name: /^Untitled task/ })).toBeVisible();
  expect(screen.queryByText('New Agent')).not.toBeInTheDocument();
});

it.each([403, 404])('withdraws a row refused after the inventory read (%s)', async (status) => {
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const path = new URL(String(input)).pathname;
      if (path === '/api/v1/agents/provenance')
        return Response.json({ items: [task, { ...task, agent_id: 'agt_shared_z' }] });
      if (path === '/api/v1/agents/agt_shared_task')
        return Response.json(
          { code: 'resource_not_found', detail: 'Agent unavailable' },
          { status },
        );
      if (path === '/api/v1/agents/agt_shared_z')
        return Response.json({
          access: 'shared',
          id: 'agt_shared_z',
          status: task.status,
          title: 'Still authorized',
          created_at: task.created_at,
          updated_at: task.updated_at,
        });
      return Response.json({ code: 'not_found' }, { status: 404 });
    },
  });
  render(<SharedAgentNavigation client={client} onNavigate={vi.fn()} />);
  expect(await screen.findByRole('treeitem', { name: /^Still authorized/ })).toBeVisible();
  expect(screen.queryByRole('treeitem', { name: /^Shared research/ })).not.toBeInTheDocument();
});

it('keeps a slow authorized inventory when optional title enrichment reaches the overall deadline', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
  try {
    const pointReads = vi.fn();
    const client = createClient({
      baseUrl: 'https://fixture.invalid',
      fetch: async (input) => {
        const path = new URL(String(input)).pathname;
        if (path === '/api/v1/agents/provenance') {
          await new Promise((resolve) => setTimeout(resolve, 10_500));
          return Response.json({ items: [task] });
        }
        if (path === '/api/v1/agents/agt_shared_task') {
          pointReads();
          return new Promise<Response>(() => {});
        }
        return Response.json({ code: 'not_found' }, { status: 404 });
      },
    });
    render(<SharedAgentNavigation client={client} onNavigate={vi.fn()} />);
    await act(async () => vi.advanceTimersByTimeAsync(10_500));
    expect(pointReads).toHaveBeenCalledOnce();
    await act(async () => vi.advanceTimersByTimeAsync(1500));
    expect(screen.getByRole('treeitem', { name: /^Shared research/ })).toBeVisible();
    await act(async () => vi.advanceTimersByTimeAsync(3_000));
    expect(screen.getByRole('treeitem', { name: /^Shared research/ })).toBeVisible();
  } finally {
    vi.useRealTimers();
  }
});
