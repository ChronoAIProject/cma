// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { createClient, ProtocolError, type V2AgentStreamFrame } from '../index';
import fixture from '../../test/externalApprovalProjection.fixture.json';
import { externalApprovalProjection } from '../agent-events-v2/external-approval';

const agent = fixture.waiting_snapshot.agent_id;
const headers = fixture.response_headers;
const encoded = (frame: typeof fixture.live_requested | typeof fixture.live_resolved) =>
  `event: ${frame.type}\nid: ${frame.data.cursor}\ndata: ${JSON.stringify(frame.data)}\n\n`;
function setup(value: unknown = fixture.waiting_snapshot, responseHeaders: HeadersInit = headers) {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockImplementation(async () => Response.json(value, { headers: responseHeaders }));
  return { fetch, api: createClient({ fetch }).agentEventsV2 };
}

it('requires explicit opt-in and affirmative support while preserving all existing preferences', async () => {
  const h = setup();
  await expect(h.api.snapshot(agent)).rejects.toBeInstanceOf(ProtocolError);
  const page = await h.api.snapshot(agent, {
    externalApprovals: true,
    toolOutput: true,
    retainedOutput: true,
    messageBindings: true,
  });
  expect(page.items[8]?.kind).toEqual(fixture.live_requested.data.kind);
  expect(page.externalApprovalProjection).toEqual({
    supported: true,
    canReview: true,
    reviewOrigin: headers['CMA-Approval-Review-Origin'],
  });
  expect(new Headers(h.fetch.mock.calls[1]?.[1]?.headers).get('Prefer')?.split(', ')).toEqual([
    'cma-external-approval-v1',
    'cma-message-bindings',
    'cma-tool-output',
    'cma-retained-output',
  ]);
});

it.each(['', 'cma-external-approval-v2', 'cma-external-approvals-v1'])(
  'refuses unacknowledged external data without applying the page %#',
  async (acknowledgment) => {
    const responseHeaders = new Headers();
    if (acknowledgment) responseHeaders.set('Preference-Applied', acknowledgment);
    const h = setup(fixture.waiting_snapshot, responseHeaders);
    const onProjection = vi.fn();
    await expect(
      h.api.snapshot(agent, {
        externalApprovals: true,
        onExternalApprovalProjection: onProjection,
      }),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(onProjection).not.toHaveBeenCalled();
  },
);

it('accepts an older server ignoring Prefer for an unaffected history without inheriting policy', async () => {
  const h = setup(
    {
      ...fixture.waiting_snapshot,
      items: fixture.waiting_snapshot.items.slice(0, 8),
      through_cursor: fixture.waiting_snapshot.items[7]!.cursor,
    },
    {},
  );
  const page = await h.api.snapshot(agent, { externalApprovals: true });
  expect(page.externalApprovalProjection).toEqual({ supported: false, canReview: false });
});

it.each([
  { 'CMA-Approval-Review-Origin': '' },
  { 'CMA-Approval-Review-Origin': 'http://identity.example.test' },
  { 'CMA-Approval-Review-Origin': 'https://name@identity.example.test' },
  { 'CMA-Approval-Review-Origin': 'https://identity.example.test/assistant/approvals' },
  { 'CMA-External-Approval-Action': 'none' },
  { 'CMA-External-Approval-Action': '' },
])(
  'keeps acknowledged waiting non-actionable for an invalid/missing/read-only policy %#',
  async (changes) => {
    const h = setup(fixture.waiting_snapshot, { ...headers, ...changes });
    const page = await h.api.snapshot(agent, { externalApprovals: true });
    expect(page.externalApprovalProjection?.supported).toBe(true);
    expect(page.externalApprovalProjection?.canReview).toBe(false);
  },
);

it.each([
  { revision: 0 },
  { revision: Number.MAX_SAFE_INTEGER + 1 },
  { provider: 'unknown' },
  { approval_id: 'x'.repeat(129) },
  { waiting_since: '2026-01-01' },
  { deadline_at: '2025-12-31T00:00:00.000Z' },
  { approve_url: 'x'.repeat(2049) },
  { extra: 'unsupported' },
  { turn_sequence: 99 },
])('preserves strict external kind and chronology boundaries %#', async (change) => {
  const value = structuredClone(fixture.waiting_snapshot);
  Object.assign(value.items[8]!.kind, change);
  await expect(
    setup(value).api.snapshot(agent, { externalApprovals: true }),
  ).rejects.toBeInstanceOf(ProtocolError);
});

it('never accepts an external-row response binding or relaxed native schema', async () => {
  const value = structuredClone(fixture.waiting_snapshot);
  Object.assign(value.items[8]!, { response_binding: value.items[2]!.response_binding });
  await expect(
    setup(value).api.snapshot(agent, { externalApprovals: true }),
  ).rejects.toBeInstanceOf(ProtocolError);
  const native = structuredClone(fixture.waiting_snapshot);
  Object.assign(native.items[8]!, {
    kind: {
      type: 'approval_requested',
      approval_id: 'native',
      action: null,
      detail: null,
      approve_url: 'https://identity.example.test/assistant/approvals',
    },
  });
  await expect(
    setup(native).api.snapshot(agent, { externalApprovals: true }),
  ).rejects.toBeInstanceOf(ProtocolError);
});

it('decodes real SSE with response policy before events and acknowledges only accepted durable rows', async () => {
  const h = setup();
  h.fetch.mockResolvedValue(
    new Response(encoded(fixture.live_requested) + encoded(fixture.live_resolved), {
      headers: { ...headers, 'content-type': 'text/event-stream' },
    }),
  );
  const seen: string[] = [];
  const frames: V2AgentStreamFrame[] = [];
  await h.api.events(agent, {
    externalApprovals: true,
    afterCursor: fixture.waiting_snapshot.items[7]!.cursor,
    onExternalApprovalProjection: (projection) => {
      expect(projection.canReview).toBe(true);
      seen.push('policy');
    },
    onEvent: (frame) => {
      frames.push(frame);
      seen.push('event');
    },
    onCursor: (cursor) => {
      seen.push(cursor);
    },
  });
  expect(frames.map((frame) => frame.data)).toEqual([
    fixture.live_requested.data,
    fixture.live_resolved.data,
  ]);
  expect(seen).toEqual([
    'policy',
    'event',
    fixture.live_requested.data.cursor,
    'event',
    fixture.live_resolved.data.cursor,
  ]);
});

it('rejects an unnegotiated live external row without a cursor or action', async () => {
  const h = setup();
  h.fetch.mockResolvedValue(
    new Response(encoded(fixture.live_requested), {
      headers: { 'content-type': 'text/event-stream' },
    }),
  );
  const onCursor = vi.fn();
  const onEvent = vi.fn();
  const policy = vi.fn();
  await expect(
    h.api.events(agent, {
      externalApprovals: true,
      afterCursor: fixture.waiting_snapshot.items[7]!.cursor,
      onEvent,
      onCursor,
      onExternalApprovalProjection: policy,
    }),
  ).rejects.toBeInstanceOf(ProtocolError);
  expect(onCursor).not.toHaveBeenCalled();
  expect(onEvent).not.toHaveBeenCalled();
  expect(policy).toHaveBeenCalledExactlyOnceWith({ supported: false, canReview: false });
});

it('ignores even a valid acknowledgment when the caller did not opt in', () => {
  expect(externalApprovalProjection(new Headers(headers), false)).toEqual({
    supported: false,
    canReview: false,
  });
});
