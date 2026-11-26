import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { faultNames, visitorCases } from './environment';

export function fixtureDocument() {
  const directory = mkdtempSync(join(tmpdir(), 'cma-e2e-fixture-'));
  const state = join(directory, 'state.json');
  const manifest = join(directory, 'manifest.json');
  const forbidden = join(directory, 'forbidden.json');
  const hooksFile = join(directory, 'hooks.mjs');
  writeFileSync(state, JSON.stringify({ cookies: [], origins: [] }), { mode: 0o600 });
  writeFileSync(forbidden, JSON.stringify(['redacted-probe']), { mode: 0o600 });
  writeFileSync(hooksFile, 'export {};', { mode: 0o600 });
  const trigger = { publicId: 'public-fixture', placementId: 'placement-fixture' };
  const document = {
    origin: 'https://staging.example.test',
    fixtureOwner: 'fixture-owner',
    baselineServices: ['model'],
    publicJourneys: {
      repository: { url: 'https://github.com/example/repository', reference: 'f'.repeat(40) },
      owners: Object.fromEntries(
        ['owner', 'secondOwner'].map((role) => [
          role,
          {
            model: 'fixture-model',
            embedUrl: `https://embed.example.test/j5/${role}`,
          },
        ]),
      ),
    },
    accounts: {
      owner: { subject: 'fixture-owner', storageState: state },
      secondOwner: { subject: 'second-owner', storageState: state },
    },
    visitors: Object.fromEntries(
      visitorCases.map((name, index) => [
        name,
        { subject: `visitor-${index}`, storageState: state },
      ]),
    ),
    triggers: Object.fromEntries(
      [
        'fresh',
        'newWorkspace',
        'ask',
        'resume',
        'missingService',
        'retired',
        'paused',
        'quota',
      ].map((name) => [name, trigger]),
    ),
    connectionJourneys: {
      schema: 'cma-j4-fixtures-v1',
      services: Object.fromEntries(
        ['oauth', 'api_key', 'device_code'].map((mode) => [
          mode,
          {
            serviceId: `service-${mode}`,
            providerRevision: 'b'.repeat(40),
            fixtureDigest: 'f'.repeat(64),
          },
        ]),
      ),
    },
    connections: Object.fromEntries(
      ['oauth', 'api_key', 'device_code'].map((name) => [name, trigger]),
    ),
    faults: Object.fromEntries(
      faultNames.map((name) => [name, { ...trigger, expectedCode: 'expected_failure' }]),
    ),
    profiles: Object.fromEntries(
      ['agent', 'workspace', 'sandbox'].map((kind) => [
        kind,
        { id: `${kind}-profile`, revision: 1, name: 'Public fixture' },
      ]),
    ),
    embeds: Object.fromEntries(
      ['markdown', 'html', 'javascript', 'react'].map((kind) => [
        kind,
        {
          url: `https://embed.example.test/${kind}`,
          placementId: 'placement-fixture',
          triggerId: 'trigger-fixture',
        },
      ]),
    ),
    provenance: {
      commit: 'a'.repeat(40),
      nyxidCommit: 'b'.repeat(40),
      apiImage: `registry.example.test/api@sha256:${'c'.repeat(64)}`,
      webImage: `registry.example.test/web@sha256:${'d'.repeat(64)}`,
      runtimeImage: `registry.example.test/runtime@sha256:${'e'.repeat(64)}`,
    },
    hooksFile,
    forbiddenValuesFile: forbidden,
  };
  writeFileSync(manifest, JSON.stringify(document), { mode: 0o600 });
  const env = {
    CMA_E2E_BASE_URL: document.origin,
    CMA_E2E_PUBLIC_ID: trigger.publicId,
    CMA_E2E_OUTPUT_DIR: join(directory, 'output'),
    CMA_E2E_MODE: 'mutation',
    CMA_E2E_MUTATION_TARGET: document.origin,
    CMA_E2E_FIXTURE_OWNER: document.fixtureOwner,
    CMA_E2E_FIXTURE_FILE: manifest,
  };
  return { document, env, manifest, state, directory };
}
