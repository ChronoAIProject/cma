import { vi } from 'vitest';
import { createClient } from '@cma/client';
import { AgentV2Session } from './client';
import { agentTransportJournal } from './journal';
import { commandStatus } from './controls.fixture';
import { json } from './controller.fixture';

export function status(
  id: number,
  kind: 'queued' | 'completed' | 'expired' | 'starting' | 'cancelled' = 'queued',
) {
  const source = commandStatus(`resp_${id}`);
  return {
    ...source,
    receipt: { ...source.receipt, command_id: `cmd_${id}`, agent_order: id },
    status: kind === 'expired' ? 'failed' : kind === 'starting' ? 'in_progress' : kind,
    turn_sequence: kind === 'starting' || kind === 'completed' ? id : null,
    delivery:
      kind === 'starting' ? 'dispatch_fenced' : kind === 'completed' ? 'execution_observed' : null,
    observed_cursor: kind === 'completed' ? id : null,
    failure: kind === 'expired' ? 'queue_timeout' : null,
  };
}
export function entry(id: number) {
  return { response: status(id), command_revision: 7, text: `Message ${id}`, editable: true };
}
export function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
export const unavailable = (code: number, retry?: string) =>
  new Response(
    JSON.stringify({
      error: {
        type: 'server_error',
        code: 'unavailable',
        message: 'Raw diagnostics must stay private',
        param: null,
      },
    }),
    {
      status: code,
      headers: { 'content-type': 'application/json', ...(retry ? { 'retry-after': retry } : {}) },
    },
  );
export function reconciliationHarness(storage: Storage = sessionStorage) {
  let rows: ReturnType<typeof entry>[] = [];
  const currentUser = vi.fn(async () => 'alice');
  const command = vi.fn(async (id: number): Promise<Response> => json(status(id)));
  const list = vi.fn(async (after: number): Promise<Response> => {
    const remaining = rows.filter((row) => row.response.receipt.agent_order > after);
    const data = remaining.slice(0, 3);
    return json({
      object: 'cma.agent.queued_inputs',
      agent_id: 'agt_1',
      data,
      next_after: remaining.length > 3 ? data.at(-1)!.response.receipt.agent_order : null,
    });
  });
  const fetch = vi.fn<typeof globalThis.fetch>(async (input) => {
    const url = new URL(String(input), 'https://example.test');
    if (url.pathname.endsWith('/queued-inputs'))
      return list(Number(url.searchParams.get('after') ?? 0));
    const match = /\/commands\/cmd_(\d+)$/.exec(url.pathname);
    if (match) return command(Number(match[1]));
    throw new Error('Unexpected request');
  });
  const journal = agentTransportJournal(storage, 'alice', 'agt_1');
  journal.select(true);
  const session = new AgentV2Session(createClient({ fetch }), journal, 'alice', {
    storage,
    currentUser,
  });
  return {
    session,
    journal,
    command,
    list,
    fetch,
    currentUser,
    setRows: (ids: number[]) => {
      rows = ids.map(entry);
    },
  };
}
