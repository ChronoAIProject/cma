import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createClient } from '@chronoai/cma-sdk';
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
        return Response.json({ code: 'not_found' }, { status: 404 });
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
        return Response.json({ code: 'not_found' }, { status: 404 });
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
        'Completed',
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
        return Response.json({ code: 'not_found' }, { status: 404 });
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
