import { expect, it, vi } from 'vitest';
import { createClient, ProtocolError, TransportError, ValidationError } from '../src/index.js';
import { bridge, allocation, request, response, status } from './workspace-cleanup-fixtures.js';

it('captures exact cleanup consent and key before credentials and recovers only by explicit replay', async () => {
  let release!: (credential: string) => void;
  const credentials = new Promise<string>((resolve) => {
    release = resolve;
  });
  const fetch = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('reply lost'))
    .mockResolvedValueOnce(response(status()));
  const client = createClient({
    baseUrl: 'https://fixture.test',
    credential: () => credentials,
    fetch,
  });
  const body = { ...request };
  const options = { idempotencyKey: 'original-cleanup' };
  const pending = client.bridges.requestWorkspaceCleanup(bridge, allocation, body, options);
  body.expected_bridge_revision = 99;
  body.expected_allocation_revision = 100;
  body.expected_workspace_revision = 101;
  body.child_cleanup_receipt_id = `bclj_${'9'.repeat(64)}`;
  options.idempotencyKey = 'replacement-key';
  release('selected-credential');
  await expect(pending).rejects.toBeInstanceOf(TransportError);
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(JSON.parse(fetch.mock.calls[0]![1].body)).toEqual(request);
  expect(new Headers(fetch.mock.calls[0]![1].headers).get('idempotency-key')).toBe(
    'original-cleanup',
  );
  const accepted = await client.bridges.requestWorkspaceCleanup(bridge, allocation, request, {
    idempotencyKey: 'original-cleanup',
  });
  expect(accepted).toEqual(status());
  expect(accepted.observed_at_ms).toBeLessThan(accepted.receipt!.accepted_at_ms);
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(JSON.parse(fetch.mock.calls[1]![1].body)).toEqual(request);
  expect(new Headers(fetch.mock.calls[1]![1].headers).get('idempotency-key')).toBe(
    'original-cleanup',
  );
});

