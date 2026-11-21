import { vi } from 'vitest';
import type { AgentDetail } from '../../api/types';
import { AGENT_DETAIL } from '../../test/agent';
import { harness } from './controller.fixture';
import { loadV2Agent } from './load';
import type { AgentLoadLineage } from '../loadAgent';

export function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<Value>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

export async function setup(lineage?: AgentLoadLineage) {
  const h = harness();
  const v2 = await h.client.agentTransport!.open('agt_1');
  if (!v2) throw new Error('Expected v2 fixture');
  const progress = vi.fn();
  const abort = new AbortController();
  const start = (allowIncomplete = false) =>
    loadV2Agent(h.client, 'agt_1', abort.signal, v2, allowIncomplete, progress, lineage).then(
      (value) => ({ value, error: undefined }),
      (error: unknown) => ({ value: undefined, error }),
    );
  return { ...h, v2, progress, abort, start };
}

export const lineage: AgentLoadLineage = {
  agentId: AGENT_DETAIL.id,
  workspaceId: AGENT_DETAIL.workspace.id,
  sandboxId: AGENT_DETAIL.sandbox.id,
};

export type FixtureDetail = AgentDetail;
