import { describe, expect, it, vi } from 'vitest';
import { Cma } from '../src/client.js';
import { resolveConfig } from '../src/config.js';
import { ProtocolError, ValidationError } from '../src/errors.js';
import {
  listContributorSponsorshipGrantsPage,
  listSponsorSponsorshipGrantsPage,
  listSponsorshipCredentialsPage,
  listSponsorshipPoliciesPage,
  toSponsoredGrant,
  toSponsorshipCredential,
  toSponsorshipPolicy,
} from '../src/sponsorships.js';

/** @vitest-environment node */

const QUOTA = { requests: 10, input_tokens: 1_000, output_tokens: 500 } as const;
const SUMMARY = { quota: QUOTA, spend_micros: 25_000 } as const;
const CREDENTIAL = {
  id: '11111111-1111-4111-8111-111111111111',
  provider: 'provider',
  provider_policy_key: 'provider-policy-v1',
  state: 'active',
  generation: 3,
  ownership_verified_at: '2026-08-29T00:00:00Z',
  updated_at: '2026-08-29T00:01:00Z',
} as const;
const POLICY = {
  id: '22222222-2222-4222-8222-222222222222',
  tenant_id: 'tenant-a',
  sponsor_display_name: 'Example sponsor',
  provider: 'provider',
  provider_policy_key: 'provider-policy-v1',
  allowed_models: ['model-a'],
  allowed_resources: ['responses'],
  per_launch_quota: QUOTA,
  aggregate_quota: { requests: 100, input_tokens: 10_000, output_tokens: 5_000 },
  per_launch_budget_micros: 50_000,
  aggregate_budget_micros: 500_000,
  max_requests_per_minute: 20,
  max_concurrent_launches: 4,
  expires_at: '2026-08-30T00:00:00Z',
  state: 'active',
  generation: 2,
  usage: {
    consumed: SUMMARY,
    reserved: { quota: QUOTA, spend_micros: 10_000 },
    active_launches: 1,
  },
  updated_at: '2026-08-29T00:02:00Z',
} as const;
const GRANT = {
  id: '33333333-3333-4333-8333-333333333333',
  policy_id: POLICY.id,
  launch_id: '44444444-4444-4444-8444-444444444444',
  session_id: 'session-a',
  contributor: 'subject-contributor',
  state: 'active',
  allocation: SUMMARY,
  usage: { quota: { requests: 1, input_tokens: 100, output_tokens: 50 }, spend_micros: 2_500 },
  capability: {
    grant_id: '33333333-3333-4333-8333-333333333333',
    grant_generation: 1,
    policy_generation: 2,
    launch_id: '44444444-4444-4444-8444-444444444444',
    session_id: 'session-a',
    expires_at: '2026-08-29T01:00:00Z',
    fingerprint: 'a'.repeat(64),
  },
  expires_at: '2026-08-29T01:00:00Z',
  updated_at: '2026-08-29T00:03:00Z',
  provider_revocation_pending: false,
} as const;

function json(body: unknown, link?: string): Response {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  if (link !== undefined) {
    headers.link = link;
  }
  return new Response(JSON.stringify(body), { status: 200, headers });
}

describe('sponsorship projection decoding', () => {
  it('decodes every public credential, policy, accounting, contributor, and grant field', () => {
    expect(toSponsorshipCredential(CREDENTIAL)).toEqual({
      id: CREDENTIAL.id,
      provider: 'provider',
      providerPolicyKey: 'provider-policy-v1',
      state: 'active',
      generation: 3,
      ownershipVerifiedAt: '2026-08-29T00:00:00Z',
      updatedAt: '2026-08-29T00:01:00Z',
    });
    expect(toSponsorshipPolicy(POLICY)).toMatchObject({
      id: POLICY.id,
      tenantId: 'tenant-a',
      sponsorDisplayName: 'Example sponsor',
      state: 'active',
      usage: { activeLaunches: 1 },
    });
    expect(toSponsoredGrant(GRANT)).toMatchObject({
      id: GRANT.id,
      contributor: 'subject-contributor',
      state: 'active',
      providerRevocationPending: false,
      capability: { grantId: GRANT.id, policyGeneration: 2 },
    });
  });

  it('rejects malformed known fields and every unknown root or nested field', () => {
    expect(() => toSponsorshipCredential({ ...CREDENTIAL, generation: 0 })).toThrow(ProtocolError);
    expect(() =>
      toSponsorshipPolicy({ ...POLICY, usage: { ...POLICY.usage, active_launches: -1 } }),
    ).toThrow(ProtocolError);
    expect(() => toSponsoredGrant({ ...GRANT, contributor: null })).toThrow(ProtocolError);
    expect(() =>
      toSponsoredGrant({ ...GRANT, provider_grant_id: 'provider-secret-do-not-project' }),
    ).toThrow(/unexpected field/);
    expect(() =>
      toSponsorshipCredential({ ...CREDENTIAL, owner: { subject: 'private-owner' } }),
    ).toThrow(/unexpected field/);

    const privateShapes: ReadonlyArray<readonly [(raw: unknown) => unknown, unknown]> = [
      [toSponsorshipCredential, { ...CREDENTIAL, request_aliases: ['sp1.old.digest'] }],
      [
        toSponsorshipCredential,
        { ...CREDENTIAL, provider_credential_id_digest: 'private-provider-authority' },
      ],
      [toSponsorshipPolicy, { ...POLICY, pending_operation: { kind: 'rotate' } }],
      [
        toSponsorshipPolicy,
        {
          ...POLICY,
          usage: { ...POLICY.usage, operation_client_request_id_digest: 'private-operation' },
        },
      ],
      [
        toSponsorshipPolicy,
        {
          ...POLICY,
          usage: {
            ...POLICY.usage,
            consumed: { ...POLICY.usage.consumed, lease: { holder: 'private-holder' } },
          },
        },
      ],
      [
        toSponsorshipPolicy,
        {
          ...POLICY,
          per_launch_quota: {
            ...POLICY.per_launch_quota,
            client_request_id_digest: 'private-client-request',
          },
        },
      ],
      [toSponsoredGrant, { ...GRANT, provider_grant_id_digest: 'private-provider-authority' }],
      [
        toSponsoredGrant,
        {
          ...GRANT,
          capability: {
            ...GRANT.capability,
            stable_request_id_digest: 'private-stable-request',
          },
        },
      ],
      [toSponsoredGrant, { ...GRANT, future_public_field: true }],
    ];
    for (const [decode, value] of privateShapes) {
      expect(() => decode(value)).toThrow(/unexpected field/);
    }
  });
});

