import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { createClient, type AgentShareLink, type CreateAgentShareLinkRequest } from '@cma/client';
import { AgentShareLinksDialog } from './AgentShareLinks';
import { chooseOption } from '../test/select';

const owner = { subject: 'owner', display_name: 'Alex', avatar_url: null };
const link = (id: string, body?: CreateAgentShareLinkRequest): AgentShareLink => ({
  id,
  agent_id: 'agent-one',
  audience: body?.audience ?? { kind: 'public' },
  mode: body?.mode ?? 'view',
  shared_by: owner,
  created_at: '2026-10-22T01:00:00Z',
  revoked_at: null,
  revision: 1,
  accepted_count: 0,
});
beforeEach(() => {
  sessionStorage.clear();
  localStorage.clear();
});

function fixture(initial: AgentShareLink[] = []) {
  const state = { links: initial, failCreate: 0, failRevoke: 0 };
  const requests: {
    method: string;
    path: string;
    body?: CreateAgentShareLinkRequest;
    key: string | null;
  }[] = [];
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    const method = init?.method ?? 'GET';
    const body = init?.body
      ? (JSON.parse(String(init.body)) as CreateAgentShareLinkRequest)
      : undefined;
    requests.push({
      method,
      path: url.pathname,
      body,
      key: new Headers(init?.headers).get('Idempotency-Key'),
    });
    if (url.pathname === '/api/v1/organizations')
      return Response.json({
        items: [{ id: 'org-one', role: 'member', display_name: 'ChronoAI' }],
      });
    if (url.pathname.endsWith('/share-recipients'))
      return Response.json({
        items: [{ subject: 'canonical-calvin-id', display_name: 'Calvin', avatar_url: null }],
      });
    if (method === 'POST') {
      if (state.failCreate-- > 0)
        return Response.json({ detail: 'Temporary create failure' }, { status: 503 });
      const created = link(`share-${String(state.links.length + 1)}`, body);
      state.links.push(created);
      return Response.json(created);
    }
    if (method === 'DELETE') {
      if (state.failRevoke-- > 0)
        return Response.json({ detail: 'Temporary revoke failure' }, { status: 503 });
      const target = state.links.find((item) => url.pathname.endsWith(`/${item.id}`))!;
      state.links = state.links.filter((item) => item.id !== target.id);
      return Response.json({ ...target, revision: 2, revoked_at: '2026-10-22T02:00:00Z' });
    }
    return Response.json({ items: state.links });
  });
  const client = createClient({ baseUrl: 'https://cma.example', fetch });
  const draw = () =>
    render(
      <AgentShareLinksDialog
        client={client}
        ownerId={owner.subject}
        agentId="agent-one"
        open
        onClose={vi.fn()}
      />,
    );
  return { state, requests, fetch, client, draw };
}

it('creates independent links with different permissions and only exposes newly generated fragment secrets', async () => {
  const test = fixture([link('existing')]);
  const user = userEvent.setup();
  test.draw();
  await screen.findByText(
    'The link secret is not stored. Create a new link if you no longer have it.',
  );
  expect(screen.queryByRole('button', { name: 'Copy link' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Create link' }));
  const firstUrl = await screen.findByRole('textbox', { name: 'Share link 2 URL' });
  const first = test.requests.find((request) => request.method === 'POST')!;
  expect(first.body?.token).toMatch(/^[a-f0-9]{64}$/);
  expect(first.body).toMatchObject({ audience: { kind: 'public' }, mode: 'view' });
  expect(first.key).toBeTruthy();
  expect(firstUrl).toHaveValue(
    `${window.location.origin}/agent-manager/share/share-2#${first.body!.token}`,
  );
  expect(test.requests.every((request) => !request.path.includes(first.body!.token))).toBe(true);
  expect(sessionStorage.length).toBe(0);
  expect(localStorage.length).toBe(0);
  await chooseOption(user, 'Share permission', 'View and write');
  await chooseOption(user, 'Share audience', 'Organization members');
  await user.click(screen.getByRole('button', { name: 'Create link' }));
  await screen.findByRole('textbox', { name: 'Share link 3 URL' });
  const second = test.requests.filter((request) => request.method === 'POST')[1]!;
  expect(second.body).toMatchObject({
    audience: { kind: 'organization', organization_id: 'org-one' },
    mode: 'view_write',
  });
  expect(second.body!.token).not.toBe(first.body!.token);
  expect(second.key).not.toBe(first.key);
});

it('retries an uncertain create with the original token, body and idempotency key', async () => {
  const test = fixture();
  test.state.failCreate = 1;
  const user = userEvent.setup();
  test.draw();
  await screen.findByText('No active links.');
  await user.click(screen.getByRole('button', { name: 'Create link' }));
  await screen.findByRole('button', { name: 'Retry creating link' });
  expect(screen.getByRole('button', { name: 'Share audience' })).toBeDisabled();
  await user.click(screen.getByRole('button', { name: 'Retry creating link' }));
  await screen.findByRole('textbox', { name: 'Share link 1 URL' });
  const posts = test.requests.filter((request) => request.method === 'POST');
  expect(posts).toHaveLength(2);
  expect(posts[1]).toEqual(posts[0]);
  expect(screen.getByRole('button', { name: 'Share audience' })).toBeEnabled();
});

it('revokes one link only after confirmation and retains other links through a failed retry', async () => {
  const test = fixture([link('first'), link('second')]);
  test.state.failRevoke = 1;
  const user = userEvent.setup();
  test.draw();
  const first = await screen.findByRole('article', { name: 'Share link 1' });
  await user.click(within(first).getByRole('button', { name: 'Revoke link' }));
  expect(test.requests.filter((request) => request.method === 'DELETE')).toHaveLength(0);
  await user.click(within(first).getByRole('button', { name: 'Revoke access' }));
  await screen.findByRole('alert');
  expect(screen.getAllByRole('article')).toHaveLength(2);
  await user.click(within(first).getByRole('button', { name: 'Revoke access' }));
  await waitFor(() => expect(screen.getAllByRole('article')).toHaveLength(1));
  expect(test.state.links.map((item) => item.id)).toEqual(['second']);
  const deletes = test.requests.filter((request) => request.method === 'DELETE');
  expect(deletes[1]).toEqual(deletes[0]);
});

it('allows specific recipients only after canonical user lookup and selection', async () => {
  const test = fixture();
  const user = userEvent.setup();
  test.draw();
  await screen.findByText('No active links.');
  await chooseOption(user, 'Share audience', 'Specific people');
  expect(screen.getByRole('button', { name: 'Create link' })).toBeDisabled();
  await user.type(screen.getByRole('textbox', { name: 'Find share recipients' }), 'calvin');
  await user.click(await screen.findByRole('button', { name: 'Add' }));
  await user.click(screen.getByRole('button', { name: 'Create link' }));
  await screen.findByRole('textbox', { name: 'Share link 1 URL' });
  expect(test.requests.find((request) => request.method === 'POST')?.body?.audience).toEqual({
    kind: 'users',
    subjects: ['canonical-calvin-id'],
  });
});
