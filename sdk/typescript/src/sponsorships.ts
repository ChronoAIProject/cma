/** Secret-free sponsorship collection projections. */

import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import {
  pageTarget,
  requestPage,
  type OperationOptions,
  type Page,
  type PageRequest,
} from './http.js';

const CREDENTIALS = '/api/v1/sponsorships/credentials';
const POLICIES = '/api/v1/sponsorships/policies';
const GRANTS = '/api/v1/sponsorships/grants';

const CREDENTIAL_STATES = ['active', 'rotation_pending', 'revocation_pending', 'revoked'] as const;
const POLICY_STATES = ['active', 'suspended', 'revoked'] as const;
const GRANT_STATES = [
  'active',
  'exhausted',
  'expired',
  'revocation_pending',
  'revoked',
  'closed',
  'suspended',
] as const;

const CREDENTIAL_FIELDS = new Set([
  'id',
  'provider',
  'provider_policy_key',
  'state',
  'generation',
  'ownership_verified_at',
  'updated_at',
]);
const POLICY_FIELDS = new Set([
  'id',
  'tenant_id',
  'sponsor_display_name',
  'provider',
  'provider_policy_key',
  'allowed_models',
  'allowed_resources',
  'per_launch_quota',
  'aggregate_quota',
  'per_launch_budget_micros',
  'aggregate_budget_micros',
  'max_requests_per_minute',
  'max_concurrent_launches',
  'expires_at',
  'state',
  'generation',
  'usage',
  'updated_at',
]);
const POLICY_USAGE_FIELDS = new Set(['consumed', 'reserved', 'active_launches']);
const USAGE_SUMMARY_FIELDS = new Set(['quota', 'spend_micros']);
const QUOTA_FIELDS = new Set(['requests', 'input_tokens', 'output_tokens']);
const GRANT_FIELDS = new Set([
  'id',
  'policy_id',
  'launch_id',
  'session_id',
  'contributor',
  'state',
  'allocation',
  'usage',
  'capability',
  'expires_at',
  'updated_at',
  'provider_revocation_pending',
]);
const CAPABILITY_FIELDS = new Set([
  'grant_id',
  'grant_generation',
  'policy_generation',
  'launch_id',
  'session_id',
  'expires_at',
  'fingerprint',
]);
const COLLECTION_CONTINUATION_FIELDS = new Set(['limit', 'cursor']);
const GRANT_CONTINUATION_FIELDS = new Set(['role', 'limit', 'cursor']);

export type SponsoredCredentialState = (typeof CREDENTIAL_STATES)[number];
export type SponsorshipPolicyState = (typeof POLICY_STATES)[number];
export type SponsoredGrantState = (typeof GRANT_STATES)[number];

export interface SponsoredQuota {
  readonly requests: number;
  readonly inputTokens: number;
  readonly outputTokens: number;
}

export interface SponsoredUsageSummary {
  readonly quota: SponsoredQuota;
  readonly spendMicros: number;
}

export interface SponsorshipPolicyUsage {
  readonly consumed: SponsoredUsageSummary;
  readonly reserved: SponsoredUsageSummary;
  readonly activeLaunches: number;
}

export interface SponsorshipCredential {
  readonly id: string;
  readonly provider: string;
  readonly providerPolicyKey: string;
  readonly state: SponsoredCredentialState;
  readonly generation: number;
  readonly ownershipVerifiedAt: string;
  readonly updatedAt: string;
}

export interface SponsorshipPolicy {
  readonly id: string;
  readonly tenantId: string;
  readonly sponsorDisplayName: string;
  readonly provider: string;
  readonly providerPolicyKey: string;
  readonly allowedModels: readonly string[];
  readonly allowedResources: readonly string[];
  readonly perLaunchQuota: SponsoredQuota;
  readonly aggregateQuota: SponsoredQuota;
  readonly perLaunchBudgetMicros: number;
  readonly aggregateBudgetMicros: number;
  readonly maxRequestsPerMinute: number;
  readonly maxConcurrentLaunches: number;
  readonly expiresAt: string;
  readonly state: SponsorshipPolicyState;
  readonly generation: number;
  readonly usage: SponsorshipPolicyUsage;
  readonly updatedAt: string;
}

export interface InertSponsoredGrantCapability {
  readonly grantId: string;
  readonly grantGeneration: number;
  readonly policyGeneration: number;
  readonly launchId: string;
  readonly sessionId: string;
  readonly expiresAt: string;
  readonly fingerprint: string;
}

