import { vi } from 'vitest';
import {
  createClient,
  type BridgeWorkspaceCleanupStatus,
  type BridgeChildCleanupStatus,
  type BridgeCleanupAllocationPage,
} from '@chronoai/cma-sdk';
import workspaceCorpus from '../../../api/crates/layer1-public/tests/fixtures/public-contract/bridge-workspace-cleanup.json';
import childCorpus from '../../../api/crates/layer1-public/tests/fixtures/public-contract/bridge-child-cleanup.json';

type Mutable<T> = { -readonly [K in keyof T]: T[K] };

export function workspaceCleanupFixture() {
  const sample = <T>(name: string): T =>
    structuredClone(
      workspaceCorpus.cases.find((value) => value.id === `bridge-workspace-cleanup-${name}`)!
        .response.body,
    ) as T;
  const before = sample<Mutable<BridgeWorkspaceCleanupStatus>>('unrequested');
  const accepted = sample<BridgeWorkspaceCleanupStatus>('explicit-acceptance');
  const child = structuredClone(
    childCorpus.cases.find((value) => value.id === 'bridge-cleanup-explicit-acceptance')!.response
      .body,
  ) as Mutable<BridgeChildCleanupStatus>;
  child.target = { ...before.target };
  child.receipt = {
    ...child.receipt!,
    id: before.child_cleanup_receipt_id!,
    target: { ...before.target },
  };
  child.progress = 'complete';
  child.holds = [];
  child.agent_capacity_released = true;
  const originalPage = structuredClone(
    childCorpus.cases.find((value) => value.id === 'bridge-cleanup-list')!.response.body,
  ) as BridgeCleanupAllocationPage;
  const page = {
    ...originalPage,
    items: originalPage.items.map((item) => ({ ...item, target: { ...before.target } })),
  };
  const allocation = page.items[0]!;
  const writes: { body: unknown; key: string | null }[] = [];
  const state = {
    child,
    workspace: before,
    accepted: undefined as BridgeWorkspaceCleanupStatus | undefined,
    loseReply: false,
    failure: 0,
    denied: false,
    corruptTarget: false,
    hold: undefined as Promise<void> | undefined,
    signal: undefined as AbortSignal | null | undefined,
  };
  const fetch = vi.fn(async (input: RequestInfo | URL, options: RequestInit = {}) => {
    const url = new URL(String(input));
    if (state.denied) return Response.json({ code: 'bridge_not_found' }, { status: 404 });
    if (url.pathname.endsWith('/allocations')) return Response.json(page);
    if (url.pathname.endsWith('/child-cleanup')) {
      if (options.method === 'POST')
        throw new Error('Workspace consent cannot submit child cleanup');
      return Response.json(state.child);
    }
    if (!url.pathname.endsWith('/workspace-cleanup')) throw new Error('Unexpected fixture request');
    if (options.method !== 'POST') {
      const value = { ...structuredClone(state.accepted ?? state.workspace) };
      if (state.accepted !== undefined) {
        value.workspace_revision = state.workspace.workspace_revision;
        value.runtime_supported = state.workspace.runtime_supported;
      }
      if (state.corruptTarget)
        value.target = { ...value.target, workspace_id: 'another-workspace' };
      return Response.json(value);
    }
    const body = JSON.parse(String(options.body));
    writes.push({ body, key: new Headers(options.headers).get('Idempotency-Key') });
    state.signal = options.signal;
    state.accepted = {
      ...structuredClone(accepted),
      receipt: { ...accepted.receipt!, request: body },
    };
    if (state.hold !== undefined) await state.hold;
    if (state.loseReply) {
      state.loseReply = false;
      throw new TypeError('Acceptance reply lost');
    }
    if (state.failure !== 0) {
      const failure = state.failure;
      state.failure = 0;
      return Response.json(
        { code: 'bridge_workspace_cleanup_conflict', detail: 'private server detail' },
        { status: failure },
      );
    }
    return Response.json(
      {
        ...state.accepted,
        runtime_supported: state.workspace.runtime_supported,
        workspace_revision: state.workspace.workspace_revision,
      },
      { status: 202 },
    );
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const props = {
    client,
    viewerId: 'manager',
    id: allocation.bridge_id,
    ownerScope: 'person:manager',
    revision: accepted.receipt!.request.expected_bridge_revision,
    allowed: true,
    onNavigate: vi.fn(),
    onAccessChanged: vi.fn(),
  };
  return { props, state, fetch, writes, allocation, original: accepted.receipt!.request };
}
