// @vitest-environment node
import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import type { APIRequestContext } from '@playwright/test';
import { Journal, launchIsQuiescent } from './journal';

describe('scoped resource journal', () => {
  it.each(['agent', 'workspace', 'sandbox'])(
    'journals only the real %s profile fork route',
    (kind) => {
      const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
      journal.observe(
        `/api/v1/marketplace/${kind}-profiles/public-profile/fork`,
        'POST',
        201,
        { id: 'created-fork' },
        'owner-test',
      );
      journal.observe(
        `/api/v1/marketplace/${kind}/public-profile/fork`,
        'POST',
        201,
        { id: 'not-a-real-route' },
        'owner-test',
      );
      expect(journal.resources).toEqual([
        { kind: `${kind}-profiles`, id: 'created-fork', subject: 'owner-test' },
      ]);
    },
  );
  it('treats running, retryable and ambiguous terminal Launches as unsafe for cleanup', () => {
    for (const state of [
      'draft',
      'resolving',
      'sandbox_pending',
      'workspace_pending',
      'agent_pending',
      'message_pending',
      'failed_retryable',
    ])
      expect(launchIsQuiescent({ state })).toBe(false);
    for (const state of ['ready', 'completed']) expect(launchIsQuiescent({ state })).toBe(true);
    for (const state of ['cancelled', 'failed_terminal']) {
      expect(launchIsQuiescent({ state })).toBe(true);
      expect(
        launchIsQuiescent({
          state,
          started_at: 'recorded',
          resolution: {
            sandbox: { state: 'created' },
            workspace: { state: 'pending' },
            agent: { state: 'pending' },
          },
        }),
      ).toBe(false);
      expect(
        launchIsQuiescent({
          state,
          started_at: 'recorded',
          resolution: {
            sandbox: { state: 'created' },
            workspace: { state: 'reused' },
            agent: { state: 'created' },
          },
        }),
      ).toBe(true);
    }
  });
  it.each(['v1', 'v2'])(
    'tracks only admitted %s Launch resolutions and never owns reused resources',
    (version) => {
      const directory = mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-'));
      const journal = new Journal(directory, 'test-run');
      const body = {
        id: 'launch-test',
        resolution: {
          sandbox: { state: 'reused', id: 'existing-sandbox' },
          workspace: { state: 'created', id: 'workspace-test' },
          agent: { state: 'pending' },
        },
      };
      journal.observe('/api/v1/launches/launch-test', 'GET', 200, body, 'visitor-test');
      expect(journal.resources).toEqual([]);
      journal.observe(
        `/api/${version}/triggers/public/public-test/launches`,
        'POST',
        202,
        body,
        'visitor-test',
      );
      expect(journal.resources.map((row) => row.kind)).toEqual(['launches', 'workspaces']);
      journal.observe('/api/v1/launches/launch-test', 'GET', 200, body, 'visitor-test');
      expect(journal.resources).toHaveLength(2);
      expect(readFileSync(join(directory, 'cleanup.ndjson'), 'utf8')).not.toContain(
        'existing-sandbox',
      );
    },
  );
  it('does not own refused, read-only or unsupported Launch routes', () => {
    const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
    for (const version of ['v1', 'v2']) {
      const path = `/api/${version}/triggers/public/public-test/launches`;
      for (const status of [400, 401, 403, 409, 422, 429, 503])
        journal.observe(path, 'POST', status, { id: 'not-admitted' }, 'visitor-test');
      journal.observe(path, 'GET', 200, { id: 'read-only' }, 'visitor-test');
      journal.observe(`${path}/extra`, 'POST', 201, { id: 'wrong-route' }, 'visitor-test');
    }
    journal.observe(
      '/api/v3/triggers/public/public-test/launches',
      'POST',
      201,
      { id: 'unsupported' },
      'visitor-test',
    );
    expect(journal.resources).toEqual([]);
  });
  it('does not journal refusals or an arbitrary existing resource read', () => {
    const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
    journal.observe('/api/v1/sandboxes', 'POST', 403, { id: 'not-created' }, 'visitor-test');
    journal.observe('/api/v1/sandboxes/existing', 'GET', 200, { id: 'existing' }, 'visitor-test');
    expect(journal.resources).toEqual([]);
    expect(() => journal.own('sandboxes', '../unsafe', 'visitor-test')).toThrow('identifier');
  });
  it('refuses cleanup before mutation if the live account differs', async () => {
    const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
    journal.own('sandboxes', 'created-sandbox', 'visitor-test');
    const api = {
      get: vi
        .fn()
        .mockResolvedValue({ ok: () => true, json: async () => ({ subject: 'another-owner' }) }),
      fetch: vi.fn(),
      post: vi.fn(),
    };
    await expect(
      journal.cleanup(api as unknown as APIRequestContext, 'https://example.test', 'visitor-test'),
    ).rejects.toThrow('live identity');
    expect(api.fetch).not.toHaveBeenCalled();
    expect(api.post).not.toHaveBeenCalled();
  });
  it('stops only a run-created Agent before deletion and verifies its removal', async () => {
    const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
    journal.own('agents', 'created-agent', 'visitor-test');
    const reply = (body: unknown, status = 200) => ({
      ok: () => status < 400,
      status: () => status,
      json: async () => body,
    });
    const api = {
      get: vi
        .fn()
        .mockResolvedValueOnce(reply({ subject: 'visitor-test' }))
        .mockResolvedValueOnce(reply({ id: 'created-agent', status: 'ready' }))
        .mockResolvedValueOnce(reply({ id: 'created-agent', status: 'stopped' }))
        .mockResolvedValueOnce(reply({}, 404)),
      post: vi.fn().mockResolvedValue(reply({}, 202)),
      fetch: vi.fn().mockResolvedValue(reply({}, 202)),
    };
    await journal.cleanup(
      api as unknown as APIRequestContext,
      'https://example.test',
      'visitor-test',
    );
    expect(api.post).toHaveBeenCalledWith(
      'https://example.test/api/v1/agents/created-agent/stop',
      expect.objectContaining({ data: {} }),
    );
    expect(api.fetch).toHaveBeenCalledWith(
      'https://example.test/api/v1/agents/created-agent',
      expect.objectContaining({ method: 'DELETE' }),
    );
  });
  it('harvests late children after cancel409 and waits for a quiescent Launch before deleting', async () => {
    vi.useFakeTimers();
    try {
      const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
      journal.own('launches', 'created-launch', 'visitor-test');
      const reply = (body: unknown, status = 200) => ({
        ok: () => status < 400,
        status: () => status,
        json: async () => body,
      });
      const resolution = {
        sandbox: { state: 'reused', id: 'existing-sandbox' },
        workspace: { state: 'pending' },
        agent: { state: 'pending' },
      };
      const api = {
        get: vi
          .fn()
          .mockResolvedValueOnce(reply({ subject: 'visitor-test' }))
          .mockResolvedValueOnce(
            reply({ id: 'created-launch', state: 'workspace_pending', resolution }),
          )
          .mockResolvedValueOnce(
            reply({
              id: 'created-launch',
              state: 'completed',
              resolution: {
                ...resolution,
                workspace: { state: 'created', id: 'late-workspace' },
                agent: { state: 'reused', id: 'existing-agent' },
              },
            }),
          )
          .mockResolvedValueOnce(reply({ id: 'late-workspace', status: 'ready' }))
          .mockResolvedValueOnce(reply({}, 404)),
        post: vi.fn().mockResolvedValue(reply({}, 409)),
        fetch: vi.fn().mockResolvedValue(reply({}, 202)),
      };
      const cleanup = journal.cleanup(
        api as unknown as APIRequestContext,
        'https://example.test',
        'visitor-test',
      );
      await vi.advanceTimersByTimeAsync(0);
      expect(api.fetch).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1000);
      await cleanup;
      expect(api.fetch).toHaveBeenCalledTimes(1);
      expect(api.fetch).toHaveBeenCalledWith(
        'https://example.test/api/v1/workspaces/late-workspace',
        expect.objectContaining({ method: 'DELETE' }),
      );
    } finally {
      vi.useRealTimers();
    }
  });
  it.each(['cancelled', 'failed_terminal'])(
    'refuses child cleanup for a started %s Launch whose side effects remain ambiguous',
    async (state) => {
      const journal = new Journal(mkdtempSync(join(tmpdir(), 'cma-e2e-ledger-')), 'test-run');
      journal.own('launches', 'created-launch', 'visitor-test');
      const reply = (body: unknown) => ({
        ok: () => true,
        status: () => 200,
        json: async () => body,
      });
      const api = {
        get: vi
          .fn()
          .mockResolvedValueOnce(reply({ subject: 'visitor-test' }))
          .mockResolvedValueOnce(
            reply({
              id: 'created-launch',
              state,
              started_at: 'recorded',
              resolution: {
                sandbox: { state: 'created', id: 'created-sandbox' },
                workspace: { state: 'pending' },
                agent: { state: 'pending' },
              },
            }),
          ),
        post: vi.fn().mockResolvedValue(reply({})),
        fetch: vi.fn(),
      };
      await expect(
        journal.cleanup(
          api as unknown as APIRequestContext,
          'https://example.test',
          'visitor-test',
        ),
      ).rejects.toThrow('quiescence');
      expect(api.fetch).not.toHaveBeenCalled();
      expect(journal.resources.map((row) => row.id)).toContain('created-sandbox');
    },
  );
});