export interface SponsoredGrant {
  readonly id: string;
  readonly policyId: string;
  readonly launchId: string;
  readonly sessionId: string;
  readonly contributor: string;
  readonly state: SponsoredGrantState;
  readonly allocation: SponsoredUsageSummary;
  readonly usage: SponsoredUsageSummary;
  readonly capability: InertSponsoredGrantCapability;
  readonly expiresAt: string;
  readonly updatedAt: string;
  readonly providerRevocationPending: boolean;
}

export type SponsorshipPageRequest = PageRequest;

/** Read one bounded page of the authenticated owner's credential metadata. */
export async function listSponsorshipCredentialsPage(
  config: ResolvedConfig,
  page: SponsorshipPageRequest = {},
  options: OperationOptions = {},
): Promise<Page<SponsorshipCredential>> {
  const target = pageTarget(CREDENTIALS, page);
  requireContinuationFields(
    target,
    COLLECTION_CONTINUATION_FIELDS,
    'sponsorship credential',
    ValidationError,
  );
  const result = await requestPage(
    config,
    CREDENTIALS,
    target,
    (body) => decodeList(body, 'sponsorship credential page', toSponsorshipCredential),
    options,
  );
  if (result.next !== undefined) {
    requireContinuationFields(
      result.next,
      COLLECTION_CONTINUATION_FIELDS,
      'sponsorship credential',
      ProtocolError,
    );
  }
  return result;
}

/** Read one bounded page of the authenticated owner's policies. */
export async function listSponsorshipPoliciesPage(
  config: ResolvedConfig,
  page: SponsorshipPageRequest = {},
  options: OperationOptions = {},
): Promise<Page<SponsorshipPolicy>> {
  const target = pageTarget(POLICIES, page);
  requireContinuationFields(
    target,
    COLLECTION_CONTINUATION_FIELDS,
    'sponsorship policy',
    ValidationError,
  );
  const result = await requestPage(
    config,
    POLICIES,
    target,
    (body) => decodeList(body, 'sponsorship policy page', toSponsorshipPolicy),
    options,
  );
  if (result.next !== undefined) {
    requireContinuationFields(
      result.next,
      COLLECTION_CONTINUATION_FIELDS,
      'sponsorship policy',
      ProtocolError,
    );
  }
  return result;
}

/** Read one bounded page of grants attributed to the authenticated contributor. */
export async function listContributorSponsorshipGrantsPage(
  config: ResolvedConfig,
  page: SponsorshipPageRequest = {},
  options: OperationOptions = {},
): Promise<Page<SponsoredGrant>> {
  return grantPage(config, 'contributor', page, options);
}

/** Read one bounded page of grants controlled by the authenticated sponsor. */
export async function listSponsorSponsorshipGrantsPage(
  config: ResolvedConfig,
  page: SponsorshipPageRequest = {},
  options: OperationOptions = {},
): Promise<Page<SponsoredGrant>> {
  return grantPage(config, 'sponsor', page, options);
}

/** Validate one secret-free credential projection. */
export function toSponsorshipCredential(raw: unknown): SponsorshipCredential {
  const body = exactObjectOf(raw, CREDENTIAL_FIELDS, 'sponsorship credential');
  return {
    id: nonEmptyString(body.id, 'sponsorship credential id'),
    provider: nonEmptyString(body.provider, 'sponsorship credential provider'),
    providerPolicyKey: nonEmptyString(
      body.provider_policy_key,
      'sponsorship credential provider_policy_key',
    ),
    state: memberOf(body.state, CREDENTIAL_STATES, 'sponsorship credential state'),
    generation: positiveInteger(body.generation, 'sponsorship credential generation'),
    ownershipVerifiedAt: nonEmptyString(
      body.ownership_verified_at,
      'sponsorship credential ownership_verified_at',
    ),
    updatedAt: nonEmptyString(body.updated_at, 'sponsorship credential updated_at'),
  };
}

