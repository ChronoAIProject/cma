import { readFileSync } from 'node:fs';
import { identifier, object, origin, privateFile, settings } from './environment';
import { assertLoadSeparation, blocked, check } from './visitor-abuse-model';

export const tc117Visitors = ['boundary', 'a', 'b', 'c', 'utc'] as const;
export type Visitor = (typeof tc117Visitors)[number];
export type Actor = Visitor | 'owner' | 'admin' | 'anonymous';
export interface BearerAccount {
  subject: string;
  bearerFile: string;
}
export interface Target {
  id: string;
  publicId: string;
  placementId: string;
}
export interface LoadBudget {
  frozen: true;
  perPodPerMinute: number;
  perPodConcurrency: number;
  networkPerMinute: number;
  maxServingPods: number;
  fleetSandboxLimit: number;
  belowRps: number;
  aboveRps: number;
  durationSeconds: number;
  proxyFiles: string[];
}
export interface TC117Fixture {
  accounts: Record<Exclude<Actor, 'anonymous'>, BearerAccount>;
  triggers: Record<'alwaysNew' | 'reuseA' | 'reuseB', Target>;
  forbiddenValuesFile: string;
  instances: string[];
  sandboxProfile: { id: string; revision: number };
  load?: LoadBudget;
}
export function instanceOrigins(value: string | undefined): string[] {
  if (value === undefined) return [];
  const values = value.split(',').map((entry) => origin(entry.trim(), 'CMA_E2E_INSTANCE_ORIGINS'));
  if (values.length !== 2 || new Set(values).size !== 2)
    throw new Error('Invalid E2E CMA_E2E_INSTANCE_ORIGINS: expected two distinct HTTPS origins');
  return values;
}
function positive(value: unknown, name: string, max = 100_000): number {
  if (!Number.isSafeInteger(value) || Number(value) < 1 || Number(value) > max)
    throw new Error(`Invalid E2E prerequisite: ${name}`);
  return Number(value);
}
export function tc117Fixture(env = process.env): TC117Fixture {
  const config = settings(env);
  if (config.mode !== 'mutation') throw new Error('Mutation mode is required for TC-117');
  const document = object(
    JSON.parse(readFileSync(privateFile(env.CMA_E2E_FIXTURE_FILE, 'CMA_E2E_FIXTURE_FILE'), 'utf8')),
  );
  if (document.origin !== config.baseURL)
    throw new Error('Fixture origin does not match mutation target');
  const section = object(document.tc117, 'tc117');
  const accounts = object(section.accounts, 'tc117.accounts');
  const parsed = {} as TC117Fixture['accounts'];
  for (const name of ['owner', 'admin', ...tc117Visitors] as const) {
    const row = object(accounts[name], `tc117.accounts.${name}`);
    parsed[name] = {
      subject: identifier(row.subject, `tc117.accounts.${name}.subject`),
      bearerFile: privateFile(row.bearerFile, `tc117.accounts.${name}.bearerFile`),
    };
    readBearer(parsed[name]);
  }
  if (new Set(Object.values(parsed).map((row) => row.subject)).size !== Object.keys(parsed).length)
    throw new Error('Each owner and TC-117 scenario requires a distinct identity');
  if (
    document.fixtureOwner !== parsed.owner.subject ||
    env.CMA_E2E_FIXTURE_OWNER !== parsed.owner.subject
  )
    throw new Error('CMA_E2E_FIXTURE_OWNER must match the TC-117 fixture owner');
  const triggers = object(section.triggers, 'tc117.triggers');
  const targets = {} as TC117Fixture['triggers'];
  for (const name of ['alwaysNew', 'reuseA', 'reuseB'] as const) {
    const row = object(triggers[name], `tc117.triggers.${name}`);
    targets[name] = {
      id: identifier(row.id, `tc117.triggers.${name}.id`),
      publicId: identifier(row.publicId, `tc117.triggers.${name}.publicId`),
      placementId: identifier(row.placementId, `tc117.triggers.${name}.placementId`),
    };
  }
  if (new Set(Object.values(targets).map((row) => row.publicId)).size !== 3)
    throw new Error('Invalid E2E tc117.triggers: three distinct published Triggers are required');
  const profile = object(section.sandboxProfile, 'tc117.sandboxProfile');
  let load: LoadBudget | undefined;
  if (section.load !== undefined) {
    const row = object(section.load, 'tc117.load');
    if (row.frozen !== true || !Array.isArray(row.proxyFiles) || row.proxyFiles.length < 2)
      throw new Error(blocked.budget);
    load = {
      frozen: true,
      perPodPerMinute: positive(row.perPodPerMinute, 'tc117.load.perPodPerMinute'),
      networkPerMinute: positive(row.networkPerMinute, 'tc117.load.networkPerMinute'),
      perPodConcurrency: positive(row.perPodConcurrency, 'tc117.load.perPodConcurrency', 256),
      maxServingPods: positive(row.maxServingPods, 'tc117.load.maxServingPods', 2),
      fleetSandboxLimit: positive(row.fleetSandboxLimit, 'tc117.load.fleetSandboxLimit'),
      belowRps: positive(row.belowRps, 'tc117.load.belowRps', 500),
      aboveRps: positive(row.aboveRps, 'tc117.load.aboveRps', 500),
      durationSeconds: positive(row.durationSeconds, 'tc117.load.durationSeconds', 60),
      proxyFiles: row.proxyFiles.map((file, i) => privateFile(file, `tc117.load.proxyFiles.${i}`)),
    };
    const networks = load.proxyFiles.map((file) =>
      identifier(
        object(JSON.parse(readFileSync(file, 'utf8')), 'tc117 egress proxy').egressNetwork,
        'tc117 egressNetwork',
      ),
    );
    check(
      new Set(networks).size === networks.length,
      'egress proxies must use operator-verified distinct effective rate-limit networks',
    );
    assertLoadSeparation(load, networks.length);
    check(
      load.belowRps < load.perPodPerMinute / 60 &&
        load.aboveRps > (load.perPodPerMinute * load.maxServingPods) / 60,
      'load must bracket the frozen fleet rate',
    );
  }
  return {
    accounts: parsed,
    triggers: targets,
    load,
    instances: instanceOrigins(env.CMA_E2E_INSTANCE_ORIGINS),
    sandboxProfile: {
      id: identifier(profile.id, 'tc117.sandboxProfile.id'),
      revision: positive(profile.revision, 'tc117.sandboxProfile.revision'),
    },
    forbiddenValuesFile: privateFile(document.forbiddenValuesFile, 'forbiddenValuesFile'),
  };
}
export function readBearer(account: BearerAccount): string {
  const value = readFileSync(account.bearerFile, 'utf8').trim();
  if (!value || /\s/.test(value)) throw new Error('Invalid E2E TC-117 bearer file content');
  return value;
}
