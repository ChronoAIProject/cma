import { describe, expect, it } from 'vitest';
import { ApiError, ProtocolError } from '../src/index.js';
import { command, now, sandbox, transport } from './resource-fixtures.js';

const update = {
  current_version: 'registry.example/runtime@sha256:old',
  latest_version: 'registry.example/runtime@sha256:new',
  update_available: true,
  state: 'available',
  can_update: true,
  schedule: { timezone: 'Asia/Singapore', local_time: '02:00' },
  preserves_home: true,
};
const detail = {
  ...sandbox,
  workspace_count: 0,
  worktree_count: 0,
  agent_count: 0,
  workspaces: [],
};

describe('Sandbox runtime updates', () => {
  it('retains exact image references and recorded states on owner reads', async () => {
    const test = transport();
    for (const state of ['up_to_date', 'available', 'queued', 'updating', 'failed']) {
      const projection = {
        ...update,
        state,
        target_version: update.latest_version,
        requested_at: now,
        started_at: now,
        completed_at: now,
        failure: { code: 'restore_failed', message: 'Restore needs attention.' },
        blocked_reason: 'Waiting for Agent work to finish.',
      };
      test.answer({ ...detail, runtime_update: projection });
      expect((await test.client.sandboxes.read(sandbox.id)).runtime_update).toEqual(projection);
    }
    test.answer({ ...detail, runtime_update: { ...update, current_version: null } });
    expect(
      (await test.client.sandboxes.read(sandbox.id)).runtime_update?.current_version,
    ).toBeNull();
    test.answer(detail);
    expect((await test.client.sandboxes.read(sandbox.id)).runtime_update).toBeUndefined();
  });

  it('submits the caller target and key exactly and accepts completed no-ops', async () => {
    const test = transport();
    const body = { target_version: update.latest_version };
    for (const status of [200, 202]) {
      test.answer({ ...sandbox, runtime_update: update }, status);
      const receipt = await test.client.sandboxes.updateRuntime(sandbox.id, body, command);
      expect(receipt.runtime_update).toEqual(update);
      const call = test.calls.at(-1)!;
      expect(call.url).toContain(`/sandboxes/${sandbox.id}/runtime-update`);
      expect(call.init.method).toBe('POST');
      expect(JSON.parse(String(call.init.body))).toEqual(body);
      expect(new Headers(call.init.headers).get('Idempotency-Key')).toBe(command.idempotencyKey);
    }
    test.answer({ code: 'runtime_target_changed', detail: 'The deployment runtime changed.' }, 409);
    await expect(
      test.client.sandboxes.updateRuntime(sandbox.id, body, command),
    ).rejects.toBeInstanceOf(ApiError);
  });

  it('rejects wrong identity, missing status and malformed progress without guessing readiness', async () => {
    const test = transport();
    for (const value of [
      sandbox,
      { ...sandbox, id: 'another', runtime_update: update },
      { ...sandbox, runtime_update: { ...update, state: 'done' } },
      { ...sandbox, runtime_update: { ...update, can_update: 'yes' } },
      { ...sandbox, runtime_update: { ...update, current_version: undefined } },
      {
        ...sandbox,
        runtime_update: { ...update, schedule: { timezone: 'UTC', local_time: '02:00' } },
      },
    ]) {
      test.answer(value, 202);
      await expect(
        test.client.sandboxes.updateRuntime(
          sandbox.id,
          { target_version: update.latest_version },
          command,
        ),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
    test.answer({ ...sandbox, runtime_update: update }, 201);
    await expect(
      test.client.sandboxes.updateRuntime(
        sandbox.id,
        { target_version: update.latest_version },
        command,
      ),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
});
