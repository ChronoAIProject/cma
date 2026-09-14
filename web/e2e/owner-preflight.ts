import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { request, type APIRequestContext } from '@playwright/test';
import { object } from './environment';
import { assertCredentialFree, credentialValues, prepareEvidenceDirectory } from './security';
import {
  assertOwnerSession,
  ownerFixture,
  ownerScope,
  ownerSettings,
  type OwnerFixture,
} from './owner-environment';

export async function ownerRead(
  api: APIRequestContext,
  origin: string,
  path: string,
  forbidden: readonly string[],
) {
  if (!path.startsWith('/api/v1/') && path !== '/auth/session')
    throw new Error('E2E owner read path refused');
  const response = await api.get(`${origin}${path}`, { maxRedirects: 0, timeout: 30_000 });
  if (!response.ok()) throw new Error('E2E owner public read unavailable');
  const text = await response.text();
  assertCredentialFree(text, forbidden);
  return JSON.parse(text) as unknown;
}

export async function validateOwnerPrerequisites(
  api: APIRequestContext,
  fixture: OwnerFixture,
  forbidden: readonly string[],
) {
  const read = (path: string) => ownerRead(api, fixture.origin, path, forbidden);
  assertOwnerSession(await read('/auth/session'), fixture.subject);
  const contents: Record<string, Record<string, unknown>> = {};
  for (const kind of ['sandbox', 'workspace', 'agent'] as const) {
    const pin = fixture.profiles[kind];
    const path = `/api/v1/${kind}-profiles/${pin.id}`;
    const profile = object(await read(path), 'owner profile');
    if (
      profile.owner !== fixture.subject ||
      profile.id !== pin.id ||
      profile.retired_at !== undefined
    )
      throw new Error('E2E owner profile must belong to the expected account and remain available');
    const revision = object(
      await read(`${path}/revisions/${pin.revision}`),
      'owner profile revision',
    );
    if (
      revision.profile_id !== pin.id ||
      revision.revision !== pin.revision ||
      revision.digest !== pin.digest ||
      typeof revision.published_at !== 'string' ||
      !Number.isFinite(Date.parse(revision.published_at))
    )
      throw new Error('E2E owner profile does not match the exact reviewed published revision');
    contents[kind] = object(revision.content, 'owner profile content');
  }
  const sandbox = contents.sandbox!;
  const workspace = contents.workspace!;
  if (
    !Array.isArray(sandbox.workspace_profiles) ||
    sandbox.workspace_profiles.length !== 0 ||
    !Array.isArray(workspace.agent_profiles) ||
    workspace.agent_profiles.length !== 0
  )
    throw new Error('E2E owner baseline requires empty automatic child profile lists');
  if (sandbox.agent_provider !== 'codex' || sandbox.model_access !== 'chronoai_managed')
    throw new Error(
      'E2E owner baseline requires the managed supported runtime without provider login',
    );
  if (
    !Number.isSafeInteger(sandbox.max_workspaces) ||
    Number(sandbox.max_workspaces) < 1 ||
    !Number.isSafeInteger(sandbox.max_agents_per_workspace) ||
    Number(sandbox.max_agents_per_workspace) < 1
  )
    throw new Error('E2E owner profiles need capacity for one Workspace and Agent');
}

export default async function ownerPreflight() {
  const config = ownerSettings();
  const fixture = ownerFixture(config);
  const forbidden = credentialValues(fixture.forbiddenValuesFile);
  const complete = prepareEvidenceDirectory(config.output, process.env.CMA_OWNER_E2E_RUN ?? '');
  const api = await request.newContext({ storageState: fixture.storageState });
  try {
    await validateOwnerPrerequisites(api, fixture, forbidden);
  } finally {
    await api.dispose();
  }
  writeFileSync(
    join(config.output, 'provenance.json'),
    JSON.stringify({
      ...ownerScope,
      origin: config.baseURL,
      executedAt: new Date().toISOString(),
      provenance: fixture.provenance,
      provenanceSource: 'operator_declared',
      provenanceVerified: false,
      profiles: fixture.profiles,
    }),
    { mode: 0o600, flag: 'wx' },
  );
  complete();
}
