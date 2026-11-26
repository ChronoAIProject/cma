import { readRevocationEvidence } from './revocationEvidence';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { AgentShareLinksDialog } from './AgentShareLinks';
import { readRevocations } from './revocationIntent';
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
  retainTaskViewer('owner');
});
function fixture() {
  const state = {
    deleteCompleted: false,
    deleteUnavailable: false,
    statusUnavailable: true,
    complete: false,
    revision: 1,
    unresolved: 1,
  };
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
      if (state.deleteCompleted)
        return Response.json({
          ...link,
          revision: 2,
          revoked_at: link.created_at,
          revocation: {
            operation_id: operation,
            kind: 'agent',
            id: 'agent',
            share_id: 'share',
            expected_revision: 1,
          },
        });
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
      return Response.json({
        ...(state.complete ? effective : pending),
        unresolved_effects: state.unresolved,
      });
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

it('retains the Agent operation after a successful original-key replay with unresolved effects', async () => {
  const test = fixture();
  test.state.deleteUnavailable = true;
  const first = test.draw();
  await revoke();
  await screen.findByRole('alert');
  const saved = readRevocations('owner', 'agent');
  expect(saved).toHaveLength(1);
  first.unmount();
  test.state.deleteUnavailable = false;
  test.state.deleteCompleted = true;
  test.state.statusUnavailable = false;
  test.state.complete = true;
  test.draw();
  await revoke();
  await screen.findByText(
    /Access revoked. Previously submitted external requests may still complete/,
  );
  expect(test.writes).toHaveLength(2);
  expect(test.writes[1]).toEqual(test.writes[0]);
  const pending = readRevocations('owner', 'agent');
  const evidence = readRevocationEvidence({ owner: 'owner', kind: 'agent', id: 'agent' });
  expect(test.reads.length).toBeGreaterThan(0);
  expect(pending.length > 0 || evidence.some((row) => row.operation === operation)).toBe(true);
});

for (const control of ['zero', 'unavailable', 'operation-save', 'history-save'] as const) {
  it(`retains Agent success recovery under ${control}`, async () => {
    const test = fixture();
    Object.assign(test.state, {
      deleteCompleted: true,
      statusUnavailable: control === 'unavailable',
      complete: true,
      unresolved: control === 'zero' ? 0 : 1,
    });
    const setItem = Storage.prototype.setItem;
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage, key, value) {
      if (
        (control === 'operation-save' &&
          key.startsWith('cma:resource-revocation:') &&
          value.includes(operation)) ||
        (control === 'history-save' && key.startsWith('cma:resource-revocation:observed:'))
      )
        throw new Error('storage unavailable');
      setItem.call(this, key, value);
    });
    test.draw();
    await revoke();
    if (control === 'zero') {
      await screen.findByText('Access revoked.');
      expect(readRevocations('owner', 'agent')).toEqual([]);
      expect(readRevocationEvidence({ owner: 'owner', kind: 'agent', id: 'agent' })).toEqual([]);
    } else {
      await screen.findByText(
        control === 'unavailable'
          ? /Revocation could not be confirmed. Your original request is saved/
          : /saved recovery information could not be updated/,
      );
      expect(readRevocations('owner', 'agent')).toEqual([
        expect.objectContaining({ shareId: 'share', revision: 1 }),
      ]);
      if (control !== 'operation-save')
        expect(readRevocations('owner', 'agent')[0]?.operation).toBe(operation);
    }
    expect(test.reads.length).toBeGreaterThan(0);
    expect(test.writes).toHaveLength(1);
  });
}
