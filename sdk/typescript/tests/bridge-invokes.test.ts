import { describe, expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import * as schema from '../src/bridges/invoke-schema.js';
import { loadCorpus } from './public-contract/context.js';
import { transport } from './resource-fixtures.js';

const corpus = loadCorpus();
const fixture = (name: string) => corpus.find((item) => item.id === `bridge-invoke-${name}`)!;
const issued = schema.receipt(fixture('issue-explicit').response.body);
const revoked = schema.receipt(fixture('revoke-explicit').response.body);
const ready = schema.readiness(fixture('readiness-present').response.body);
const issue = schema.issueRequest(fixture('issue-explicit').request.body);
const revoke = schema.revokeRequest(fixture('revoke-explicit').request.body);
const grant = issued.result;
const bridgeId = issued.bridge_id;
const command = { idempotencyKey: 'explicit-retained-consent' };

describe('explicit retained-context grant boundaries', () => {
  it('rejects invalid identity, lineage, attribution, lifecycle and forbidden projection fields', async () => {
    const test = transport();
    for (const wrong of [
      { ...grant, id: `brinv_${'a'.repeat(64)}` },
      { ...grant, id: `brinv_${'C'.repeat(64)}` },
      { ...grant, revision: 2 },
      { ...grant, owner: { ...grant.owner, subject: '' } },
      { ...grant, pin: { ...grant.pin, config_revision: 257 } },
      { ...grant, issued_by: { kind: 'api', subject: 'owner', delegation_id: 'delegated' } },
      { ...grant, issued_by: { kind: 'channel', bridge_id: bridgeId } },
      { ...grant, target: { ...grant.target, agent_creation_fingerprint: 'bad' } },
      { ...grant, target: { ...grant.target, worktree_identity_digest: 'E'.repeat(64) } },
      { ...grant, target: { ...grant.target, workspace_created_at: '\n' } },
      { ...grant, issued_at: '' },
      { ...grant, state: { ...grant.state, proof: 'forbidden' } },
      { ...grant, resource_audit: [] },
      { ...grant, config: {} },
      { ...grant, custody: {} },
    ]) {
      test.answer(wrong);
      await expect(test.client.bridges.readInvokeGrant(grant.id)).rejects.toBeInstanceOf(
        ProtocolError,
      );
    }
    test.answer(grant);
    expect(await test.client.bridges.readInvokeGrant(grant.id)).toEqual(grant);
  });
  it('requires a current active readiness candidate and treats absent consent explicitly', async () => {
    const test = transport();
    for (const wrong of [
      { ...ready, bridge_id: 'other' },
      { ...ready, revision: 0 },
      { ...ready, revision: Number.MAX_SAFE_INTEGER + 1 },
      { ...ready, config_revision: ready.revision + 1 },
      { ...ready, status: 'consent_required' },
      { ...ready, candidate: null },
      { ...ready, candidate: revoked.result },
      { ...ready, candidate: { ...grant, pin: { ...grant.pin, bridge_id: 'other' } } },
      { ...ready, candidate: { ...grant, pin: { ...grant.pin, config_revision: 2 } } },
    ]) {
      test.answer(wrong);
      await expect(test.client.bridges.invokeReadiness(bridgeId)).rejects.toBeInstanceOf(
        ProtocolError,
      );
    }
    for (const response of [
      ready,
      { ...ready, status: 'consent_required', candidate: null },
      { ...ready, status: 'not_applicable', candidate: null },
    ]) {
      test.answer(response);
      expect(await test.client.bridges.invokeReadiness(bridgeId)).toEqual(response);
    }
  });
  it('correlates issue receipt with exact target, bridge, consent and actual issue actor', async () => {
    const test = transport();
    for (const wrong of [
      { ...issued, id: 'a'.repeat(63) },
      { ...issued, request_digest: 'B'.repeat(64) },
      { ...issued, actor: { kind: 'human', subject: 'another-manager' } },
      { ...issued, bridge_id: 'other' },
      {
        ...issued,
        bridge_id: 'other',
        result: { ...grant, pin: { ...grant.pin, bridge_id: 'other' } },
      },
      {
        ...issued,
        result: {
          ...grant,
          target: { ...grant.target, ids: { ...grant.target.ids, agent_id: 'other' } },
        },
      },
      { ...issued, result: { ...grant, consent: 'implicit' } },
      { ...issued, result: { ...grant, pin: { ...grant.pin, config_revision: 2 } } },
      { ...issued, result: revoked.result },
    ]) {
      test.answer(wrong, 201);
      await expect(
        test.client.bridges.issueInvokeGrant(bridgeId, issue, command),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
    test.answer(issued, 200);
    await expect(
      test.client.bridges.issueInvokeGrant(bridgeId, issue, command),
    ).rejects.toBeInstanceOf(ProtocolError);
    test.answer(issued, 201);
    const mutable = structuredClone(issue);
    const pending = test.client.bridges.issueInvokeGrant(bridgeId, mutable, command);
    Object.assign(mutable.target, { agent_id: 'changed-after-call' });
    expect(await pending).toEqual(issued);
    expect(JSON.parse(String(test.calls.at(-1)!.init.body))).toEqual(issue);
  });
  it('requires exact terminal successor state while preserving original consent and lineage', async () => {
    const test = transport();
    for (const wrong of [
      { ...revoked, result: { ...revoked.result, id: `brinv_${'d'.repeat(64)}` } },
      { ...revoked, result: { ...revoked.result, revision: 3 } },
      { ...revoked, result: { ...revoked.result, pin: { ...grant.pin, config_revision: 2 } } },
      { ...revoked, result: grant },
      {
        ...revoked,
        result: { ...revoked.result, state: { kind: 'revoked', at: '', actor: revoked.actor } },
      },
      { ...revoked, actor: { kind: 'human', subject: 'other-manager' } },
    ]) {
      test.answer(wrong);
      await expect(
        test.client.bridges.revokeInvokeGrant(grant.id, revoke, command),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
    test.answer(revoked);
    await expect(
      test.client.bridges.revokeInvokeGrant(grant.id, { ...revoke, expected_revision: 2 }, command),
    ).rejects.toBeInstanceOf(ProtocolError);
    const revoker = { kind: 'human' as const, subject: 'current-manager' };
    const response = {
      ...revoked,
      actor: revoker,
      result: { ...revoked.result, state: { ...revoked.result.state, actor: revoker } },
    };
    test.answer(response);
    const result = await test.client.bridges.revokeInvokeGrant(grant.id, revoke, command);
    expect(result.result).toMatchObject({
      target: grant.target,
      issued_by: grant.issued_by,
      issued_at: grant.issued_at,
      pin: grant.pin,
      consent: grant.consent,
    });
    expect(result).toEqual(response);
  });
  it('checks ordered pages, original ownership, exact continuation and requested bounds', async () => {
    const test = transport();
    const next = { ...grant, id: `brinv_${'d'.repeat(64)}` };
    for (const items of [
      [grant, grant],
      [next, grant],
      [grant, { ...next, owner: { ...grant.owner, subject: 'other' } }],
      [{ ...grant, pin: { ...grant.pin, bridge_id: 'other' } }],
      Array.from({ length: 51 }, () => grant),
    ]) {
      test.answer({ items, next_cursor: null });
      await expect(test.client.bridges.listInvokeGrants(bridgeId)).rejects.toBeInstanceOf(
        ProtocolError,
      );
    }
    for (const wrong of [
      { items: [], next_cursor: grant.id },
      { items: [grant], next_cursor: next.id },
    ]) {
      test.answer(wrong);
      await expect(test.client.bridges.listInvokeGrants(bridgeId)).rejects.toBeInstanceOf(
        ProtocolError,
      );
    }
    test.answer({ items: [grant], next_cursor: null });
    await expect(
      test.client.bridges.listInvokeGrants(bridgeId, { cursor: grant.id }),
    ).rejects.toBeInstanceOf(ProtocolError);
    test.answer({ items: [grant, next], next_cursor: null });
    await expect(
      test.client.bridges.listInvokeGrants(bridgeId, { limit: 1 }),
    ).rejects.toBeInstanceOf(ProtocolError);
    test.answer({ items: [revoked.result], next_cursor: grant.id });
    expect((await test.client.bridges.listInvokeGrants(bridgeId)).next_cursor).toBe(grant.id);
    test.answer({ items: [], next_cursor: null });
    expect(
      (await test.client.bridges.listInvokeGrants(bridgeId, { cursor: grant.id, limit: 1 })).items,
    ).toHaveLength(0);
  });
  it('rejects malformed input, injected fields, invalid IDs and missing consent before HTTP', () => {
    const test = transport();
    for (const wrong of [
      { ...issue, expected_bridge_revision: 0 },
      { ...issue, expected_bridge_revision: Number.MAX_SAFE_INTEGER + 1 },
      { ...issue, shared_context: undefined },
      { ...issue, shared_context: 'implicit' },
      { ...issue, proof: {} },
      { ...issue, target: { ...issue.target, agent_id: 'invalid target' } },
    ])
      expect(() => test.client.bridges.issueInvokeGrant(bridgeId, wrong as never, command)).toThrow(
        ValidationError,
      );
    for (const idempotencyKey of ['', 'x'.repeat(201), undefined]) {
      expect(() =>
        test.client.bridges.issueInvokeGrant(bridgeId, issue, {
          idempotencyKey: idempotencyKey as never,
        }),
      ).toThrow(ValidationError);
      expect(() =>
        test.client.bridges.revokeInvokeGrant(grant.id, revoke, {
          idempotencyKey: idempotencyKey as never,
        }),
      ).toThrow(ValidationError);
    }
    for (const id of ['', 'brinv_bad', `brinv_${'C'.repeat(64)}`]) {
      expect(() => test.client.bridges.readInvokeGrant(id)).toThrow(ValidationError);
      expect(() => test.client.bridges.revokeInvokeGrant(id, revoke, command)).toThrow(
        ValidationError,
      );
      expect(() => test.client.bridges.listInvokeGrants(bridgeId, { cursor: id })).toThrow(
        ValidationError,
      );
    }
    for (const limit of [0, 101, 1.5])
      expect(() => test.client.bridges.listInvokeGrants(bridgeId, { limit })).toThrow(
        ValidationError,
      );
    expect(() => test.client.bridges.invokeReadiness('bad bridge')).toThrow(ValidationError);
    expect(test.calls).toHaveLength(0);
  });
});