/** Validate one owner-safe policy and its durable accounting. */
export function toSponsorshipPolicy(raw: unknown): SponsorshipPolicy {
  const body = exactObjectOf(raw, POLICY_FIELDS, 'sponsorship policy');
  return {
    id: nonEmptyString(body.id, 'sponsorship policy id'),
    tenantId: nonEmptyString(body.tenant_id, 'sponsorship policy tenant_id'),
    sponsorDisplayName: requiredString(
      body.sponsor_display_name,
      'sponsorship policy sponsor_display_name',
    ),
    provider: nonEmptyString(body.provider, 'sponsorship policy provider'),
    providerPolicyKey: nonEmptyString(
      body.provider_policy_key,
      'sponsorship policy provider_policy_key',
    ),
    allowedModels: stringArray(body.allowed_models, 'sponsorship policy allowed_models'),
    allowedResources: stringArray(body.allowed_resources, 'sponsorship policy allowed_resources'),
    perLaunchQuota: quotaOf(body.per_launch_quota, 'sponsorship policy per_launch_quota'),
    aggregateQuota: quotaOf(body.aggregate_quota, 'sponsorship policy aggregate_quota'),
    perLaunchBudgetMicros: unsignedInteger(
      body.per_launch_budget_micros,
      'sponsorship policy per_launch_budget_micros',
    ),
    aggregateBudgetMicros: unsignedInteger(
      body.aggregate_budget_micros,
      'sponsorship policy aggregate_budget_micros',
    ),
    maxRequestsPerMinute: unsignedInteger(
      body.max_requests_per_minute,
      'sponsorship policy max_requests_per_minute',
    ),
    maxConcurrentLaunches: unsignedInteger(
      body.max_concurrent_launches,
      'sponsorship policy max_concurrent_launches',
    ),
    expiresAt: nonEmptyString(body.expires_at, 'sponsorship policy expires_at'),
    state: memberOf(body.state, POLICY_STATES, 'sponsorship policy state'),
    generation: positiveInteger(body.generation, 'sponsorship policy generation'),
    usage: policyUsageOf(body.usage),
    updatedAt: nonEmptyString(body.updated_at, 'sponsorship policy updated_at'),
  };
}

/** Validate one contributor/sponsor-safe grant projection. */
export function toSponsoredGrant(raw: unknown): SponsoredGrant {
  const body = exactObjectOf(raw, GRANT_FIELDS, 'sponsored grant');
  const capability = exactObjectOf(
    body.capability,
    CAPABILITY_FIELDS,
    'sponsored grant capability',
  );
  return {
    id: nonEmptyString(body.id, 'sponsored grant id'),
    policyId: nonEmptyString(body.policy_id, 'sponsored grant policy_id'),
    launchId: nonEmptyString(body.launch_id, 'sponsored grant launch_id'),
    sessionId: nonEmptyString(body.session_id, 'sponsored grant session_id'),
    contributor: nonEmptyString(body.contributor, 'sponsored grant contributor'),
    state: memberOf(body.state, GRANT_STATES, 'sponsored grant state'),
    allocation: summaryOf(body.allocation, 'sponsored grant allocation'),
    usage: summaryOf(body.usage, 'sponsored grant usage'),
    capability: {
      grantId: nonEmptyString(capability.grant_id, 'sponsored grant capability grant_id'),
      grantGeneration: positiveInteger(
        capability.grant_generation,
        'sponsored grant capability grant_generation',
      ),
      policyGeneration: positiveInteger(
        capability.policy_generation,
        'sponsored grant capability policy_generation',
      ),
      launchId: nonEmptyString(capability.launch_id, 'sponsored grant capability launch_id'),
      sessionId: nonEmptyString(capability.session_id, 'sponsored grant capability session_id'),
      expiresAt: nonEmptyString(capability.expires_at, 'sponsored grant capability expires_at'),
      fingerprint: nonEmptyString(capability.fingerprint, 'sponsored grant capability fingerprint'),
    },
    expiresAt: nonEmptyString(body.expires_at, 'sponsored grant expires_at'),
    updatedAt: nonEmptyString(body.updated_at, 'sponsored grant updated_at'),
    providerRevocationPending: requiredBoolean(
      body.provider_revocation_pending,
      'sponsored grant provider_revocation_pending',
    ),
  };
}

async function grantPage(
  config: ResolvedConfig,
  role: 'contributor' | 'sponsor',
  page: SponsorshipPageRequest,
  options: OperationOptions,
): Promise<Page<SponsoredGrant>> {
  const target =
    page.next === undefined ? pageTarget(GRANTS, page, { role }) : pageTarget(GRANTS, page);
  requireContinuationFields(
    target,
    GRANT_CONTINUATION_FIELDS,
    `${role} sponsored grant`,
    ValidationError,
  );
  requireGrantRole(target, role, ValidationError);
  const result = await requestPage(
    config,
    GRANTS,
    target,
    (body) => decodeList(body, `${role} sponsored grant page`, toSponsoredGrant),
    options,
  );
  if (result.next !== undefined) {
    requireContinuationFields(
      result.next,
      GRANT_CONTINUATION_FIELDS,
      `${role} sponsored grant`,
      ProtocolError,
    );
    requireGrantRole(result.next, role, ProtocolError);
  }
  return result;
}

