// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../errors.js';
import { transport } from './resource-fixtures.js';

const pool = {
  id: 'wp_original',
  revision: 1,
  config_sha256: 'c'.repeat(64),
  image: 'registry/runtime@sha256:original',
  cpu_millis: 1000,
  memory_mib: 1024,
  target_replicas: 3,
  ready_replicas: null,
  observed: false,
  status: 'pending',
  resource_name: null,
  error: null,
};

describe('native warm pool controls', () => {
  it('accepts older inventories without additive configuration digests', async () => {
    const t = transport();
    const oldPool = Object.fromEntries(
      Object.entries(pool).filter(([key]) => key !== 'config_sha256'),
    );
    for (const items of [[], [oldPool]]) {
      t.answer({
        items,
        default_image: 'next-image',
        default_cpu_millis: 1000,
        default_memory_mib: 1024,
        available: true,
      });
      const list = await t.client.admin.listWarmPools();
      expect(list.default_config_sha256).toBeUndefined();
      expect(list.machine_image).toBeUndefined();
      expect(list.items).toEqual(items);
      expect(list.items[0]?.config_sha256).toBeUndefined();
      expect(list.items[0]?.machine).toBeUndefined();
    }
  });
  it('names the shared desktop pool and its image, and rejects malformed desktop fields', async () => {
    const t = transport();
    const desktop = { ...pool, id: 'wp_desktop', machine: true, image: 'registry/desktop' };
    const inventory = {
      items: [{ ...pool, machine: false }, desktop],
      default_image: 'next-image',
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_config_sha256: 'd'.repeat(64),
      available: true,
      machine_image: 'registry/desktop',
    };
    t.answer(inventory);
    const list = await t.client.admin.listWarmPools();
    expect(list.machine_image).toBe('registry/desktop');
    expect(list.items.map((item) => item.machine)).toEqual([false, true]);
    t.answer({ ...inventory, machine_image: null });
    expect((await t.client.admin.listWarmPools()).machine_image).toBeNull();
    for (const value of [null, 'true', 1]) {
      t.answer({ ...inventory, items: [{ ...desktop, machine: value }] });
      await expect(t.client.admin.listWarmPools()).rejects.toBeInstanceOf(ProtocolError);
    }
    for (const value of [42, false, {}]) {
      t.answer({ ...inventory, machine_image: value });
      await expect(t.client.admin.listWarmPools()).rejects.toBeInstanceOf(ProtocolError);
    }
  });
  it('preserves unavailable default digests and rejects malformed present digest fields', async () => {
    const t = transport();
    const inventory = {
      items: [pool],
      default_image: 'next-image',
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_config_sha256: null,
      available: true,
    };
    t.answer(inventory);
    expect((await t.client.admin.listWarmPools()).default_config_sha256).toBeNull();
    for (const value of [null, 42, false, {}, []]) {
      t.answer({ ...inventory, items: [{ ...pool, config_sha256: value }] });
      await expect(t.client.admin.listWarmPools()).rejects.toBeInstanceOf(ProtocolError);
      if (value !== null) {
        t.answer({ ...inventory, default_config_sha256: value });
        await expect(t.client.admin.listWarmPools()).rejects.toBeInstanceOf(ProtocolError);
      }
    }
  });
  it('preserves a drained pool with a zero provider target and rejects malformed values', async () => {
    const t = transport();
    const drained = {
      ...pool,
      target_replicas: 0,
      provider_target_replicas: 0,
      ready_replicas: 0,
      observed: true,
      status: 'disabled',
      retirement_status: 'drained',
    };
    const inventory = {
      items: [drained],
      default_image: 'next-image',
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      available: true,
    };
    t.answer(inventory);
    expect((await t.client.admin.listWarmPools()).items).toEqual([drained]);
    for (const fields of [
      { provider_target_replicas: null },
      { provider_target_replicas: -1 },
      { retirement_status: null },
      { retirement_status: 'disabled' },
    ]) {
      t.answer({ ...inventory, items: [{ ...drained, ...fields }] });
      await expect(t.client.admin.listWarmPools()).rejects.toBeInstanceOf(ProtocolError);
    }
  });
  it('sends idempotent create and revision guarded disable commands', async () => {
    const t = transport();
    t.answer(pool, 202);
    await t.client.admin.configureWarmPool(
      { cpu_millis: 1000, memory_mib: 1024, target_replicas: 3 },
      { idempotencyKey: 'create-original' },
    );
    expect(t.calls[0]!.url).toBe('https://cma.example.test/api/v1/admin/warm-pools');
    expect(new Headers(t.calls[0]!.init.headers).get('Idempotency-Key')).toBe('create-original');
    expect(JSON.parse(t.calls[0]!.init.body as string)).toEqual({
      cpu_millis: 1000,
      memory_mib: 1024,
      target_replicas: 3,
    });
    await t.client.admin.resizeWarmPool(pool.id, { expected_revision: 1, target_replicas: 0 });
    expect(t.calls[1]!.init.method).toBe('PUT');
    expect(JSON.parse(t.calls[1]!.init.body as string)).toEqual({
      expected_revision: 1,
      target_replicas: 0,
    });
  });
  it('preserves unknown readiness and rejects mismatched resource receipts', async () => {
    const t = transport();
    t.answer({
      items: [{ ...pool, status: 'unavailable' }],
      default_image: 'next-image',
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_config_sha256: 'd'.repeat(64),
      available: false,
    });
    const list = await t.client.admin.listWarmPools();
    expect(list.available).toBe(false);
    expect(list.items[0]!.config_sha256).toBe('c'.repeat(64));
    expect(list.default_config_sha256).toBe('d'.repeat(64));
    expect(list.items[0]!.ready_replicas).toBeNull();
    expect(list.items[0]!.image).toBe(pool.image);
    t.answer({ ...pool, id: 'replacement' }, 202);
    await expect(
      t.client.admin.resizeWarmPool(pool.id, { expected_revision: 1, target_replicas: 0 }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
  it('refuses unsafe numeric targets and missing replay keys before HTTP', () => {
    const t = transport();
    expect(() =>
      t.client.admin.configureWarmPool(
        { cpu_millis: 1000, memory_mib: 1024, target_replicas: 1001 },
        { idempotencyKey: 'k' },
      ),
    ).toThrow(ValidationError);
    expect(() =>
      t.client.admin.configureWarmPool(
        { cpu_millis: 1000, memory_mib: 1024, target_replicas: 1 },
        { idempotencyKey: '' },
      ),
    ).toThrow(ValidationError);
    expect(() =>
      t.client.admin.resizeWarmPool(pool.id, { expected_revision: 0, target_replicas: 0 }),
    ).toThrow(ValidationError);
    expect(t.calls).toHaveLength(0);
  });
});
