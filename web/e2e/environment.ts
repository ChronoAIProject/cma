import { existsSync, lstatSync, readFileSync, realpathSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import type { Page } from '@playwright/test';

export const faultNames = [
  'install_failure',
  'clone_failure',
  'worktree_conflict',
  'agent_start_timeout',
  'first_message_ambiguity',
  'worker_crash',
] as const;
export type FaultName = (typeof faultNames)[number];
export type Role = 'owner' | 'visitor' | 'secondOwner' | 'signup';
export const visitorCases = [
  'J1',
  'J2',
  'J3',
  'J4-oauth',
  'J4-api_key',
  'J4-device_code',
  'J7-retired',
  'J7-paused',
  'G-T1-markdown',
  'G-T1-html',
  'G-T1-javascript',
  'G-T1-react',
  'G-T3',
  ...faultNames.map((name) => `FAULT-${name}`),
  'FAULT-browser-close',
  'FAULT-popup-blocked',
  'FAULT-nyxid',
  'FAULT-nyxid-requirements',
  'FAULT-handoff-response',
  'SECURITY-negative',
  'SECURITY-csrf',
];
export interface Account {
  subject: string;
  storageState?: string;
}
export interface TriggerFixture {
  publicId: string;
  placementId: string;
}
export interface StagingFixtures {
  origin: string;
  fixtureOwner: string;
  baselineServices: string[];
  accounts: Record<Role, Account>;
  visitors: Record<string, Account>;
  triggers: Record<
    'fresh' | 'newWorkspace' | 'ask' | 'resume' | 'missingService' | 'retired' | 'paused' | 'quota',
    TriggerFixture
  >;
  faults: Record<FaultName, TriggerFixture & { expectedCode: string }>;
  connections: Record<'oauth' | 'api_key' | 'device_code', TriggerFixture>;
  profiles: Record<
    'sandbox' | 'workspace' | 'agent',
    { id: string; revision: number; name: string }
  >;
  embeds: Record<
    'markdown' | 'html' | 'javascript' | 'react',
    { url: string; placementId: string }
  >;
  provenance: Record<'commit' | 'nyxidCommit' | 'apiImage' | 'webImage' | 'runtimeImage', string>;
  hooksFile: string;
  forbiddenValuesFile: string;
}
export interface Hooks {
  // Hooks are audited operator-provided ordinary UI and scoped fault drivers, never NyxID admin.
  completeSignup(page: Page): Promise<void>;
  completeConnection(page: Page, mode: 'oauth' | 'api_key' | 'device_code'): Promise<void>;
  controlFault(input: {
    name:
      | FaultName
      | 'nyxid_unavailable'
      | 'orchestration_pause'
      | 'restart_pausing'
      | 'restart_reviving'
      | 'rotate_image';
    phase: 'arm' | 'release';
    fixtureOwner: string;
    launchId?: string;
    sandboxId?: string;
  }): Promise<void>;
  verifyRuntime(input: {
    phase: 'before_pause' | 'after_revive' | 'after_rotation';
    sandboxId: string;
    agentIds: string[];
  }): Promise<{
    isolatedHomes: boolean;
    installBeforeCredentials: boolean;
    softwarePresent: boolean;
    dirtyWorktreePresent: boolean;
    transcriptsPreserved: boolean;
    runtimeVersion: string;
  }>;
}

export function required(value: unknown, name: string): string {
  if (typeof value !== 'string' || value.trim() === '')
    throw new Error(`Required E2E prerequisite: ${name}`);
  return value;
}
export function object(value: unknown, name = 'JSON object'): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value))
    throw new Error(`Invalid E2E prerequisite: ${name}`);
  return value as Record<string, unknown>;
}
export function identifier(value: unknown, name: string): string {
  const result = required(value, name);
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}$/.test(result))
    throw new Error(`Invalid E2E identifier: ${name}`);
  return result;
}
export function origin(value: unknown, name: string): string {
  const url = new URL(required(value, name));
  if (
    url.protocol !== 'https:' ||
    url.username !== '' ||
    url.password !== '' ||
    url.pathname !== '/' ||
    url.search !== '' ||
    url.hash !== ''
  )
    throw new Error(`E2E requires an HTTPS origin without credentials or a path: ${name}`);
  return url.origin;
}
export function outsideRepository(value: unknown, name: string): string {
  const path = required(value, name);
  const root = realpathSync(fileURLToPath(new URL('../..', import.meta.url)));
  let ancestor = resolve(path);
  while (!existsSync(ancestor) && dirname(ancestor) !== ancestor) ancestor = dirname(ancestor);
  const physical = join(realpathSync(ancestor), relative(ancestor, resolve(path)));
  const rel = relative(root, physical);
  if (!isAbsolute(path) || (!rel.startsWith('../') && !isAbsolute(rel)))
    throw new Error(`E2E ${name} must be an absolute path outside the repository`);
  return path;
}
function privateFile(value: unknown, name: string): string {
  const path = outsideRepository(value, name);
  let stat;
  try {
    stat = lstatSync(path);
  } catch {
    throw new Error(`Required E2E private file is unavailable: ${name}`);
  }
  if (!stat.isFile() || (stat.mode & 0o077) !== 0)
    throw new Error(`E2E ${name} must be a private regular file (mode 0600)`);
  return path;
}
export function settings(env = process.env) {
  const baseURL = origin(env.CMA_E2E_BASE_URL, 'CMA_E2E_BASE_URL');
  const mode = env.CMA_E2E_MODE ?? 'smoke';
  if (mode !== 'smoke' && mode !== 'mutation') throw new Error('Invalid CMA_E2E_MODE');
  if (mode === 'mutation' && env.CMA_E2E_MUTATION_TARGET !== baseURL)
    throw new Error(
      'Mutation mode requires CMA_E2E_MUTATION_TARGET to equal the exact base origin',
    );
  return {
    baseURL,
    mode,
    publicId: identifier(env.CMA_E2E_PUBLIC_ID, 'CMA_E2E_PUBLIC_ID'),
    output: outsideRepository(env.CMA_E2E_OUTPUT_DIR, 'CMA_E2E_OUTPUT_DIR'),
  };
}
function trigger(value: unknown, name: string): TriggerFixture {
  const row = object(value, name);
  return {
    publicId: identifier(row.publicId, `${name}.publicId`),
    placementId: identifier(row.placementId, `${name}.placementId`),
  };
}
export function fixtures(env = process.env): StagingFixtures {
  const config = settings(env);
  if (config.mode !== 'mutation')
    throw new Error('Mutation fixtures cannot be loaded in smoke mode');
  const document = object(
    JSON.parse(readFileSync(privateFile(env.CMA_E2E_FIXTURE_FILE, 'CMA_E2E_FIXTURE_FILE'), 'utf8')),
  );
  if (document.origin !== config.baseURL)
    throw new Error('Fixture origin does not match mutation target');
  const accounts = object(document.accounts, 'accounts');
  const parsedAccounts = {} as StagingFixtures['accounts'];
  for (const role of ['owner', 'secondOwner'] as const) {
    const account = object(accounts[role], `accounts.${role}`);
    parsedAccounts[role] = {
      subject: identifier(account.subject, `accounts.${role}.subject`),
      storageState: privateFile(account.storageState, `accounts.${role}.storageState`),
    };
  }
  const visitors = object(document.visitors, 'visitors');
  const parsedVisitors: Record<string, Account> = {};
  for (const name of visitorCases) {
    const account = object(visitors[name], `visitors.${name}`);
    parsedVisitors[name] = {
      subject: identifier(account.subject, `visitors.${name}.subject`),
      storageState: privateFile(account.storageState, `visitors.${name}.storageState`),
    };
  }
  const identities = [...Object.values(parsedAccounts), ...Object.values(parsedVisitors)].map(
    (value) => value.subject,
  );
  if (new Set(identities).size !== identities.length)
    throw new Error(
      'Each owner and visitor scenario requires a distinct identity to preserve real quotas',
    );
  parsedAccounts.visitor = parsedVisitors.J1!;
  parsedAccounts.signup = { subject: '' };
  const fixtureOwner = identifier(document.fixtureOwner, 'fixtureOwner');
  if (!Array.isArray(document.baselineServices) || document.baselineServices.length === 0)
    throw new Error('Required E2E prerequisite: baselineServices');
  const baselineServices = document.baselineServices.map((value) =>
    identifier(value, 'baselineServices'),
  );
  if (fixtureOwner !== parsedAccounts.owner.subject || env.CMA_E2E_FIXTURE_OWNER !== fixtureOwner)
    throw new Error('CMA_E2E_FIXTURE_OWNER must match the manifest fixture owner');
  const triggers = object(document.triggers, 'triggers');
  const parsedTriggers = {} as StagingFixtures['triggers'];
  for (const key of [
    'fresh',
    'newWorkspace',
    'ask',
    'resume',
    'missingService',
    'retired',
    'paused',
    'quota',
  ] as const)
    parsedTriggers[key] = trigger(triggers[key], `triggers.${key}`);
  const faults = object(document.faults, 'faults');
  const connections = object(document.connections, 'connections');
  const parsedConnections = {} as StagingFixtures['connections'];
  for (const mode of ['oauth', 'api_key', 'device_code'] as const)
    parsedConnections[mode] = trigger(connections[mode], `connections.${mode}`);
  const parsedFaults = {} as StagingFixtures['faults'];
  for (const key of faultNames) {
    const row = object(faults[key], `faults.${key}`);
    parsedFaults[key] = {
      ...trigger(row, `faults.${key}`),
      expectedCode: identifier(row.expectedCode, `faults.${key}.expectedCode`),
    };
  }
  const profiles = object(document.profiles, 'profiles');
  const parsedProfiles = {} as StagingFixtures['profiles'];
  for (const kind of ['sandbox', 'workspace', 'agent'] as const) {
    const row = object(profiles[kind], `profiles.${kind}`);
    if (!Number.isSafeInteger(row.revision) || Number(row.revision) < 1)
      throw new Error(`Invalid profiles.${kind}.revision`);
    parsedProfiles[kind] = {
      id: identifier(row.id, `profiles.${kind}.id`),
      revision: Number(row.revision),
      name: required(row.name, `profiles.${kind}.name`),
    };
  }
  const embeds = object(document.embeds, 'embeds');
  const parsedEmbeds = {} as StagingFixtures['embeds'];
  for (const kind of ['markdown', 'html', 'javascript', 'react'] as const) {
    const row = object(embeds[kind], `embeds.${kind}`);
    const url = new URL(required(row.url, `embeds.${kind}.url`));
    if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash)
      throw new Error('Embed fixtures require public HTTPS URLs without query strings');
    parsedEmbeds[kind] = {
      url: url.href,
      placementId: identifier(row.placementId, `embeds.${kind}.placementId`),
    };
  }
  const provenance = object(document.provenance, 'provenance');
  const parsedProvenance = {} as StagingFixtures['provenance'];
  for (const field of ['commit', 'nyxidCommit', 'apiImage', 'webImage', 'runtimeImage'] as const) {
    const value = required(provenance[field], `provenance.${field}`);
    const pattern = field.endsWith('Image')
      ? /^[a-zA-Z0-9./_-]+@sha256:[a-f0-9]{64}$/
      : /^[a-f0-9]{40}$/;
    if (!pattern.test(value)) throw new Error(`Expected pinned provenance.${field}`);
    parsedProvenance[field] = value;
  }
  return {
    origin: config.baseURL,
    fixtureOwner,
    baselineServices,
    accounts: parsedAccounts,
    visitors: parsedVisitors,
    triggers: parsedTriggers,
    connections: parsedConnections,
    faults: parsedFaults,
    profiles: parsedProfiles,
    embeds: parsedEmbeds,
    provenance: parsedProvenance,
    hooksFile: privateFile(document.hooksFile, 'hooksFile'),
    forbiddenValuesFile: privateFile(document.forbiddenValuesFile, 'forbiddenValuesFile'),
  };
}
export async function hooks(fixture: StagingFixtures): Promise<Hooks> {
  const loaded = object(await import(pathToFileURL(fixture.hooksFile).href), 'hooks exports');
  for (const name of ['completeSignup', 'completeConnection', 'controlFault', 'verifyRuntime'])
    if (typeof loaded[name] !== 'function') throw new Error(`Required E2E operator hook: ${name}`);
  return loaded as unknown as Hooks;
}
