// @vitest-environment node
import { expect, it } from 'vitest';
import { createClient, ProtocolError } from '../index.js';
import { agentShareLink, shareReceipt } from '../resource-access.js';

const operation = 'a'.repeat(64);
const actor = { kind: 'human', subject: 'owner' };
const at = '2026-01-01T00:00:00Z';
type Change = 'agent' | 'revoke' | 'set_mode';

function fixture(change: Change) {
  const kind = change === 'agent' ? 'agent' : 'sandbox';
  const revocation = {
    operation_id: operation,
    kind,
    id: 'resource',
    share_id: 'share',
    expected_revision: 1,
  };
  const share = {
    id: 'share',
    owner: { kind: 'person', subject: 'owner' },
    selector: { kind: 'sandbox', sandbox_id: 'resource' },
    target_org_id: 'org',
    mode: 'view',
    issuer_actor: actor,
    created_at: at,
    revision: 2,
    revoked_at: change === 'set_mode' ? null : at,
  };
  const link = {
    id: 'share',
    agent_id: 'resource',
    audience: { kind: 'public' },
    mode: 'view',
    shared_by: { subject: 'owner', display_name: 'Owner', avatar_url: null },
    created_at: at,
    revoked_at: at,
    revision: 2,
    accepted_count: 1,
  };
  const receipt = { operation_id: operation, actor, at, policy_revision: 2, share };
  const legacy = change === 'agent' ? link : receipt;
  async function send(body: unknown) {
    const client = createClient({
      baseUrl: 'https://fixture.invalid',
      fetch: async () => Response.json(body),
    });
    const options = { idempotencyKey: 'original' };
    return change === 'agent'
      ? client.resourceAccess.revokeAgentShareLink('resource', 'share', 1, options)
      : change === 'revoke'
        ? client.resourceAccess.revokeShare('sandbox', 'resource', 'share', 1, options)
        : client.resourceAccess.updateShare(
            'sandbox',
            'resource',
            'share',
            { expected_revision: 1, mode: 'view' },
            options,
          );
  }
  return { legacy, revocation, send };
}

for (const change of ['agent', 'revoke', 'set_mode'] as const) {
  it(`${change} parses successful recovery metadata while old readers preserve legacy fields`, async () => {
    const { legacy, revocation, send } = fixture(change);
    const body = { ...legacy, revocation };
    expect(await send(body)).toEqual({ ...legacy, revocation: { operation } });
    expect(await send(legacy)).toEqual(legacy);
    expect((change === 'agent' ? agentShareLink : shareReceipt)(body)).toEqual(legacy);
  });

  it.each([
    null,
    {},
    { operation_id: '../other' },
    { operation_id: 'a'.repeat(63) },
    { kind: 'other' },
    { id: 'other' },
    { share_id: 'other' },
    { expected_revision: 2 },
  ])(`${change} refuses malformed or mismatched recovery reference %j`, async (patch) => {
    const { legacy, revocation, send } = fixture(change);
    await expect(
      send({
        ...legacy,
        revocation:
          patch === null
            ? null
            : Object.keys(patch).length === 0
              ? {}
              : { ...revocation, ...patch },
      }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });

  it(`${change} refuses a success DTO inconsistent with its recovery reference`, async () => {
    const { legacy, revocation, send } = fixture(change);
    const body = { ...legacy, revocation };
    const patches =
      change === 'agent'
        ? [{ id: 'other' }, { agent_id: 'other' }, { revision: 3 }, { revoked_at: null }]
        : [
            { operation_id: 'b'.repeat(64) },
            { share: { ...('share' in legacy ? legacy.share : {}), revision: 3 } },
            {
              share: {
                ...('share' in legacy ? legacy.share : {}),
                revoked_at: change === 'revoke' ? null : at,
              },
            },
          ];
    for (const patch of patches)
      await expect(send({ ...body, ...patch })).rejects.toBeInstanceOf(ProtocolError);
  });
}
