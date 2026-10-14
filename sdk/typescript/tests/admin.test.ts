import { describe, expect, it } from 'vitest';
import { Administration } from '../src/admin/index.js';
import { ApiError, ProtocolError, ValidationError } from '../src/errors.js';
import { now, sandbox, transport } from './resource-fixtures.js';

const settings = {
  defaults: {
    sandbox_profile: { id: 'sbp_default', revision: 1 },
    workspace_profile: { id: 'wkp_default', revision: 2 },
    agent_profile: { id: 'agp_default', revision: 3 },
  },
  inactivity: { pause_after_seconds: 900 },
  retention: { agent_log_days: 7 },
  limits: { max_sandboxes_per_user: 5, visitor_max_sandboxes: 1, visitor_max_launches_per_day: 10 },
  triggers: { enabled: true, orchestration_paused: false },
  marketplace: { enabled: true },
};
const summary = {
  id: sandbox.id,
  owner: 'owner-1',
  name: 'Project',
  status: 'active',
  runtime_version: null,
  agent_count: 0,
  last_activity_at: now,
  retired_at: null,
};
const rawSandbox = {
  ...sandbox,
  owner: 'owner-1',
  revision: 1,
  profile: { ...sandbox.profile, digest: 'digest', content: { credential: 'not exposed' } },
  work: { credential: 'not exposed' },
  checkpoint: { credential: 'not exposed' },
};
const profile = {
  kind: 'agent',
  id: 'agp-1',
  name: 'Review',
  owner: 'owner-1',
  visibility: 'private',
  updated_at: now,
  retired_at: null,
};
const fixture = () => {
  const t = transport();
  return { ...t, admin: new Administration(t.client.config) };
};

