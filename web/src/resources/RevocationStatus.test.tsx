import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { AgentShareLinksDialog } from './AgentShareLinks';
import { readRevocations, saveRevocation } from './revocationIntent';
import { retainTaskViewer } from '../create/taskIdentity';

const operation = 'a'.repeat(64);
const link = {
  id: 'share',
  agent_id: 'agent',
  audience: { kind: 'public' },
  mode: 'view',
  shared_by: { subject: 'owner', display_name: 'Owner', avatar_url: null },
  created_at: '2026-01-01T00:00:00Z',
  revoked_at: null,
  revision: 1,
  accepted_count: 1,
};
const pending = {
  operation_id: operation,
  selector: { kind: 'agent', agent_id: 'agent' },
  share_id: 'share',
  expected_revision: 1,
  state: 'pending',
  participants: 1,
  stopped: 0,
  unresolved_effects: 1,
  deadline_unix_ms: 11000,
  deadline_exceeded: true,
  receipt: null,
};
const actor = { kind: 'human', subject: 'owner' };
const effective = {
  ...pending,
  state: 'effective',
  stopped: 1,
  deadline_exceeded: false,
  receipt: {
    operation_id: operation,
    share: {
      id: 'share',
      owner: { kind: 'person', subject: 'owner' },
      selector: pending.selector,
      target_org_id: 'org',
      mode: 'view',
      issuer_actor: actor,
      created_at: link.created_at,
      revoked_at: link.created_at,
      revision: 2,
    },
    actor,
    at: link.created_at,
    policy_revision: 2,
  },
};
beforeEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});
function fixture() {
  const state = { deleteUnavailable: false, statusUnavailable: true, complete: false, revision: 1 };
  const writes: { key: string | null; revision: string | null }[] = [];
  const reads: string[] = [];
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    if (init?.method === 'DELETE') {
      writes.push({
        key: new Headers(init.headers).get('Idempotency-Key'),
        revision: url.searchParams.get('expected_revision'),
      });
      if (state.deleteUnavailable) throw new TypeError('lost response');
      return Response.json(
        { code: 'resource_revocation_pending', detail: 'Pending' },
        {
          status: 409,
          headers: {
            Link: `</api/v1/resource-revocations/${operation}?kind=agent&id=agent>; rel="status"`,
          },
        },
      );
    }
    if (url.pathname.includes('/resource-revocations/')) {
      reads.push(url.pathname);
      if (state.statusUnavailable)
        return Response.json({ code: 'resource_revocation_unavailable' }, { status: 503 });
      return Response.json(state.complete ? effective : pending);
    }
    if (url.pathname.endsWith('/share-links'))
      return Response.json({
        items: [{ ...link, agent_id: url.pathname.split('/')[4], revision: state.revision }],
      });
    return Response.json({ items: [] });
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const draw = (owner = 'owner') =>
    render(
      <AgentShareLinksDialog
        client={client}
        ownerId={owner}
        agentId="agent"
        open
        onClose={() => {}}
      />,
    );
  return { state, writes, reads, draw, client };
}
async function revoke() {
  const user = userEvent.setup();
  const row = await screen.findByRole('article', { name: 'Share link 1' });
  await user.click(within(row).getByRole('button', { name: 'Revoke link' }));
  await user.click(within(row).getByRole('button', { name: 'Revoke access' }));
  return user;
}
it('resumes status after remount without resending and confirms unresolved external effects truthfully', async () => {
  const test = fixture();
  const first = test.draw();
  await revoke();
  await screen.findByRole('button', { name: 'Check revocation' });
  const saved = readRevocations('owner', 'agent');
  expect(saved).toEqual([{ key: test.writes[0]!.key, shareId: 'share', revision: 1, operation }]);
  expect(screen.getByRole('button', { name: 'Revoke link' })).toBeDisabled();
  first.unmount();
  test.draw();
  await screen.findByRole('button', { name: 'Check revocation' });
  expect(test.writes).toHaveLength(1);
  test.state.statusUnavailable = false;
  test.state.complete = true;
  await userEvent.click(screen.getByRole('button', { name: 'Check revocation' }));
  await screen.findByText(
    'Access revoked. Previously submitted external requests may still complete.',
  );
  expect(readRevocations('owner', 'agent')).toEqual([]);
  expect(screen.queryByRole('article')).not.toBeInTheDocument();
  expect(test.writes).toHaveLength(1);
});
it('retains the confirmed external-effect warning after remount when status is unavailable', async () => {
  const test = fixture();
  const first = test.draw();
  await revoke();
  await screen.findByRole('button', { name: 'Check revocation' });
  test.state.statusUnavailable = false;
  test.state.complete = true;
  await userEvent.click(screen.getByRole('button', { name: 'Check revocation' }));
  await screen.findByText(
    'Access revoked. Previously submitted external requests may still complete.',
  );
  first.unmount();
  const previousReads = test.reads.length;
  test.state.statusUnavailable = true;
  test.draw();
  await screen.findByText(/Previously submitted external requests may still complete/);
  await waitFor(() => expect(test.reads.length).toBeGreaterThan(previousReads));
  expect(test.writes).toHaveLength(1);
});
it('retains key and original revision after lost response and reload, until an explicit retry', async () => {
  const test = fixture();
  test.state.deleteUnavailable = true;
  const first = test.draw();
  await revoke();
  await screen.findByRole('alert');
  const saved = readRevocations('owner', 'agent');
  expect(saved).toHaveLength(1);
  first.unmount();
  test.state.revision = 2;
  test.draw();
  await screen.findByRole('article');
  expect(test.writes).toHaveLength(1);
  test.state.deleteUnavailable = false;
  await revoke();
  await screen.findByRole('button', { name: 'Check revocation' });
  expect(test.writes).toHaveLength(2);
  expect(test.writes[1]).toEqual(test.writes[0]);
});
it('does not send or replace a request when storage cannot preserve it', async () => {
  const test = fixture();
  test.draw();
  await screen.findByRole('article');
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('storage unavailable');
  });
  await revoke();
  await screen.findByText(
    'Your revocation request could not be saved. Restore session storage before retrying.',
  );
  expect(test.writes).toEqual([]);
});
it('isolates saved operations by owner and clears them on account changes', async () => {
  retainTaskViewer('owner');
  saveRevocation('owner', 'agent', { key: 'original', shareId: 'share', revision: 1, operation });
  const test = fixture();
  test.draw('other');
  await screen.findByRole('article');
  await waitFor(() => expect(test.reads).toEqual([]));
  expect(readRevocations('other', 'agent')).toEqual([]);
  retainTaskViewer('other');
  expect(readRevocations('owner', 'agent')).toEqual([]);
  saveRevocation('other', 'agent', { key: 'second', shareId: 'share', revision: 1 });
  retainTaskViewer();
  expect(readRevocations('other', 'agent')).toEqual([]);
});

