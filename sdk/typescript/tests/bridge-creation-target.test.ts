import { describe, expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

const bridgeId = `brg_${'a'.repeat(64)}`;
const target = {
  bridge_id: bridgeId,
  owner: { kind: 'person', subject: 'owner' },
  callback_url: `https://public.example/api/v1/bridge-callbacks/${bridgeId}`,
};

describe('bridge creation callback preview', () => {
  it('reads a callback on the public origin with the caller’s retained creation key', async () => {
    const test = transport();
    test.answer(target);
    expect(await test.client.bridges.creationTarget({ idempotencyKey: 'same-creation' })).toEqual(
      target,
    );
    expect(test.calls[0]?.url).toBe('https://cma.example.test/api/v1/bridge-creation-target');
    expect(test.calls[0]?.init.method).toBe('GET');
    expect(test.calls[0]?.init.body).toBeUndefined();
    expect(new Headers(test.calls[0]?.init.headers).get('Idempotency-Key')).toBe('same-creation');
    test.answer({ ...target, owner: { kind: 'organization', subject: 'org-one' } });
    await test.client.bridges.creationTarget({
      organization: 'org-one',
      idempotencyKey: 'same-creation',
    });
    expect(new URL(test.calls[1]!.url).searchParams.get('organization')).toBe('org-one');
  });

  it('requires a bounded visible creation key before any request', () => {
    const test = transport();
    for (const idempotencyKey of [undefined, '', 'space key', 'x'.repeat(201)])
      expect(() =>
        test.client.bridges.creationTarget({ idempotencyKey: idempotencyKey as never }),
      ).toThrow(ValidationError);
    expect(test.calls).toHaveLength(0);
  });

  it('refuses missing fields, malformed callbacks and inconsistent identities', async () => {
    const test = transport();
    for (const value of [
      { ...target, bridge_id: undefined },
      { ...target, owner: undefined },
      { ...target, callback_url: undefined },
      { ...target, bridge_id: 'brg_wrong' },
      { ...target, owner: { kind: 'organization', subject: 'org-one' } },
      ...[
        target.callback_url.replace('https:', 'http:'),
        target.callback_url.replace('https://', 'https://user@'),
        `${target.callback_url}?value=one`,
        `${target.callback_url}#fragment`,
        `${target.callback_url}/extra`,
        `https://public.example/brg_${'b'.repeat(64)}`,
        'relative/path',
      ].map((callback_url) => ({ ...target, callback_url })),
    ]) {
      test.answer(value);
      await expect(
        test.client.bridges.creationTarget({ idempotencyKey: 'stable' }),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
    for (const owner of [target.owner, { kind: 'organization', subject: 'other' }]) {
      test.answer({ ...target, owner });
      await expect(
        test.client.bridges.creationTarget({ organization: 'org-one', idempotencyKey: 'stable' }),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
  });
});