describe('public operator administration', () => {
  it('retains verified creator and actor names separately from authority identifiers', async () => {
    const t = fixture();
    t.answer({ items: [{ ...summary, owner_name: 'Sam Lee' }], next: null });
    expect((await t.admin.listSandboxes()).items[0]).toMatchObject({
      owner: 'owner-1',
      owner_name: 'Sam Lee',
    });
    t.answer({ sandbox: rawSandbox, owner_name: 'Sam Lee', workspaces: [], agents: [] });
    expect((await t.admin.readSandbox(sandbox.id)).owner_name).toBe('Sam Lee');
    t.answer({
      items: [
        {
          id: 'activity-one',
          at: now,
          actor: 'operator-one',
          actor_name: 'Alex Chen',
          action: 'read',
          target: { kind: 'subject', id: 'owner-1' },
          target_name: 'Sam Lee',
          outcome: { status: 'succeeded' },
          detail: {},
        },
      ],
      next: null,
    });
    expect((await t.admin.listActivity()).items[0]).toMatchObject({
      actor: 'operator-one',
      actor_name: 'Alex Chen',
      target_name: 'Sam Lee',
    });
  });

  it('preserves retired snapshot fields from older reads without sending new controls', async () => {
    const t = fixture();
    const current = {
      ...settings,
      checkpoints: { interval_seconds: 1800, future: 'preserved' },
      retention: { ...settings.retention, checkpoint_grace_days: 7 },
    };
    t.answer({ revision: 1, settings: current });
    expect((await t.admin.readSettings()).settings).toMatchObject(current);
    await t.admin.patchSettings({ expected_revision: 1, marketplace: { enabled: false } });
    expect(JSON.parse(String(t.calls[1]!.init.body))).toEqual({
      expected_revision: 1,
      marketplace: { enabled: false },
    });
  });
  it('encodes Sandbox filters and accepts required nullable paging fields', async () => {
    const t = fixture();
    t.answer({ items: [summary], next: 'page/2' });
    expect(
      await t.admin.listSandboxes({
        owner: 'owner & one',
        status: 'active',
        runtime_version: 'v/2',
        cursor: 'page/1',
        limit: 200,
      }),
    ).toEqual({ items: [summary], next: 'page/2' });
    const call = t.calls[0]!;
    expect(new URL(call.url).pathname).toBe('/api/v1/admin/sandboxes');
    expect(Object.fromEntries(new URL(call.url).searchParams)).toEqual({
      owner: 'owner & one',
      status: 'active',
      runtime_version: 'v/2',
      cursor: 'page/1',
      limit: '200',
    });
    expect(call.init.method).toBe('GET');
    t.answer({ items: [], next: null });
    expect(await t.admin.listSandboxes()).toEqual({ items: [], next: null });
    t.answer({ items: [] });
    await expect(t.admin.listSandboxes()).rejects.toBeInstanceOf(ProtocolError);
  });

  it('projects only safe Sandbox detail and guards response identity', async () => {
    const t = fixture();
    t.answer({ sandbox: rawSandbox, workspaces: [], agents: [] });
    const detail = await t.admin.readSandbox(sandbox.id);
    expect(detail.sandbox.profile).toEqual({ ...sandbox.profile, digest: 'digest' });
    expect(detail.sandbox).not.toHaveProperty('work');
    expect(detail.sandbox).not.toHaveProperty('checkpoint');
    await expect(t.admin.readSandbox('different')).rejects.toThrow('identity');
  });

  it('reads typed events and rejects an event from another Sandbox', async () => {
    const t = fixture();
    const event = {
      sandbox_id: sandbox.id,
      seq: 2,
      appended_at: now,
      kind: { type: 'status_changed', from: 'active', to: 'pausing' },
    };
    t.answer([event]);
    expect(await t.admin.sandboxEvents(sandbox.id, { after: 1, limit: 50 })).toEqual([event]);
    expect(t.calls[0]!.url).toContain('/events?after=1&limit=50');
    await expect(t.admin.sandboxEvents('different')).rejects.toThrow('identity');
    await expect(t.admin.sandboxEvents(sandbox.id, { after: -1 })).rejects.toBeInstanceOf(
      ValidationError,
    );
  });

  it('retires a Sandbox without a body, invented key, or alternate success status', async () => {
    const t = fixture();
    t.answer(rawSandbox);
    await t.admin.retireSandbox(sandbox.id, { requestId: 'operator-830' });
    const call = t.calls[0]!;
    expect(call.url).toContain(`/admin/sandboxes/${sandbox.id}/retire`);
    expect(call.init.method).toBe('POST');
    expect(call.init.body).toBeUndefined();
    expect(new Headers(call.init.headers).has('Idempotency-Key')).toBe(false);
    expect(new Headers(call.init.headers).get('X-Request-Id')).toBe('operator-830');
    t.answer(rawSandbox, 201);
    await expect(t.admin.retireSandbox(sandbox.id)).rejects.toBeInstanceOf(ProtocolError);
    expect(t.calls).toHaveLength(2);
  });

  it('lists and retires profiles with exact kind and identity', async () => {
    const t = fixture();
    t.answer({ items: [profile], next: null });
    await t.admin.listProfiles({
      kind: 'agent',
      visibility: 'private',
      owner: 'owner-1',
      limit: 50,
    });
    expect(Object.fromEntries(new URL(t.calls[0]!.url).searchParams)).toEqual({
      kind: 'agent',
      visibility: 'private',
      owner: 'owner-1',
      limit: '50',
    });
    t.answer(profile);
    await t.admin.retireProfile('agent', 'agp-1');
    expect(t.calls[1]!.url).toContain('/admin/profiles/agent/agp-1/retire');
    expect(t.calls[1]!.init.method).toBe('POST');
    expect(t.calls[1]!.init.body).toBeUndefined();
    await expect(t.admin.retireProfile('sandbox', 'agp-1')).rejects.toThrow('kind');
    await expect(t.admin.retireProfile('agent', 'different')).rejects.toThrow('identity');
  });

  it('reads revision zero and retains newer root, section, and reference fields through CAS', async () => {
    const t = fixture();
    const newer = {
      ...settings,
      future: { preserved: true },
      inactivity: { ...settings.inactivity, warning_seconds: 120 },
      defaults: {
        ...settings.defaults,
        agent_profile: { ...settings.defaults.agent_profile, future: 'reference metadata' },
      },
    };
    t.answer({ revision: 0, settings: newer });
    const stored = await t.admin.readSettings();
    expect(stored).toEqual({ revision: 0, settings: newer });
    t.answer({ revision: 1, settings: newer });
    await t.admin.replaceSettings({
      expected_revision: stored.revision,
      settings: stored.settings,
    });
    expect(t.calls[1]!.init.method).toBe('PUT');
    expect(JSON.parse(String(t.calls[1]!.init.body))).toEqual({
      expected_revision: 0,
      settings: newer,
    });
    expect(new Headers(t.calls[1]!.init.headers).has('Idempotency-Key')).toBe(false);
  });

  it('lists allowlisted activity with nullable actor and target', async () => {
    const t = fixture();
    const activity = {
      id: 'act-1',
      at: now,
      actor: null,
      target: null,
      action: 'configure',
      outcome: { status: 'refused', code: 'not_an_operator' },
      detail: { capability: 'configure_platform' },
    };
    t.answer({ items: [activity], next: null });
    expect(
      (
        await t.admin.listActivity({
          actor: 'owner-1',
          action: 'configure',
          cursor: 'next',
          limit: 50,
        })
      ).items,
    ).toEqual([activity]);
    expect(Object.fromEntries(new URL(t.calls[0]!.url).searchParams)).toEqual({
      actor: 'owner-1',
      action: 'configure',
      cursor: 'next',
      limit: '50',
    });
  });

  it('patches full sections, resets with CAS, and controls Trigger orchestration once', async () => {
    const t = fixture();
    t.answer({ revision: 2, settings });
    await t.admin.patchSettings({ expected_revision: 1, triggers: settings.triggers });
    await t.admin.resetSettings({ expected_revision: 2 });
    await t.admin.pauseTriggers();
    await t.admin.resumeTriggers();
    expect(
      t.calls.map((call) => [new URL(call.url).pathname, call.init.method, call.init.body]),
    ).toEqual([
      [
        '/api/v1/admin/settings',
        'PATCH',
        JSON.stringify({ expected_revision: 1, triggers: settings.triggers }),
      ],
      ['/api/v1/admin/settings/reset', 'POST', JSON.stringify({ expected_revision: 2 })],
      ['/api/v1/admin/triggers/pause', 'POST', undefined],
      ['/api/v1/admin/triggers/resume', 'POST', undefined],
    ]);
    for (const call of t.calls)
      expect(new Headers(call.init.headers).has('Idempotency-Key')).toBe(false);
  });

  it('refuses empty, null, partial, unknown, or invalid-revision settings patches locally', () => {
    const t = fixture();
    for (const body of [
      { expected_revision: 1 },
      { expected_revision: 1, triggers: null },
      { expected_revision: 1, triggers: { enabled: true } },
      { expected_revision: 1, invented: {} },
      { expected_revision: -1, triggers: settings.triggers },
    ])
      expect(() => t.admin.patchSettings(body as never)).toThrow(ValidationError);
    expect(() => t.admin.resetSettings({ expected_revision: -1 })).toThrow(ValidationError);
    expect(t.fetch).not.toHaveBeenCalled();
  });

  it('reads typed runtime inventory and rejects unknown lifecycle states', async () => {
    const t = fixture();
    const inventory = {
      observed_at: now,
      runtimes: [
        { runtime_id: 'runtime-1', generation: 1, owner_reference: null, status: 'ready' },
      ],
    };
    t.answer(inventory);
    expect(await t.admin.readRuntimeInventory()).toEqual(inventory);
    expect(t.calls[0]!.url).toContain('/api/v1/admin/runtime-inventory');
    t.answer({ ...inventory, runtimes: [{ ...inventory.runtimes[0], status: 'invented' }] });
    await expect(t.admin.readRuntimeInventory()).rejects.toBeInstanceOf(ProtocolError);
  });

  it.each([0, 201, -1, 1.5, Number.NaN])(
    'refuses invalid page limit %s without a request',
    (limit) => {
      const t = fixture();
      expect(() => t.admin.listSandboxes({ limit })).toThrow(ValidationError);
      expect(() => t.admin.listProfiles({ limit })).toThrow(ValidationError);
      expect(() => t.admin.listActivity({ limit })).toThrow(ValidationError);
      expect(t.fetch).not.toHaveBeenCalled();
    },
  );

  it('validates cursor, path, settings bounds, and exact reference revisions before sending', () => {
    const t = fixture();
    expect(() => t.admin.listProfiles({ cursor: '' })).toThrow(ValidationError);
    expect(() => t.admin.readSandbox('..')).toThrow(ValidationError);
    for (const pause_after_seconds of [0, 59, 86401])
      expect(() =>
        t.admin.replaceSettings({
          expected_revision: 1,
          settings: { ...settings, inactivity: { pause_after_seconds } },
        }),
      ).toThrow(ValidationError);
    expect(() => t.admin.replaceSettings({ expected_revision: -1, settings })).toThrow(
      ValidationError,
    );
    expect(() =>
      t.admin.replaceSettings({
        expected_revision: 1,
        settings: {
          ...settings,
          retention: { ...settings.retention, agent_log_days: 4_294_967_296 },
        },
      }),
    ).toThrow(ValidationError);
    expect(() =>
      t.admin.replaceSettings({
        expected_revision: 1,
        settings: {
          ...settings,
          defaults: { ...settings.defaults, agent_profile: { id: 'agp-1', revision: 0 } },
        },
      }),
    ).toThrow(ValidationError);
    expect(t.fetch).not.toHaveBeenCalled();
  });

  it.each([
    [403, 'not_an_operator'],
    [404, 'profile_not_found'],
    [409, 'settings_conflict'],
    [503, 'settings_unavailable'],
  ] as const)('preserves typed %s refusal without retrying', async (status, code) => {
    const t = fixture();
    t.answer(
      {
        title: 'Refused',
        detail: 'A typed refusal',
        status,
        code,
        field: 'inactivity.pause_after_seconds',
      },
      status,
    );
    try {
      await t.admin.replaceSettings({ expected_revision: 1, settings });
      throw new Error('expected refusal');
    } catch (error) {
      expect(error).toBeInstanceOf(ApiError);
      expect(error).toMatchObject({ status, code, field: 'inactivity.pause_after_seconds' });
    }
    expect(t.calls).toHaveLength(1);
  });
});
