import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { AgentInventory } from './AgentInventory';
import { chooseOption } from '../test/select';

const access = {
  owner: { kind: 'person', is_current_person: true },
  capabilities: ['view_details', 'manage_sharing'],
  revalidate_at: null,
  policy_revision: 0,
};

const row = {
  agent_id: 'agt_1',
  title: 'Retained investigation',
  status: 'deleted',
  created_at: '2026-09-01T00:00:00Z',
  updated_at: '2026-09-02T00:00:00Z',
  creation_origin: 'manual',
  channel_usage: { active: true, has_history: true, first_used_at: null, last_used_at: null },
};

it('continues through an empty filtered page, preserves the filter, and opens a retained Agent', async () => {
  const queries: URLSearchParams[] = [];
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    if (url.pathname.startsWith('/api/v1/resource-access/agent/')) return Response.json(access);
    if (url.pathname.endsWith('/organizations')) return Response.json({ items: [] });
    queries.push(url.searchParams);
    if (url.searchParams.get('creation_origin') === 'manual' && !url.searchParams.has('cursor'))
      return Response.json({ items: [], next: 'agt_0' });
    return Response.json({ items: [row] });
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const navigate = vi.fn();
  render(<AgentInventory client={client} onNavigate={navigate} />);
  const user = userEvent.setup();
  await screen.findByRole('link', { name: row.title });
  await chooseOption(user, 'Creation origin', 'Created manually');
  await chooseOption(user, 'Channel activity', 'Currently linked to a channel');
  expect(
    await screen.findByText('No matches on this page. Continue to check the next page.'),
  ).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Next page' }));
  await screen.findByRole('link', { name: row.title });
  expect(queries.at(-1)?.get('creation_origin')).toBe('manual');
  expect(queries.at(-1)?.get('channel_active')).toBe('true');
  expect(queries.at(-1)?.get('cursor')).toBe('agt_0');
  expect(screen.getByRole('button', { name: 'Next page' })).toBeDisabled();
  expect(screen.getByText('Deleted')).toBeInTheDocument();
  expect(screen.getByText('Linked to a channel')).toBeInTheDocument();
  await user.click(screen.getByRole('link', { name: row.title }));
  expect(navigate).toHaveBeenCalledWith({ kind: 'agent', agentId: 'agt_1' });
});

it('clears old scope rows immediately and ignores an aborted late page', async () => {
  let finish: ((response: Response) => void) | undefined;
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    if (url.pathname.startsWith('/api/v1/resource-access/agent/')) return Response.json(access);
    if (url.pathname.endsWith('/organizations'))
      return Response.json({ items: [{ id: 'org', role: 'viewer', display_name: 'Research' }] });
    if (url.searchParams.has('organization'))
      return new Promise<Response>((resolve) => {
        finish = resolve;
      });
    return Response.json({ items: [row] });
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const navigate = vi.fn();
  const view = render(<AgentInventory client={client} onNavigate={navigate} />);
  await screen.findByRole('link', { name: row.title });
  view.rerender(<AgentInventory client={client} organization="org" onNavigate={navigate} />);
  expect(screen.queryByText(row.title)).not.toBeInTheDocument();
  await waitFor(() => expect(finish).toBeDefined());
  view.rerender(<AgentInventory client={client} onNavigate={navigate} />);
  await screen.findByRole('link', { name: row.title });
  await act(async () =>
    finish?.(Response.json({ items: [{ ...row, title: 'Old organization' }] })),
  );
  expect(screen.queryByText('Old organization')).not.toBeInTheDocument();
});