describe('sponsorship collection pages', () => {
  it('uses bare array pages and follows owner collection links', async () => {
    const older = { ...CREDENTIAL, id: '55555555-5555-4555-8555-555555555555' };
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        json(
          [CREDENTIAL],
          '</api/v1/sponsorships/credentials?limit=1&cursor=credential-next>; rel="next"',
        ),
      )
      .mockResolvedValueOnce(json([older]));
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    const first = await listSponsorshipCredentialsPage(config, { limit: 1 });
    expect(first.items.map(({ id }) => id)).toEqual([CREDENTIAL.id]);
    if (first.next === undefined) {
      throw new Error('credential page did not return its continuation');
    }
    const second = await listSponsorshipCredentialsPage(config, { next: first.next });
    expect(second.items.map(({ id }) => id)).toEqual([older.id]);
    expect(fetch.mock.calls.map(([url]) => String(url))).toEqual([
      '/api/v1/sponsorships/credentials?limit=1',
      '/api/v1/sponsorships/credentials?limit=1&cursor=credential-next',
    ]);
  });

  it('refuses malformed collection bodies', async () => {
    const fetch = vi.fn().mockResolvedValue(json({ items: [POLICY] }));
    await expect(
      listSponsorshipPoliciesPage(
        resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch }),
      ),
    ).rejects.toBeInstanceOf(ProtocolError);
  });

  it('keeps grant continuations bound to contributor versus sponsor role', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValue(
        json(
          [GRANT],
          '</api/v1/sponsorships/grants?role=contributor&limit=1&cursor=grant-next>; rel="next"',
        ),
      );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });
    const contributor = await listContributorSponsorshipGrantsPage(config, { limit: 1 });
    expect(fetch).toHaveBeenCalledTimes(1);
    if (contributor.next === undefined) {
      throw new Error('contributor grant page did not return its continuation');
    }
    await expect(
      listSponsorSponsorshipGrantsPage(config, { next: contributor.next }),
    ).rejects.toBeInstanceOf(ValidationError);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('refuses a server continuation that changes the requested grant role', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValue(
        json(
          [GRANT],
          '</api/v1/sponsorships/grants?role=sponsor&limit=1&cursor=wrong-role>; rel="next"',
        ),
      );
    await expect(
      listContributorSponsorshipGrantsPage(
        resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch }),
        { limit: 1 },
      ),
    ).rejects.toBeInstanceOf(ProtocolError);
  });

  it('refuses unknown fields in every server-owned continuation shape', async () => {
    const credentialFetch = vi
      .fn()
      .mockResolvedValue(
        json(
          [CREDENTIAL],
          '</api/v1/sponsorships/credentials?limit=1&cursor=next&request_aliases=private>; rel="next"',
        ),
      );
    await expect(
      listSponsorshipCredentialsPage(
        resolveConfig({ fetch: credentialFetch as unknown as typeof globalThis.fetch }),
        { limit: 1 },
      ),
    ).rejects.toThrow(/continuation carried an unexpected field/);

    const policyFetch = vi
      .fn()
      .mockResolvedValue(
        json(
          [POLICY],
          '</api/v1/sponsorships/policies?limit=1&cursor=next&provider_authority_digest=private>; rel="next"',
        ),
      );
    await expect(
      listSponsorshipPoliciesPage(
        resolveConfig({ fetch: policyFetch as unknown as typeof globalThis.fetch }),
        { limit: 1 },
      ),
    ).rejects.toThrow(/continuation carried an unexpected field/);

    const grantFetch = vi
      .fn()
      .mockResolvedValue(
        json(
          [GRANT],
          '</api/v1/sponsorships/grants?role=contributor&limit=1&cursor=next&lease=private>; rel="next"',
        ),
      );
    await expect(
      listContributorSponsorshipGrantsPage(
        resolveConfig({ fetch: grantFetch as unknown as typeof globalThis.fetch }),
        { limit: 1 },
      ),
    ).rejects.toThrow(/continuation carried an unexpected field/);
  });

  it('exposes all four page operations on the configured Cma client', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(json([CREDENTIAL]))
      .mockResolvedValueOnce(json([POLICY]))
      .mockResolvedValueOnce(json([GRANT]))
      .mockResolvedValueOnce(json([GRANT]));
    const client = new Cma({ fetch: fetch as unknown as typeof globalThis.fetch });

    await client.listSponsorshipCredentialsPage();
    await client.listSponsorshipPoliciesPage();
    await client.listContributorSponsorshipGrantsPage();
    await client.listSponsorSponsorshipGrantsPage();

    expect(fetch.mock.calls.map(([url]) => String(url))).toEqual([
      '/api/v1/sponsorships/credentials',
      '/api/v1/sponsorships/policies',
      '/api/v1/sponsorships/grants?role=contributor',
      '/api/v1/sponsorships/grants?role=sponsor',
    ]);
  });
});
