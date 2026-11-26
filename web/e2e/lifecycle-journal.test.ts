// @vitest-environment node
import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { APIRequestContext } from '@playwright/test';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Journal } from './journal';

const subject = 'journey-owner';
const baseURL = 'https://cma.example.test';
const response = (body: unknown, status = 200) => ({
  ok: () => status >= 200 && status < 300,
  status: () => status,
  json: async () => body,
});
function fixture() {
  const directory = mkdtempSync(join(tmpdir(), 'cma-lifecycle-journal-'));
  const journal = new Journal(directory, 'journey-run');
  const rows = () =>
    readFileSync(join(directory, 'cleanup.ndjson'), 'utf8')
      .trim()
      .split('\n')
      .map((row) => JSON.parse(row) as Record<string, unknown>);
  return { journal, rows };
}
afterEach(() => vi.useRealTimers());

describe('current public creation receipts', () => {
  it('tracks the operation before its Agent exists and binds later observations to its owner', () => {
    const { journal } = fixture();
    const path = '/api/agent-manager/v1/agent-lifecycle/operation-fixture';
    const operation = { id: 'operation-fixture', status: 'running', agent_id: 'agent-fixture' };
    journal.observe(path, 'GET', 200, operation, subject);
    journal.observe('/api/agent-manager/v1/agents', 'POST', 403, operation, subject);
    expect(journal.resources).toEqual([]);
    journal.observe(
      '/api/agent-manager/v1/agents',
      'POST',
      202,
      { id: operation.id, status: 'provisioning' },
      subject,
    );
    expect(journal.resources).toEqual([{ kind: 'agent-lifecycle', id: operation.id, subject }]);
    journal.observe(path, 'GET', 200, operation, 'other-owner');
    journal.observe(path, 'GET', 200, { ...operation, id: 'other-operation' }, subject);
    expect(journal.resources).toHaveLength(1);
    journal.observe(path, 'GET', 200, operation, subject);
    journal.observe(path, 'GET', 200, operation, subject);
    expect(journal.resources).toEqual([
      { kind: 'agent-lifecycle', id: operation.id, subject },
      { kind: 'agents', id: operation.agent_id, subject },
    ]);
  });

  it('records inline Workspace admission through the Agent Manager public prefix', () => {
    const { journal } = fixture();
    journal.observe(
      '/api/agent-manager/v1/sandboxes/sandbox-fixture/workspaces/inline',
      'POST',
      201,
      { id: 'workspace-fixture' },
      subject,
    );
    journal.observe(
      '/api/agent-manager/v1/workspaces/workspace-fixture/agents/inline',
      'POST',
      201,
      { id: 'not-a-public-route' },
      subject,
    );
    expect(journal.resources).toEqual([{ kind: 'workspaces', id: 'workspace-fixture', subject }]);
  });

  it('discovers and cleans the Agent after a lost task response before touching its parent', async () => {
    const { journal, rows } = fixture();
    journal.own('workspaces', 'workspace-fixture', subject);
    journal.own('agent-lifecycle', 'operation-fixture', subject);
    const get = vi
      .fn()
      .mockResolvedValueOnce(response({ subject }))
      .mockResolvedValueOnce(
        response({ id: 'operation-fixture', status: 'succeeded', agent_id: 'agent-fixture' }),
      )
      .mockResolvedValueOnce(response({ owner: subject, status: 'stopped' }))
      .mockResolvedValueOnce(response({}, 404))
      .mockResolvedValueOnce(response({ owner: subject }))
      .mockResolvedValueOnce(response({}, 404));
    const fetch = vi.fn().mockResolvedValue(response({}, 202));
    const post = vi.fn();
    await journal.cleanup({ get, fetch, post } as unknown as APIRequestContext, baseURL, subject);
    expect(fetch.mock.calls.map(([url]) => url)).toEqual([
      `${baseURL}/api/v1/agents/agent-fixture`,
      `${baseURL}/api/v1/workspaces/workspace-fixture`,
    ]);
    expect(post).not.toHaveBeenCalled();
    expect(rows()).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          action: 'lifecycle_observed',
          id: 'operation-fixture',
          state: 'succeeded',
          agent_id: 'agent-fixture',
        }),
        expect.objectContaining({ action: 'cleanup_complete', id: 'agent-fixture' }),
      ]),
    );
  });

  it.each(['queued', 'running', 'waiting_for_connection', 'succeeded'])(
    'keeps parents when %s creation has no proved terminal Agent',
    async (status) => {
      vi.useFakeTimers();
      const { journal, rows } = fixture();
      journal.own('agent-lifecycle', 'operation-fixture', subject);
      journal.own('workspaces', 'workspace-fixture', subject);
      const get = vi
        .fn()
        .mockResolvedValueOnce(response({ subject }))
        .mockResolvedValue(response({ id: 'operation-fixture', status }));
      const fetch = vi.fn();
      const result = journal
        .cleanup({ get, fetch } as unknown as APIRequestContext, baseURL, subject)
        .then(
          () => undefined,
          (error: unknown) => error,
        );
      await vi.advanceTimersByTimeAsync(91_000);
      expect(await result).toBeInstanceOf(Error);
      expect(fetch).not.toHaveBeenCalled();
      expect(rows().at(-1)).toMatchObject({
        action: 'cleanup_blocked',
        kind: 'agent-lifecycle',
        id: 'operation-fixture',
      });
    },
  );
});
