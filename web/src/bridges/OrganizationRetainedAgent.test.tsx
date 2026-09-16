import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { BridgePage } from './BridgePage';
import { BridgeManager } from './BridgeManager';
import { placementTransport } from './placement.test-support';
import { bridgeDetail } from './fixtures.test-support';
import { chooseOption } from '../test/select';
import {
  organizationAccess,
  organizationResponse,
  organizationTarget,
} from './organization.fixture';

function transport(override?: Parameters<typeof placementTransport>[1]) {
  return placementTransport('org-one', (url, init) => {
    const custom = override?.(url, init);
    if (custom !== undefined) return custom;
    const response = organizationResponse(url);
    return response === undefined ? undefined : Response.json(response);
  });
}
async function fill(override?: Parameters<typeof placementTransport>[1]) {
  const test = transport(override);
  render(
    <BridgePage
      client={test.client}
      viewerId="owner"
      route={{ kind: 'bridge_new', organization: 'org-one' }}
      onNavigate={vi.fn()}
    />,
  );
  const user = userEvent.setup();
  await waitFor(() => expect(screen.getByLabelText('Bridge name')).toBeEnabled());
  await user.type(screen.getByLabelText('Bridge name'), 'Retained research');
  await chooseOption(user, 'Existing channel bot', 'telegram · bot-one');
  await chooseOption(user, 'Existing channel key', 'key-one');
  await user.click(screen.getByRole('checkbox', { name: 'route-one' }));
  await chooseOption(user, 'Resource placement', 'One retained Agent');
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Retained Sandbox' })).toBeEnabled(),
  );
  await chooseOption(user, 'Retained Sandbox', 'Organization Sandbox');
  return { ...test, user };
}
async function chooseAgent(test: Awaited<ReturnType<typeof fill>>) {
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Retained Workspace' })).toBeEnabled(),
  );
  await chooseOption(test.user, 'Retained Workspace', 'Organization project');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Retained Agent' })).toBeEnabled());
  await chooseOption(test.user, 'Retained Agent', 'Organization Agent');
  await screen.findByText('Linked to a channel');
  await test.user.click(screen.getByRole('checkbox', { name: /I reviewed who owns/ }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Save disabled bridge' })).toBeEnabled(),
  );
}

it('creates an organization retained target from shared hierarchy without inventing redacted details', async () => {
  const test = await fill();
  await chooseAgent(test);
  expect(screen.getAllByText('Unavailable in the current resource view')).toHaveLength(2);
  await screen.findByText(/Its output remains in CMA/);
  expect(screen.getByText('Linked to a channel')).toBeInTheDocument();
  await test.user.click(screen.getByRole('button', { name: 'Save disabled bridge' }));
  await waitFor(() => expect(test.writes).toHaveLength(1));
  expect(JSON.parse(String(test.writes[0]!.body))).toMatchObject({
    owner: { kind: 'organization', subject: 'org-one' },
    config: {
      partition: 'fixed_shared_agent',
      placement: { kind: 'existing_agent', target: organizationTarget },
    },
  });
  const list = test.fetch.mock.calls
    .map(([url]) => new URL(String(url)))
    .find((url) => url.pathname.endsWith('/sandboxes'))!;
  expect(list.searchParams.get('organization')).toBe('org-one');
  expect(
    test.fetch.mock.calls.some(([url]) => new URL(String(url)).pathname === '/api/v1/agents'),
  ).toBe(false);
});

it('checks parent authority before fetching organization child details', async () => {
  const test = await fill((url) =>
    url.pathname.includes('/resource-access/')
      ? Response.json({ ...organizationAccess, capabilities: ['view_details'] })
      : undefined,
  );
  await screen.findByText(/Current management access or parent resources could not be verified/);
  expect(screen.getByRole('button', { name: 'Retained Workspace' })).toBeDisabled();
  expect(
    test.fetch.mock.calls.some(
      ([url]) => new URL(String(url)).pathname === '/api/v1/sandboxes/sandbox-1',
    ),
  ).toBe(false);
  expect(test.writes).toHaveLength(0);
});

it('refuses a new save if current target authority changes after selection', async () => {
  let revoked = false;
  const test = await fill((url) =>
    revoked && url.pathname.includes('/resource-access/')
      ? Response.json({
          ...organizationAccess,
          owner: { kind: 'organization', organization_id: 'other' },
        })
      : undefined,
  );
  await chooseAgent(test);
  revoked = true;
  await test.user.click(screen.getByRole('button', { name: 'Save disabled bridge' }));
  await screen.findByRole('alert');
  expect(test.writes).toHaveLength(0);
  expect(screen.queryByRole('button', { name: 'Retry create bridge' })).not.toBeInTheDocument();
});

it('recovers an uncertain organization save with the exact original body and key after access changes', async () => {
  const attempts: RequestInit[] = [];
  let revoked = false;
  const test = await fill((url, init) => {
    if (url.pathname.endsWith('/bridges') && init.method === 'POST') {
      attempts.push(init);
      if (attempts.length === 1) return Promise.reject(new Error('Response lost'));
    }
    if (revoked && url.pathname.includes('/resource-access/'))
      return new Response(null, { status: 403 });
    return undefined;
  });
  await chooseAgent(test);
  await test.user.click(screen.getByRole('button', { name: 'Save disabled bridge' }));
  const retry = await screen.findByRole('button', { name: 'Retry create bridge' });
  revoked = true;
  const reads = test.fetch.mock.calls.filter(([url]) =>
    String(url).includes('/resource-access/agent/'),
  ).length;
  await test.user.click(retry);
  await waitFor(() => expect(attempts).toHaveLength(2));
  expect(
    test.fetch.mock.calls.filter(([url]) => String(url).includes('/resource-access/agent/')),
  ).toHaveLength(reads);
  expect(attempts[1]!.body).toBe(attempts[0]!.body);
  expect(new Headers(attempts[1]!.headers).get('Idempotency-Key')).toBe(
    new Headers(attempts[0]!.headers).get('Idempotency-Key'),
  );
});

it('keeps saved organization target IDs and unrelated editing when target access is unavailable', async () => {
  const saved = {
    ...bridgeDetail,
    bridge: { ...bridgeDetail.bridge, owner: { kind: 'organization', subject: 'org-one' } },
    config: {
      ...bridgeDetail.config,
      placement: { kind: 'existing_agent', target: organizationTarget },
    },
  };
  const test = transport((url) => {
    if (url.pathname.endsWith('/configuration')) return Response.json(saved);
    if (url.pathname.includes('/resource-access/')) return new Response(null, { status: 403 });
    return undefined;
  });
  render(<BridgeManager client={test.client} id="brg_one" allowed onChanged={vi.fn()} />);
  await screen.findByText(/Saved target IDs remain visible/);
  expect(screen.getByText('worktree-1')).toBeInTheDocument();
  const user = userEvent.setup();
  await waitFor(() => expect(screen.getByLabelText('Bridge name')).toBeEnabled());
  await user.type(screen.getByLabelText('Bridge name'), ' renamed');
  await user.click(screen.getByRole('checkbox', { name: /I reviewed resource ownership/ }));
  expect(screen.getByRole('button', { name: 'Save settings' })).toBeEnabled();
});