it.each(['owner', 'agent'] as const)(
  'retires the previous dialog when its %s changes without relying on a caller key',
  async (changed) => {
    const test = fixture();
    const view = test.draw();
    await revoke();
    await screen.findByRole('button', { name: 'Check revocation' });
    const saved = readRevocations('owner', 'agent');
    const originalReads = test.reads.length;
    const redraw = (owner: string, agent: string) =>
      view.rerender(
        <AgentShareLinksDialog
          client={test.client}
          ownerId={owner}
          agentId={agent}
          open
          onClose={() => {}}
        />,
      );

    redraw(changed === 'owner' ? 'other' : 'owner', changed === 'agent' ? 'other' : 'agent');
    await screen.findByRole('article');
    expect(screen.queryByLabelText('Revocation status')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Revoke link' })).toBeEnabled();
    expect(test.reads).toHaveLength(originalReads);
    expect(test.writes).toHaveLength(1);
    expect(readRevocations('owner', 'agent')).toEqual(saved);

    redraw('owner', 'agent');
    await screen.findByRole('button', { name: 'Check revocation' });
    expect(test.reads).toHaveLength(originalReads + 1);
    expect(test.writes).toHaveLength(1);
    expect(readRevocations('owner', 'agent')).toEqual(saved);
  },
);
