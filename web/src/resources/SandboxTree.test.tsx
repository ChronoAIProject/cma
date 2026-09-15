import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createClient } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import { chooseOption, chosenOption } from '../test/select';
import { SandboxTree } from './SandboxTree';

const at = '2026-09-01T00:00:00Z';
const sandbox = {
  access: 'shared',
  id: 'sbx_research',
  name: 'Research Sandbox',
  status: 'active',
  created_at: at,
  updated_at: at,
  last_activity_at: at,
  workspace_count: 2,
  worktree_count: 3,
  agent_count: 3,
};
const agent = {
  agent_id: 'agt_retained',
  title: 'Retained investigation',
  status: 'stopped',
  created_at: at,
  updated_at: at,
  creation_origin: 'manual',
  channel_usage: {
    active: false,
    has_history: true,
    first_used_at: at,
    last_used_at: at,
  },
};

function fixture() {
  const calls: { url: URL; signal: AbortSignal | null | undefined }[] = [];
  const state = { revoked: false, sandboxPage: false, agentPage: false };
  const fetch = vi.fn(
    async (input: RequestInfo | URL, options?: RequestInit): Promise<Response> => {
      const url = new URL(String(input));
      calls.push({ url, signal: options?.signal });
      if (state.revoked)
        return Response.json(
          { code: 'resource_not_found', detail: 'Access removed' },
          { status: 404 },
        );
      if (
        url.pathname === '/api/v1/resource-access/sandbox/sbx_research' ||
        url.pathname === '/api/v1/resource-access/agent/agt_retained'
      )
        return Response.json({
          owner: { kind: 'person', is_current_person: false },
          capabilities: ['view_details'],
          revalidate_at: null,
          policy_revision: 1,
        });
      if (url.pathname === '/api/v1/sandboxes')
        return Response.json(
          state.sandboxPage && !url.searchParams.has('cursor')
            ? { items: [], next: 'sbx_previous' }
            : { items: [sandbox] },
        );
      expect(url.pathname).toBe('/api/v1/agents/provenance');
      return Response.json(
        state.agentPage && !url.searchParams.has('cursor')
          ? { items: [], next: 'agt_previous' }
          : { items: [agent] },
      );
    },
  );
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const navigate = vi.fn();
  return { calls, client, fetch, state, navigate };
}

