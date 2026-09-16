import { vi } from 'vitest';
import {
  createClient,
  type BridgeChildCleanupStatus,
  type BridgeCleanupAllocationPage,
  type BridgeCleanupAllocation,
} from '@chronoai/cma-sdk';
import corpus from '../../../api/crates/layer1-public/tests/fixtures/public-contract/bridge-child-cleanup.json';
const sample = <T>(name: string): T =>
  structuredClone(
    corpus.cases.find((value) => value.id === `bridge-cleanup-${name}`)!.response.body,
  ) as T;
export function cleanupFixture(
  override: Partial<Pick<BridgeCleanupAllocation, 'placement' | 'state'>> = {},
) {
  const originalPage = sample<BridgeCleanupAllocationPage>('list');
  const page = {
    ...originalPage,
    items: originalPage.items.map((item) => ({ ...item, ...override })),
  };
  const allocation = page.items[0]!;
  const original = sample<BridgeChildCleanupStatus>('explicit-acceptance');
  const before = sample<BridgeChildCleanupStatus>('unrequested');
  const writes: { body: unknown; key: string | null }[] = [];
  const state = {
    emptyFirst: false,
    loseReply: false,
    conflict: false,
    denied: false,
    corruptTarget: false,
    accepted: undefined as BridgeChildCleanupStatus | undefined,
    hold: undefined as Promise<void> | undefined,
    signal: undefined as AbortSignal | null | undefined,
  };
  const fetch = vi.fn(async (input: RequestInfo | URL, options: RequestInit = {}) => {
    const url = new URL(String(input));
    if (state.denied)
      return Response.json(
        { code: 'bridge_child_cleanup_not_found', detail: 'Not accessible.' },
        { status: 404 },
      );
    if (url.pathname.endsWith('/allocations')) {
      return Response.json(
        state.emptyFirst && !url.searchParams.has('cursor')
          ? { items: [], next_cursor: 'opaque/+/v1?part=1' }
          : page,
      );
    }
    if (options.method !== 'POST') {
      let value = structuredClone(state.accepted ?? before);
      if (state.corruptTarget)
        value = { ...value, target: { ...value.target, agent_id: 'another-agent' } };
      return Response.json(value);
    }
    const body = JSON.parse(String(options.body));
    writes.push({ body, key: new Headers(options.headers).get('Idempotency-Key') });
    state.signal = options.signal;
    state.accepted = {
      ...structuredClone(original),
      receipt: { ...original.receipt!, request: body },
    };
    if (state.hold !== undefined) await state.hold;
    if (state.loseReply) {
      state.loseReply = false;
      throw new TypeError('Response lost');
    }
    if (state.conflict) {
      state.conflict = false;
      return Response.json(
        { code: 'bridge_child_cleanup_conflict', detail: 'private server detail' },
        { status: 409 },
      );
    }
    return Response.json(state.accepted, { status: 202 });
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const props = {
    client,
    viewerId: 'manager',
    id: allocation.bridge_id,
    ownerScope: 'person:manager',
    revision: original.receipt!.request.expected_bridge_revision,
    allowed: true,
    onNavigate: vi.fn(),
    onAccessChanged: vi.fn(),
  };
  return { page, allocation, writes, state, fetch, props };
}