function requireGrantRole(
  target: string,
  expected: 'contributor' | 'sponsor',
  ErrorType: typeof ProtocolError | typeof ValidationError,
): void {
  const parsed = new URL(target, 'https://cma.invalid');
  const roles = parsed.searchParams.getAll('role');
  if (roles.length !== 1 || roles[0] !== expected) {
    throw new ErrorType(`sponsorship grant continuation is not bound to the ${expected} role`);
  }
}

function requireContinuationFields(
  target: string,
  allowed: ReadonlySet<string>,
  name: string,
  ErrorType: typeof ProtocolError | typeof ValidationError,
): void {
  const parsed = new URL(target, 'https://cma.invalid');
  for (const field of new Set(parsed.searchParams.keys())) {
    if (!allowed.has(field)) {
      throw new ErrorType(`${name} continuation carried an unexpected field`);
    }
    if (parsed.searchParams.getAll(field).length !== 1) {
      throw new ErrorType(`${name} continuation repeated a field`);
    }
  }
}

function decodeList<T>(raw: unknown, name: string, decode: (entry: unknown) => T): readonly T[] {
  if (!Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not a list`);
  }
  return raw.map(decode);
}

function policyUsageOf(raw: unknown): SponsorshipPolicyUsage {
  const body = exactObjectOf(raw, POLICY_USAGE_FIELDS, 'sponsorship policy usage');
  return {
    consumed: summaryOf(body.consumed, 'sponsorship policy consumed usage'),
    reserved: summaryOf(body.reserved, 'sponsorship policy reserved usage'),
    activeLaunches: unsignedInteger(body.active_launches, 'sponsorship policy active_launches'),
  };
}

function summaryOf(raw: unknown, name: string): SponsoredUsageSummary {
  const body = exactObjectOf(raw, USAGE_SUMMARY_FIELDS, name);
  return {
    quota: quotaOf(body.quota, `${name} quota`),
    spendMicros: unsignedInteger(body.spend_micros, `${name} spend_micros`),
  };
}

function quotaOf(raw: unknown, name: string): SponsoredQuota {
  const body = exactObjectOf(raw, QUOTA_FIELDS, name);
  return {
    requests: unsignedInteger(body.requests, `${name} requests`),
    inputTokens: unsignedInteger(body.input_tokens, `${name} input_tokens`),
    outputTokens: unsignedInteger(body.output_tokens, `${name} output_tokens`),
  };
}

function objectOf(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not an object`);
  }
  return raw as Record<string, unknown>;
}

function exactObjectOf(
  raw: unknown,
  allowed: ReadonlySet<string>,
  name: string,
): Record<string, unknown> {
  const body = objectOf(raw, name);
  const unexpected = Object.keys(body).find((field) => !allowed.has(field));
  if (unexpected !== undefined) {
    throw new ProtocolError(`${name} carried an unexpected field`);
  }
  return body;
}

function requiredString(raw: unknown, name: string): string {
  if (typeof raw !== 'string') {
    throw new ProtocolError(`${name} was not text`);
  }
  return raw;
}

function nonEmptyString(raw: unknown, name: string): string {
  const value = requiredString(raw, name);
  if (value === '') {
    throw new ProtocolError(`${name} was empty`);
  }
  return value;
}

function stringArray(raw: unknown, name: string): readonly string[] {
  if (!Array.isArray(raw) || !raw.every((entry) => typeof entry === 'string')) {
    throw new ProtocolError(`${name} was not a list of strings`);
  }
  return raw;
}

function unsignedInteger(raw: unknown, name: string): number {
  if (typeof raw !== 'number' || !Number.isSafeInteger(raw) || raw < 0) {
    throw new ProtocolError(`${name} was not an unsigned safe integer`);
  }
  return raw;
}

function positiveInteger(raw: unknown, name: string): number {
  const value = unsignedInteger(raw, name);
  if (value === 0) {
    throw new ProtocolError(`${name} was not positive`);
  }
  return value;
}

function requiredBoolean(raw: unknown, name: string): boolean {
  if (typeof raw !== 'boolean') {
    throw new ProtocolError(`${name} was not boolean`);
  }
  return raw;
}

function memberOf<T extends string>(raw: unknown, values: readonly T[], name: string): T {
  if (typeof raw !== 'string' || !values.includes(raw as T)) {
    throw new ProtocolError(`${name} used an unrecognised value`);
  }
  return raw as T;
}