it('keeps personal inventory usable during an organization outage and clears refused pages', async () => {
  let revoked = false;
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const url = new URL(String(input));
      if (url.pathname.startsWith('/api/v1/resource-access/agent/')) return Response.json(access);
      if (url.pathname.endsWith('/organizations'))
        return Response.json(
          { code: 'organization_authority_unavailable', detail: 'Try again later' },
          { status: 503 },
        );
      return revoked
        ? Response.json({ code: 'resource_not_found', detail: 'Access removed' }, { status: 404 })
        : Response.json({ items: [row] });
    },
  });
  render(<AgentInventory client={client} onNavigate={vi.fn()} />);
  await screen.findByRole('link', { name: row.title });
  expect(await screen.findByText(/Organization choices are unavailable/)).toBeInTheDocument();
  revoked = true;
  await userEvent.setup().click(screen.getByRole('button', { name: 'Refresh' }));
  expect(screen.queryByText(row.title)).not.toBeInTheDocument();
  expect(await screen.findByRole('alert')).toHaveTextContent('Access removed');
});

it('clears the displayed page when export is refused, before the follow-up read completes', async () => {
  let reads = 0;
  let finish: ((response: Response) => void) | undefined;
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      if (new URL(String(input)).pathname.startsWith('/api/v1/resource-access/agent/'))
        return Response.json(access);
      if (new URL(String(input)).pathname.endsWith('/organizations'))
        return Response.json({ items: [] });
      reads += 1;
      if (reads === 1) return Response.json({ items: [row] });
      if (reads === 2)
        return Response.json(
          { code: 'resource_not_found', detail: 'Access removed' },
          { status: 404 },
        );
      return new Promise<Response>((resolve) => {
        finish = resolve;
      });
    },
  });
  render(<AgentInventory client={client} onNavigate={vi.fn()} />);
  await screen.findByRole('link', { name: row.title });
  await userEvent.setup().click(screen.getByRole('button', { name: 'Export this page' }));
  await waitFor(() => expect(finish).toBeDefined());
  expect(screen.queryByText(row.title)).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Export this page' })).not.toBeInTheDocument();
  await act(async () =>
    finish?.(
      Response.json({ code: 'resource_not_found', detail: 'Access removed' }, { status: 404 }),
    ),
  );
  expect(await screen.findByRole('alert')).toHaveTextContent('Access removed');
});

it('uses the shared aggregate for filters and fresh exports and keeps organization IDs distinct', async () => {
  const calls: URL[] = [];
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const url = new URL(String(input));
      if (url.pathname.startsWith('/api/v1/resource-access/agent/')) return Response.json(access);
      if (url.pathname.endsWith('/organizations'))
        return Response.json({
          items: [{ id: 'shared', display_name: 'Named organization', role: 'viewer' }],
        });
      calls.push(url);
      if (calls.length === 3)
        return Response.json(
          { code: 'resource_not_found', detail: 'Shared access removed' },
          { status: 404 },
        );
      if (calls.length > 3) return Response.json({ items: [] });
      return Response.json({ items: [row] });
    },
  });
  const navigate = vi.fn();
  const view = render(<AgentInventory client={client} shared onNavigate={navigate} />);
  await screen.findByRole('link', { name: row.title });
  const user = userEvent.setup();
  await chooseOption(user, 'Creation origin', 'Created manually');
  await screen.findByRole('link', { name: row.title });
  await user.click(screen.getByRole('button', { name: 'Export this page' }));
  await waitFor(() => expect(screen.queryByText(row.title)).not.toBeInTheDocument());
  expect(calls[2]?.searchParams.get('shared')).toBe('true');
  expect(calls[2]?.searchParams.get('creation_origin')).toBe('manual');
  expect(calls.every((url) => !url.searchParams.has('organization'))).toBe(true);
  await chooseOption(user, 'Resource scope', 'Named organization (Organization)');
  expect(navigate).toHaveBeenLastCalledWith({ kind: 'inventory', organization: 'shared' });
  view.rerender(<AgentInventory client={client} organization="shared" onNavigate={navigate} />);
  await chooseOption(user, 'Resource scope', 'Shared');
  expect(navigate).toHaveBeenLastCalledWith({ kind: 'inventory', shared: true });
});
