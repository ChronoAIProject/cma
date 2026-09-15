import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { createClient, type ResourceShare } from '@chronoai/cma-sdk';
import { SharingButton } from './SharingButton';
import { chooseOption } from '../test/select';
import { loadSharing } from './sharing';

const target = { kind: 'agent', id: 'agt_manual', name: 'Retained investigation' } as const;
const sandbox = { kind: 'sandbox', id: 'sbx_own', name: 'Research Sandbox' } as const;
const at = '2026-09-01T00:00:00Z';
const actor = { kind: 'human', subject: 'owner' } as const;
const grant: ResourceShare = {
  id: 'share_1',
  owner: { kind: 'person', subject: 'owner' },
  selector: { kind: 'agent', agent_id: target.id },
  target_org_id: 'org_research',
  mode: 'view',
  issuer_actor: actor,
  created_at: at,
  revoked_at: null,
  revision: 1,
};

function fixture() {
  const state = {
    allowed: true,
    organizations: true,
    policy: 1,
    uncertain: false,
    shares: [] as ResourceShare[],
  };
  const writes: {
    method: string;
    path: string;
    key: string | null;
    body: unknown;
    revision: string | null;
  }[] = [];
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = new URL(String(input));
    const path = url.pathname;
    if (path.endsWith('/organizations'))
      return state.organizations
        ? Response.json({
            items: [{ id: grant.target_org_id, display_name: 'Research', role: 'admin' }],
          })
        : Response.json(
            { code: 'organization_authority_unavailable', detail: 'Unavailable' },
            { status: 503 },
          );
    if (path.includes('/resource-access/'))
      return Response.json({
        capabilities: state.allowed ? ['view_details', 'manage_sharing'] : ['view_details'],
        revalidate_at: null,
        policy_revision: state.policy,
      });
    if (!path.includes('/resource-shares/')) throw new Error(`Unexpected resource request ${path}`);
    const kind = path.split('/')[4] as 'agent' | 'sandbox';
    if (!init?.method || init.method === 'GET')
      return Response.json({
        items: state.shares.filter(
          (share) => share.selector.kind === kind && share.revoked_at === null,
        ),
      });
    const body = init.body ? JSON.parse(String(init.body)) : undefined;
    writes.push({
      method: init.method,
      path,
      key: new Headers(init.headers).get('idempotency-key'),
      body,
      revision: url.searchParams.get('expected_revision'),
    });
    if (state.uncertain) {
      state.uncertain = false;
      return Response.json(
        { code: 'resource_access_unavailable', detail: 'Retry the same command key.' },
        { status: 503 },
      );
    }
    if (
      body?.mode === 'org_roles' &&
      kind === 'agent' &&
      !state.shares.some(
        (share) =>
          share.selector.kind === 'sandbox' &&
          share.mode === 'org_roles' &&
          share.revoked_at === null,
      )
    )
      return Response.json({ code: 'unsafe_interactive_share', detail: 'Unsafe' }, { status: 409 });
    let share: ResourceShare;
    if (init.method === 'POST') {
      share = {
        ...grant,
        id: `share_${state.policy}`,
        selector: kind === 'agent' ? grant.selector : { kind: 'sandbox', sandbox_id: sandbox.id },
        mode: body.mode,
      };
      state.shares.push(share);
    } else {
      const found = state.shares.find((share) => path.endsWith('/' + share.id))!;
      const revision = body?.expected_revision ?? Number(url.searchParams.get('expected_revision'));
      if (revision !== found.revision)
        return Response.json(
          { code: 'resource_access_changed', detail: 'Read current access and retry.' },
          { status: 409 },
        );
      share = {
        ...found,
        revision: found.revision + 1,
        mode: body?.mode ?? found.mode,
        revoked_at: init.method === 'DELETE' ? at : null,
      };
      state.shares = state.shares.map((old) => (old === found ? share : old));
    }
    state.policy += 1;
    return Response.json({
      operation_id: `operation_${state.policy}`,
      share,
      actor,
      at,
      policy_revision: state.policy,
    });
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const show = () => render(<SharingButton client={client} target={target} sandbox={sandbox} />);
  const open = async () => {
    const view = show();
    await userEvent.setup().click(await screen.findByRole('button', { name: 'Sharing' }));
    await screen.findByRole('heading', { name: 'Current grants' });
    return view;
  };
  return { state, fetch, client, writes, open, show };
}

