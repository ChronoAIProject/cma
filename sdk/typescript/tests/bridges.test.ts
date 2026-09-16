import { describe, expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import * as schema from '../src/bridges/schema.js';
import { loadCorpus } from './public-contract/context.js';
import { transport } from './resource-fixtures.js';
const corpus = loadCorpus();
const fixture = (name: string) => corpus.find((item) => item.id === `bridge-setup-${name}`)!;
const bridge = schema.bridge(fixture('read-success').response.body);
const create = schema.createRequest(fixture('create-success').request.body);
const detail = schema.detail(fixture('configuration-success').response.body);
const receipt = schema.receipt(fixture('control-success').response.body);
const command = schema.controlRequest(fixture('control-success').request.body);

describe('bridge management protocol boundaries', () => {
  it('retains background human authority only with explicit boolean consent', async () => {
    const test = transport();
    test.answer({ revision: 1, present: true, revoked: false });
    for (const consent of [undefined, false, true]) {
      await test.client.bridges.importKey(bridge.id, 1, 'selected-secret', {
        retainControlPlaneAuthority: consent,
      });
      expect(JSON.parse(String(test.calls.at(-1)!.init.body))).toEqual({
        expected_revision: 1,
        key: 'selected-secret',
        ...(consent === undefined ? {} : { retain_control_plane_authority: consent }),
      });
    }
    expect(() =>
      test.client.bridges.importKey(bridge.id, 1, 'selected-secret', {
        retainControlPlaneAuthority: 'true' as never,
      }),
    ).toThrow(ValidationError);
    expect(test.calls).toHaveLength(3);
  });
  it('keeps idempotent command input fixed and rejects a receipt for another command or actor', async () => {
    const test = transport();
    test.answer(receipt);
    await test.client.bridges.control(bridge.id, command, { idempotencyKey: 'stable' });
    await test.client.bridges.control(bridge.id, command, { idempotencyKey: 'stable' });
    expect(test.calls[0]!.init.body).toEqual(test.calls[1]!.init.body);
    expect(new Headers(test.calls[0]!.init.headers).get('idempotency-key')).toBe('stable');
    for (const wrong of [
      { ...receipt, bridge_id: 'other' },
      { ...receipt, revision: 8 },
      { ...receipt, command: 'enable' },
      { ...receipt, config_revision: 9 },
      { ...receipt, actor: { kind: 'trigger', subject: 'owner', launch_id: 'launch' } },
    ]) {
      test.answer(wrong);
      await expect(
        test.client.bridges.control(bridge.id, command, { idempotencyKey: 'stable' }),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
    test.answer(
      {
        ...schema.receipt(fixture('create-success').response.body),
        owner: { kind: 'person', subject: 'other' },
      },
      201,
    );
    await expect(
      test.client.bridges.create(create, { idempotencyKey: 'create' }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
  it('refuses foreign, duplicated, unordered and inconsistent inventory pages', async () => {
    const test = transport();
    for (const wrong of [
      { items: [bridge, bridge] },
      { items: [bridge], next_cursor: 'other' },
      { items: [], next_cursor: bridge.id },
      { items: [{ ...bridge, config_revision: 99 }] },
    ]) {
      test.answer(wrong);
      await expect(test.client.bridges.list()).rejects.toBeInstanceOf(ProtocolError);
    }
    test.answer({ items: [bridge] });
    await expect(test.client.bridges.list({ organization: 'foreign' })).rejects.toBeInstanceOf(
      ProtocolError,
    );
    await expect(test.client.bridges.list({ cursor: bridge.id })).rejects.toBeInstanceOf(
      ProtocolError,
    );
    test.answer({ items: [bridge, { ...bridge, id: `${bridge.id}z` }] });
    await expect(test.client.bridges.list({ limit: 1 })).rejects.toBeInstanceOf(ProtocolError);
    test.answer({ ...bridge, id: 'other' });
    await expect(test.client.bridges.read(bridge.id)).rejects.toBeInstanceOf(ProtocolError);
  });
  it('preserves readable historical configuration while enforcing fresh shared-context intent', async () => {
    const test = transport();
    for (const wrong of [
      { ...detail, bridge: { ...bridge, id: 'other' } },
      { ...detail, bridge: { ...bridge, can_manage: false } },
      {
        ...detail,
        config: { ...detail.config, binding: { ...detail.config.binding, bot_id: 'other' } },
      },
    ]) {
      test.answer(wrong);
      await expect(test.client.bridges.configuration(bridge.id)).rejects.toBeInstanceOf(
        ProtocolError,
      );
    }
    const historical = {
      ...detail.config,
      partition: 'conversation' as const,
      placement: {
        kind: 'existing_agent' as const,
        target: { sandbox_id: 'sb', workspace_id: 'ws', worktree_id: 'wt', agent_id: 'ag' },
      },
    };
    test.answer({ ...detail, config: historical });
    expect((await test.client.bridges.configuration(bridge.id)).config.partition).toBe(
      'conversation',
    );
    expect(() =>
      test.client.bridges.create({ ...create, config: historical }, { idempotencyKey: 'new' }),
    ).toThrow(ValidationError);
  });
  it('checks current validation identity and credential status without echoing imported material', async () => {
    const test = transport();
    const valid = schema.validation(fixture('validate-success').response.body);
    for (const wrong of [
      { ...valid, bridge_id: 'other' },
      { ...valid, revision: 2 },
      { ...valid, config_revision: 9 },
      { ...valid, valid: false },
    ]) {
      test.answer(wrong);
      await expect(test.client.bridges.validate(bridge.id, 1)).rejects.toBeInstanceOf(
        ProtocolError,
      );
    }
    for (const wrong of [
      { revision: 0, present: true, revoked: false },
      { revision: 0, present: false, revoked: true },
    ]) {
      test.answer(wrong);
      await expect(
        test.client.bridges.importKey(bridge.id, 1, 'selected-secret'),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
    test.answer({ revision: 1, present: true, revoked: false, key: 'server-accidental-secret' });
    expect(await test.client.bridges.importKey(bridge.id, 1, 'selected-secret')).toEqual({
      revision: 1,
      present: true,
      revoked: false,
    });
  });
  it('rejects invalid bounds and missing keys before issuing any transport request', () => {
    const test = transport();
    for (const limit of [0, 101, 1.1])
      expect(() => test.client.bridges.list({ limit })).toThrow(ValidationError);
    for (const key of ['', 'x'.repeat(201), undefined])
      expect(() =>
        test.client.bridges.control(bridge.id, command, { idempotencyKey: key as never }),
      ).toThrow(ValidationError);
    for (const key of ['', 'x'.repeat(4097), 1])
      expect(() => test.client.bridges.importKey(bridge.id, 1, key as never)).toThrow(
        ValidationError,
      );
    expect(() =>
      test.client.bridges.revokeKey(bridge.id, 0, 1, { idempotencyKey: 'revoke' }),
    ).toThrow(ValidationError);
    for (const routes of [
      [],
      ['duplicate', 'duplicate'],
      Array.from({ length: 65 }, (_, index) => String(index)),
    ]) {
      expect(() =>
        test.client.bridges.create(
          {
            ...create,
            config: { ...create.config, binding: { ...create.config.binding, route_ids: routes } },
          },
          { idempotencyKey: 'create' },
        ),
      ).toThrow(ValidationError);
    }
    expect(() =>
      test.client.bridges.create(
        {
          ...create,
          config: { ...create.config, limits: { ...create.config.limits, max_sandboxes: 99 } },
        },
        { idempotencyKey: 'create' },
      ),
    ).toThrow(ValidationError);
    expect(test.calls).toHaveLength(0);
  });
});