it.each([false, true])(
  'filters authorized Sandbox Agents, continues empty pages, and keeps exact scope (shared=%s)',
  async (shared) => {
    const test = fixture();
    test.state.agentPage = true;
    render(
      <SandboxTree
        client={test.client}
        organization={shared ? undefined : 'org_research'}
        shared={shared ? true : undefined}
        onNavigate={test.navigate}
      />,
    );
    const user = userEvent.setup();
    await screen.findByRole('button', { name: sandbox.name });
    await chooseOption(user, 'Sandbox Agent creation', 'Created manually');
    await chooseOption(user, 'Sandbox Agent channel activity', 'Previously linked, now detached');
    expect(
      test.calls.every(
        ({ url }) =>
          url.pathname === '/api/v1/sandboxes' ||
          url.pathname === '/api/v1/resource-access/sandbox/sbx_research',
      ),
    ).toBe(true);
    await user.click(await screen.findByRole('button', { name: sandbox.name }));
    await screen.findByText('No matching Agents on this page. Continue to check the next page.');
    await user.click(screen.getByRole('button', { name: 'Next matching Agents' }));
    const link = await screen.findByRole('link', { name: agent.title });
    expect(link).toHaveAttribute('href', '/agents/agt_retained');
    const params = test.calls
      .filter(({ url }) => !url.pathname.includes('/resource-access/'))
      .at(-1)!.url.searchParams;
    expect(Object.fromEntries(params)).toEqual({
      ...(shared ? { shared: 'true' } : { organization: 'org_research' }),
      sandbox: sandbox.id,
      creation_origin: 'manual',
      channel_active: 'false',
      channel_history: 'true',
      cursor: 'agt_previous',
      limit: '50',
    });
    expect(
      screen.getByText('Stopped · Created manually · Previously linked to a channel'),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Next matching Agents' })).toBeDisabled();
    await user.click(link);
    expect(test.navigate).toHaveBeenCalledWith({ kind: 'agent', agentId: agent.agent_id });
    await user.click(screen.getByRole('button', { name: 'Previous matching Agents' }));
    await screen.findByText('No matching Agents on this page. Continue to check the next page.');
    expect(
      test.calls
        .filter(({ url }) => !url.pathname.includes('/resource-access/'))
        .at(-1)!
        .url.searchParams.has('cursor'),
    ).toBe(false);
    expect(
      test.calls.every(
        ({ url }) =>
          /\/(sandboxes|agents\/provenance)$/.test(url.pathname) ||
          [
            '/api/v1/resource-access/sandbox/sbx_research',
            '/api/v1/resource-access/agent/agt_retained',
          ].includes(url.pathname),
      ),
    ).toBe(true);
  },
);

it('resets Sandbox and Agent pages and clears expanded rows when a filter changes', async () => {
  const test = fixture();
  test.state.sandboxPage = true;
  test.state.agentPage = true;
  render(<SandboxTree client={test.client} organization="org" onNavigate={test.navigate} />);
  const user = userEvent.setup();
  await chooseOption(user, 'Sandbox Agent creation', 'Created manually');
  await screen.findByText(/No authorized Sandboxes on this page/);
  await user.click(screen.getByRole('button', { name: 'Next Sandboxes' }));
  await user.click(await screen.findByRole('button', { name: sandbox.name }));
  await screen.findByText('No matching Agents on this page. Continue to check the next page.');
  await user.click(screen.getByRole('button', { name: 'Next matching Agents' }));
  await screen.findByRole('link', { name: agent.title });
  await chooseOption(user, 'Sandbox Agent channel activity', 'Any channel history');
  expect(screen.queryByText(agent.title)).not.toBeInTheDocument();
  await screen.findByText(/No authorized Sandboxes on this page/);
  expect(screen.getByRole('button', { name: 'Previous Sandboxes' })).toBeDisabled();
  expect(
    test.calls
      .filter(({ url }) => !url.pathname.includes('/resource-access/'))
      .at(-1)!
      .url.searchParams.has('cursor'),
  ).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Next Sandboxes' }));
  const branch = await screen.findByRole('button', { name: sandbox.name });
  expect(branch).toHaveAttribute('aria-expanded', 'false');
  await user.click(branch);
  await screen.findByText('No matching Agents on this page. Continue to check the next page.');
  expect(screen.getByRole('button', { name: 'Previous matching Agents' })).toBeDisabled();
  expect(
    test.calls
      .filter(({ url }) => !url.pathname.includes('/resource-access/'))
      .at(-1)!
      .url.searchParams.get('channel_history'),
  ).toBe('true');
  expect(
    test.calls
      .filter(({ url }) => !url.pathname.includes('/resource-access/'))
      .at(-1)!
      .url.searchParams.has('cursor'),
  ).toBe(false);
});

it('discards a late filtered response after scope change and resets filters for the new scope', async () => {
  const test = fixture();
  const original = test.fetch.getMockImplementation()!;
  let finish: ((response: Response) => void) | undefined;
  let pendingSignal: AbortSignal | null | undefined;
  test.fetch.mockImplementation(async (input, options) => {
    const url = new URL(String(input));
    if (url.pathname.endsWith('/provenance') && url.searchParams.has('organization')) {
      pendingSignal = options?.signal;
      return new Promise((resolve) => {
        finish = resolve;
      });
    }
    return original(input, options);
  });
  const props = { client: test.client, onNavigate: test.navigate };
  const view = render(<SandboxTree {...props} organization="shared" />);
  const user = userEvent.setup();
  await chooseOption(user, 'Sandbox Agent creation', 'Created manually');
  await user.click(await screen.findByRole('button', { name: sandbox.name }));
  await waitFor(() => expect(finish).toBeDefined());
  view.rerender(<SandboxTree {...props} shared />);
  expect(pendingSignal?.aborted).toBe(true);
  expect(chosenOption('Sandbox Agent creation')).toContain('All creation origins');
  expect(chosenOption('Sandbox Agent channel activity')).toContain('All channel activity');
  await act(async () => finish?.(Response.json({ items: [agent] })));
  expect(screen.queryByText(agent.title)).not.toBeInTheDocument();
  await chooseOption(user, 'Sandbox Agent channel activity', 'Any channel history');
  await user.click(await screen.findByRole('button', { name: sandbox.name }));
  await screen.findByRole('link', { name: agent.title });
  const params = test.calls.filter(({ url }) => !url.pathname.includes('/resource-access/')).at(-1)!
    .url.searchParams;
  expect(params.get('shared')).toBe('true');
  expect(params.has('organization')).toBe(false);
  expect(params.has('creation_origin')).toBe(false);
  expect(params.has('cursor')).toBe(false);
});

it('clears filtered rows immediately on refresh and removes the parent after access refusal', async () => {
  const test = fixture();
  render(<SandboxTree client={test.client} shared onNavigate={test.navigate} />);
  const user = userEvent.setup();
  await chooseOption(user, 'Sandbox Agent creation', 'Created manually');
  await user.click(await screen.findByRole('button', { name: sandbox.name }));
  await screen.findByRole('link', { name: agent.title });
  test.state.revoked = true;
  await user.click(screen.getByRole('button', { name: 'Refresh filtered Agents' }));
  expect(screen.queryByText(agent.title)).not.toBeInTheDocument();
  await screen.findByRole('alert');
  expect(screen.getByRole('alert')).toHaveTextContent('Access removed');
  expect(screen.queryByText(sandbox.name)).not.toBeInTheDocument();
  expect(chosenOption('Sandbox Agent creation')).toContain('Created manually');
});

it('does not show rows from a repeated continuation and keeps older summary responses usable', async () => {
  const test = fixture();
  const original = test.fetch.getMockImplementation()!;
  test.fetch.mockImplementation(async (input, options) => {
    const url = new URL(String(input));
    if (!url.pathname.endsWith('/provenance')) return original(input, options);
    return Response.json(
      url.searchParams.has('cursor')
        ? { items: [agent], next: 'agt_previous' }
        : { items: [{ ...agent, channel_usage: undefined }], next: 'agt_previous' },
    );
  });
  render(<SandboxTree client={test.client} organization="org" onNavigate={test.navigate} />);
  const user = userEvent.setup();
  await chooseOption(user, 'Sandbox Agent creation', 'Created manually');
  await user.click(await screen.findByRole('button', { name: sandbox.name }));
  await screen.findByRole('link', { name: agent.title });
  expect(screen.getByText(/Channel activity unavailable/)).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Next matching Agents' }));
  await screen.findByText('Filtered Agent pagination did not advance. Refresh this Sandbox.');
  expect(screen.queryByText(agent.title)).not.toBeInTheDocument();
});