it('does not expose sharing controls or request grants for a recipient', async () => {
  const test = fixture();
  test.state.allowed = false;
  test.show();
  await waitFor(() => expect(test.fetch).toHaveBeenCalledOnce());
  expect(screen.queryByRole('button', { name: 'Sharing' })).not.toBeInTheDocument();
  expect(test.fetch.mock.calls.every(([url]) => String(url).includes('/resource-access/'))).toBe(
    true,
  );
});

it('creates view access, refuses unsafe Agent interaction, and requires explicit whole-Sandbox selection', async () => {
  const test = fixture();
  await test.open();
  const user = userEvent.setup();
  await chooseOption(user, 'Share with organization', 'Research');
  expect(screen.getByLabelText('Sharing exposure preview')).toHaveTextContent(
    'no parent, sibling or file access',
  );
  await user.click(screen.getByRole('button', { name: 'Create share' }));
  await screen.findByText('Current access: View only');
  expect(test.writes[0]?.body).toEqual({ target_org_id: grant.target_org_id, mode: 'view' });
  await chooseOption(user, 'Access for Research', 'Use organization roles');
  await user.click(screen.getByRole('button', { name: 'Save access' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'does not have proven execution isolation',
  );
  expect(test.writes.every(({ path }) => path.includes('/agent/'))).toBe(true);
  expect(test.fetch.mock.calls.some(([url]) => String(url).includes('/sandbox/'))).toBe(false);
  await chooseOption(user, 'Share scope', 'Whole Sandbox');
  await screen.findByText('No direct organization grants.');
  await chooseOption(user, 'Share with organization', 'Research');
  await chooseOption(user, 'New share access', 'Use organization roles');
  expect(screen.getByRole('button', { name: 'Create share' })).toBeDisabled();
  expect(screen.getByLabelText('Sharing exposure preview')).toHaveTextContent('future resources');
  await user.click(screen.getByRole('checkbox'));
  await user.click(screen.getByRole('button', { name: 'Create share' }));
  await screen.findByText('Current access: Use organization roles');
  expect(test.writes.at(-1)?.path).toBe('/api/v1/resource-shares/sandbox/sbx_own');
  await chooseOption(user, 'Share scope', 'This Agent');
  await screen.findByText('Current access: View only');
  await chooseOption(user, 'Access for Research', 'Use organization roles');
  await user.click(screen.getByRole('button', { name: 'Save access' }));
  await screen.findByText('Current access: Use organization roles');
  expect(test.writes.at(-1)?.body).toEqual({ expected_revision: 1, mode: 'org_roles' });
  expect(
    test.state.shares.every((share) => share.owner.subject === 'owner' && share.created_at === at),
  ).toBe(true);
  expect(test.writes.every(({ key }) => Boolean(key))).toBe(true);
});

it('revokes direct access with its current revision during an organization outage', async () => {
  const test = fixture();
  test.state.shares = [grant];
  test.state.organizations = false;
  await test.open();
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Existing grants can still be revoked',
  );
  expect(screen.queryByText(grant.target_org_id)).not.toBeInTheDocument();
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Revoke' }));
  expect(screen.getByText(/Other grants may still provide access/)).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Revoke grant' }));
  await screen.findByText('No direct organization grants.');
  expect(test.writes).toEqual([expect.objectContaining({ method: 'DELETE', revision: '1' })]);
});

it('retains the idempotency key when retrying an unconfirmed create', async () => {
  const test = fixture();
  await test.open();
  test.state.uncertain = true;
  const user = userEvent.setup();
  await chooseOption(user, 'Share with organization', 'Research');
  await user.click(screen.getByRole('button', { name: 'Create share' }));
  await screen.findByText('Retry the same command key.');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Create share' })).toBeEnabled());
  await user.click(screen.getByRole('button', { name: 'Create share' }));
  await screen.findByText('Current access: View only');
  expect(test.writes).toHaveLength(2);
  expect(test.writes[1]?.key).toBe(test.writes[0]?.key);
});

it('rechecks owner permission before sending and clears grants after refusal', async () => {
  const test = fixture();
  test.state.shares = [grant];
  await test.open();
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Revoke' }));
  test.state.allowed = false;
  await user.click(screen.getByRole('button', { name: 'Revoke grant' }));
  await waitFor(() =>
    expect(screen.queryByText('Current access: View only')).not.toBeInTheDocument(),
  );
  expect(test.writes).toHaveLength(0);
  expect(screen.queryByRole('button', { name: 'Create share' })).not.toBeInTheDocument();
});

it('cancels a pending preflight on scope change and ignores its late reply', async () => {
  const test = fixture();
  await test.open();
  const user = userEvent.setup();
  await chooseOption(user, 'Share with organization', 'Research');
  const original = test.fetch.getMockImplementation()!;
  let finish: ((response: Response) => void) | undefined;
  let signal: AbortSignal | null | undefined;
  test.fetch.mockImplementationOnce((_url, init) => {
    signal = init?.signal;
    return new Promise((resolve) => {
      finish = resolve;
    });
  });
  await user.click(screen.getByRole('button', { name: 'Create share' }));
  await waitFor(() => expect(finish).toBeDefined());
  test.fetch.mockImplementation(original);
  await chooseOption(user, 'Share scope', 'Whole Sandbox');
  expect(signal?.aborted).toBe(true);
  await act(async () =>
    finish?.(
      Response.json({ capabilities: ['manage_sharing'], revalidate_at: null, policy_revision: 1 }),
    ),
  );
  expect(test.writes).toHaveLength(0);
});

it('reloads a conflicting revoke and requires a new explicit action with the new revision', async () => {
  const test = fixture();
  test.state.shares = [grant];
  await test.open();
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Revoke' }));
  test.state.shares = [{ ...grant, revision: 2 }];
  test.state.policy += 1;
  await user.click(screen.getByRole('button', { name: 'Revoke grant' }));
  await screen.findByRole('button', { name: 'Revoke' });
  expect(test.writes).toHaveLength(1);
  expect(test.state.shares[0]?.revoked_at).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Revoke' }));
  await user.click(screen.getByRole('button', { name: 'Revoke grant' }));
  await screen.findByText('No direct organization grants.');
  expect(test.writes.map((write) => write.revision)).toEqual(['1', '2']);
  expect(test.writes[1]?.key).not.toBe(test.writes[0]?.key);
});

it('refuses a grant snapshot buffered across a policy change or referring to another resource', async () => {
  const test = fixture();
  const original = test.fetch.getMockImplementation()!;
  test.fetch.mockImplementation(async (url, init) => {
    if (String(url).includes('/resource-shares/')) test.state.policy += 1;
    return original(url, init);
  });
  await expect(loadSharing(test.client, target, new AbortController().signal)).rejects.toThrow(
    'Sharing access changed',
  );
  test.fetch.mockImplementation(original);
  test.state.shares = [{ ...grant, selector: { kind: 'agent', agent_id: 'another_agent' } }];
  await expect(loadSharing(test.client, target, new AbortController().signal)).rejects.toThrow(
    'Sharing information changed',
  );
});

it('closes the dialog when navigating to another Agent without modifying shares', async () => {
  const test = fixture();
  const view = await test.open();
  expect(
    within(screen.getByRole('dialog')).getByText(/Retained investigation/),
  ).toBeInTheDocument();
  view.rerender(
    <SharingButton
      client={test.client}
      target={{ ...target, id: 'another_agent', name: 'Another Agent' }}
      sandbox={sandbox}
    />,
  );
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(test.writes).toHaveLength(0);
});

it('does not send after a preflight times out even when transport ignores cancellation', async () => {
  const test = fixture();
  await test.open();
  await chooseOption(userEvent.setup(), 'Share with organization', 'Research');
  let finish: ((response: Response) => void) | undefined;
  test.fetch.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  vi.useFakeTimers();
  try {
    fireEvent.click(screen.getByRole('button', { name: 'Create share' }));
    await act(async () => {});
    await act(() => vi.advanceTimersByTimeAsync(15_000));
    expect(screen.getByRole('alert')).toHaveTextContent('result could not be confirmed');
    expect(screen.getByRole('button', { name: 'Create share' })).toBeEnabled();
    await act(async () =>
      finish?.(
        Response.json({
          capabilities: ['manage_sharing'],
          revalidate_at: null,
          policy_revision: 1,
        }),
      ),
    );
    expect(test.writes).toHaveLength(0);
  } finally {
    vi.useRealTimers();
  }
});

it('does not confirm a receipt for a different organization', async () => {
  const test = fixture();
  await test.open();
  const original = test.fetch.getMockImplementation()!;
  test.fetch.mockImplementation(async (url, init) => {
    if (init?.method === 'POST')
      return Response.json({
        operation_id: 'wrong',
        share: { ...grant, target_org_id: 'another_org' },
        actor,
        at,
        policy_revision: 1,
      });
    return original(url, init);
  });
  const user = userEvent.setup();
  await chooseOption(user, 'Share with organization', 'Research');
  await user.click(screen.getByRole('button', { name: 'Create share' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'did not confirm the requested change',
  );
  expect(
    screen.queryByText('Sharing command confirmed. Current grants are shown below.'),
  ).not.toBeInTheDocument();
});
