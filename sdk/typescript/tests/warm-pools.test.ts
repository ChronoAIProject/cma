import { describe, expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/errors.js';
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