it('retains exact target and receipt while progress reaches Complete and refuses contradictory proof', async () => {
  const fetch = vi.fn();
  const client = createClient({
    baseUrl: 'https://fixture.test',
    credential: () => 'selected-credential',
    fetch,
  });
  const done = { ...status(), progress: 'complete', holds: [], workspace_capacity_released: true };
  fetch.mockResolvedValueOnce(response(done, 200));
  expect(await client.bridges.readWorkspaceCleanup(bridge, allocation)).toEqual(done);
  fetch.mockResolvedValueOnce(response(status(), 200));
  await expect(
    client.bridges.requestWorkspaceCleanup(bridge, allocation, request, {
      idempotencyKey: 'same-cleanup',
    }),
  ).rejects.toBeInstanceOf(ProtocolError);
  const before = {
    ...status(),
    receipt: null,
    progress: 'not_requested',
    holds: [],
    observed_at_ms: null,
  };
  fetch.mockResolvedValueOnce(response(before, 200));
  expect(await client.bridges.readWorkspaceCleanup(bridge, allocation)).toEqual(before);
  for (const bad of [
    { ...status(), bridge_id: 'another' },
    { ...status(), allocation_id: `bral_${'3'.repeat(64)}` },
    { ...status(), target: { ...status().target, agent_id: 'replacement' } },
    { ...status(), receipt: { ...status().receipt, generation: 2 } },
    {
      ...status(),
      receipt: { ...status().receipt, request: { ...request, expected_bridge_revision: 8 } },
    },
    {
      ...status(),
      receipt: {
        ...status().receipt,
        requested_by: { kind: 'api', subject: 'manager', delegation_id: 'delegated' },
      },
    },
    {
      ...status(),
      receipt: {
        ...status().receipt,
        requested_by: { kind: 'human', subject: 'manager', credential: 'hidden' },
      },
    },
    { ...status(), holds: ['workspace_busy', 'child_cleanup_incomplete'] },
    { ...status(), holds: ['workspace_busy', 'workspace_busy'] },
    { ...status(), progress: 'complete' },
    { ...done, holds: ['workspace_busy'] },
    { ...done, history_retained: false },
    { ...done, parent_retained: false },
    { ...status(), observed_at_ms: null },
    { ...status(), workspace_revision: null },
    { ...status(), child_cleanup_receipt_id: null },
    { ...status(), eligible: true },
    { ...status(), child_cleanup_receipt_id: `bclj_${'8'.repeat(64)}` },
    {
      ...status(),
      receipt: { ...status().receipt, request: { ...request, expected_workspace_revision: 8 } },
    },
    {
      ...status(),
      receipt: {
        ...status().receipt,
        request: { ...request, child_cleanup_receipt_id: `bclj_${'9'.repeat(64)}` },
      },
    },
    before,
    { ...status(), private_evidence: {} },
  ]) {
    fetch.mockResolvedValueOnce(response(bad));
    await expect(
      client.bridges.requestWorkspaceCleanup(bridge, allocation, request, {
        idempotencyKey: 'same-cleanup',
      }),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
});

it('refuses invalid keys, revisions, targeting fields and limits before credential lookup or HTTP', () => {
  const token = vi.fn(() => 'selected-credential');
  const fetch = vi.fn();
  const client = createClient({ baseUrl: 'https://fixture.test', credential: token, fetch });
  for (const key of ['', 'x'.repeat(201), 'contains space', 'line\nbreak'])
    expect(() =>
      client.bridges.requestWorkspaceCleanup(bridge, allocation, request, { idempotencyKey: key }),
    ).toThrow(ValidationError);
  for (const body of [
    { ...request, action: 'remove_child' },
    { ...request, expected_workspace_revision: 0 },
    { ...request, child_cleanup_receipt_id: 'bwcl_wrong' },
    { ...request, expected_bridge_revision: 0 },
    { ...request, expected_allocation_revision: Number.MAX_SAFE_INTEGER + 1 },
    { ...request, target: status().target },
  ])
    expect(() =>
      client.bridges.requestWorkspaceCleanup(bridge, allocation, body as never, {
        idempotencyKey: 'retained',
      }),
    ).toThrow(ValidationError);
  expect(token).not.toHaveBeenCalled();
  expect(fetch).not.toHaveBeenCalled();
});

it('requires current eligibility inputs while retaining accepted and Complete receipts after runtime changes', async () => {
  const fetch = vi.fn();
  const client = createClient({ baseUrl: 'https://fixture.test', fetch });
  const eligible = {
    ...status(),
    receipt: null,
    progress: 'not_requested',
    holds: [],
    eligible: true,
  };
  fetch.mockResolvedValueOnce(response(eligible, 200));
  expect(await client.bridges.readWorkspaceCleanup(bridge, allocation)).toEqual(eligible);
  for (const change of [
    { workspace_revision: null },
    { child_cleanup_receipt_id: null },
    { runtime_supported: false },
    { holds: ['runtime_unsupported'] },
  ]) {
    fetch.mockResolvedValueOnce(response({ ...eligible, ...change }, 200));
    await expect(client.bridges.readWorkspaceCleanup(bridge, allocation)).rejects.toBeInstanceOf(
      ProtocolError,
    );
  }
  for (const progress of [
    'requested',
    'admission_closed',
    'removal_started',
    'runtime_removed',
    'complete',
  ]) {
    const retained = {
      ...status(),
      workspace_revision: 8,
      runtime_supported: false,
      progress,
      holds: [],
      workspace_capacity_released: progress === 'complete',
    };
    fetch.mockResolvedValueOnce(response(retained));
    expect(
      await client.bridges.requestWorkspaceCleanup(bridge, allocation, request, {
        idempotencyKey: 'original',
      }),
    ).toEqual(retained);
  }
  for (const field of [
    'workspace_revision',
    'child_cleanup_receipt_id',
    'receipt',
    'observed_at_ms',
  ]) {
    const bad: Record<string, unknown> = { ...status() };
    delete bad[field];
    fetch.mockResolvedValueOnce(response(bad, 200));
    await expect(client.bridges.readWorkspaceCleanup(bridge, allocation)).rejects.toBeInstanceOf(
      ProtocolError,
    );
  }
});
