import { render } from '@testing-library/react';
import { vi } from 'vitest';
import { createClient, type ResourceShare } from '@cma/client';
import { CommandKeys } from '../api/client';
import { ShareScopeEditor } from './ShareScopeEditor';

export const target = { kind: 'sandbox', id: 'sandbox', name: 'Research' } as const;
export const operation = 'a'.repeat(64);
export const actor = { kind: 'human', subject: 'owner' } as const;
const at = '2026-01-01T00:00:00Z';
export const original: ResourceShare = {
  id: 'share',
  owner: { kind: 'person', subject: 'owner' },
  selector: { kind: 'sandbox', sandbox_id: target.id },
  target_org_id: 'org',
  mode: 'org_roles',
  issuer_actor: actor,
  created_at: at,
  revoked_at: null,
  revision: 1,
};
export function fixture() {
  const state = {
    allowed: true,
    unknown: false,
    success: false,
    unresolved: 1,
    statusUnavailable: true,
    effective: false,
    operation,
    wrongReceipt: false,
    mode: 'view' as ResourceShare['mode'],
    revoked: false,
    shares: [original] as ResourceShare[],
  };
  const writes: { key: string | null; revision: number; method: string; mode?: string }[] = [];
  const reads: string[] = [];
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    if (url.pathname.includes('/resource-access/'))
      return Response.json({
        capabilities: state.allowed ? ['manage_sharing'] : ['view_details'],
        policy_revision: 1,
        revalidate_at: null,
      });
    if (url.pathname.endsWith('/organizations'))
      return Response.json({
        items: [{ id: 'org', role: 'admin', display_name: 'Research team' }],
      });
    if (url.pathname.includes('/resource-revocations/')) {
      reads.push(url.pathname);
      if (state.statusUnavailable)
        return Response.json({ code: 'resource_revocation_unavailable' }, { status: 503 });
      return Response.json({
        operation_id: operation,
        selector: original.selector,
        share_id: original.id,
        expected_revision: 1,
        state: state.effective ? 'effective' : 'evidence_received',
        participants: 1,
        stopped: 1,
        unresolved_effects: state.unresolved,
        deadline_unix_ms: 11000,
        deadline_exceeded: !state.effective,
        receipt: state.effective
          ? {
              operation_id: operation,
              actor,
              at,
              policy_revision: 2,
              share: {
                ...original,
                mode: state.wrongReceipt ? 'org_roles' : state.mode,
                revoked_at: state.revoked ? at : null,
                revision: 2,
              },
            }
          : null,
      });
    }
    if (init?.method === 'PATCH' || init?.method === 'DELETE') {
      const body = init.body
        ? (JSON.parse(String(init.body)) as { expected_revision: number; mode: string })
        : undefined;
      writes.push({
        key: new Headers(init.headers).get('Idempotency-Key'),
        method: init.method,
        revision: body?.expected_revision ?? Number(url.searchParams.get('expected_revision')),
        ...(body ? { mode: body.mode } : {}),
      });
      if (state.unknown) throw new TypeError('Lost reply');
      if (state.success)
        return Response.json({
          operation_id: operation,
          actor,
          at,
          policy_revision: 2,
          share: {
            ...original,
            revision: 2,
            mode: state.mode,
            revoked_at: state.revoked ? at : null,
          },
          revocation: {
            operation_id: operation,
            kind: 'sandbox',
            id: target.id,
            share_id: original.id,
            expected_revision: 1,
          },
        });
      return Response.json(
        { code: 'resource_revocation_pending', detail: 'Pending' },
        {
          status: 409,
          headers: {
            Link: `</api/v1/resource-revocations/${state.operation}?kind=sandbox&id=sandbox>; rel="status"`,
          },
        },
      );
    }
    if (url.pathname.includes('/resource-shares/')) return Response.json({ items: state.shares });
    throw new Error(`Unexpected fixture request ${url.pathname}`);
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const draw = (owner = 'owner') =>
    render(
      <ShareScopeEditor client={client} target={target} ownerId={owner} keys={new CommandKeys()} />,
    );
  return { state, writes, reads, fetch, client, draw };
}
