import { lstatSync, readFileSync } from 'node:fs';
import { identifier, object, origin, outsideRepository, required } from './environment';

export const ownerCase =
  'OWNER-resource-lifecycle preserves its first answer across browser return and completes a second answer after pause/resume';
export const ownerCapabilities = [
  'create_sandbox',
  'use_own_sandboxes',
  'delete_own_sandbox',
  'manage_own_profiles',
] as const;
export const ownerScope = {
  identityClass: 'existing_owner',
  accountCount: 1,
  executionTarget: 'configured_remote_origin',
  scope: 'owner_only_partial',
  mocked: false,
  fullCampaignGatesAdvanced: false,
  deferredGates: [
    'J0',
    'J1',
    'J2',
    'J3',
    'J4',
    'J5',
    'J6',
    'J7',
    'G-A1-G-A5',
    'G-S1',
    'G-S2',
    'G-P1',
    'G-T1',
    'G-T2',
    'G-T3',
    'Storage',
    'fresh_signup',
    'visitor_restrictions',
    'cross_owner_isolation',
    'provider_connections',
    'real_embeds',
    'restart_rotation_faults',
  ],
} as const;

export interface OwnerProfile {
  readonly id: string;
  readonly revision: number;
  readonly digest: string;
}
export interface OwnerFixture {
  readonly origin: string;
  readonly subject: string;
  readonly storageState: string;
  readonly forbiddenValuesFile: string;
  readonly profiles: Record<'sandbox' | 'workspace' | 'agent', OwnerProfile>;
  readonly provenance: Record<
    'commit' | 'nyxidCommit' | 'apiImage' | 'webImage' | 'runtimeImage',
    string
  >;
}

function privateInput(value: unknown, name: string): string {
  const path = outsideRepository(value, name);
  let stat;
  try {
    stat = lstatSync(path);
  } catch {
    throw new Error(`Required E2E owner input unavailable: ${name}`);
  }
  if (!stat.isFile() || stat.isSymbolicLink() || (stat.mode & 0o777) !== 0o600)
    throw new Error(`Invalid E2E owner input: ${name} must be a regular 0600 file`);
  return path;
}

export function ownerSettings(env = process.env) {
  const baseURL = origin(env.CMA_OWNER_E2E_TARGET, 'CMA_OWNER_E2E_TARGET');
  if (env.CMA_OWNER_E2E_MUTATION_TARGET !== baseURL)
    throw new Error('E2E owner mutation target must equal the exact HTTPS origin');
  return {
    baseURL,
    subject: identifier(env.CMA_OWNER_E2E_SUBJECT, 'CMA_OWNER_E2E_SUBJECT'),
    output: outsideRepository(env.CMA_OWNER_E2E_OUTPUT_DIR, 'CMA_OWNER_E2E_OUTPUT_DIR'),
    fixtureFile: privateInput(env.CMA_OWNER_E2E_FIXTURE_FILE, 'CMA_OWNER_E2E_FIXTURE_FILE'),
  };
}

export function ownerFixture(config: ReturnType<typeof ownerSettings>): OwnerFixture {
  const row = object(JSON.parse(readFileSync(config.fixtureFile, 'utf8')), 'owner fixture');
  if (row.origin !== config.baseURL)
    throw new Error('E2E owner fixture origin does not match target');
  if (row.subject !== config.subject)
    throw new Error('E2E owner fixture account does not match expected subject');
  const profiles = object(row.profiles, 'owner profiles');
  const parsed = {} as OwnerFixture['profiles'];
  for (const kind of ['sandbox', 'workspace', 'agent'] as const) {
    const profile = object(profiles[kind], `owner ${kind} profile`);
    if (!Number.isSafeInteger(profile.revision) || Number(profile.revision) < 1)
      throw new Error('Invalid E2E owner profile revision');
    parsed[kind] = {
      id: identifier(profile.id, `owner ${kind} profile ID`),
      revision: Number(profile.revision),
      digest: required(profile.digest, `owner ${kind} reviewed digest`),
    };
  }
  const provenance = object(row.provenance, 'owner provenance');
  const pins = {} as OwnerFixture['provenance'];
  for (const field of ['commit', 'nyxidCommit', 'apiImage', 'webImage', 'runtimeImage'] as const) {
    const value = required(provenance[field], `owner provenance.${field}`);
    const pattern = field.endsWith('Image')
      ? /^[a-zA-Z0-9./_-]+@sha256:[a-f0-9]{64}$/
      : /^[a-f0-9]{40}$/;
    if (!pattern.test(value))
      throw new Error('Invalid E2E owner provenance: immutable pins required');
    pins[field] = value;
  }
  return {
    origin: config.baseURL,
    subject: config.subject,
    storageState: privateInput(row.storageState, 'owner browser state'),
    forbiddenValuesFile: privateInput(row.forbiddenValuesFile, 'owner credential sentinels'),
    profiles: parsed,
    provenance: pins,
  };
}

export function assertOwnerSession(value: unknown, subject: string) {
  const row = object(value, 'owner session');
  if (row.authenticated !== true || row.subject !== subject || row.visitor !== false)
    throw new Error('E2E owner session does not match the existing owner account');
  const capabilities = row.capabilities;
  if (
    !Array.isArray(capabilities) ||
    ownerCapabilities.some((name) => !capabilities.includes(name))
  )
    throw new Error('E2E owner session lacks required resource or cleanup capabilities');
}

export function assertOwnerCase(title: string) {
  if (title !== ownerCase)
    throw new Error('E2E owner entrypoint refuses non-owner or full-campaign case labels');
}
