import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { campaignFixture } from './public-journey-evidence.fixture.mjs';
import { requiredCases } from './staging-authority.mjs';

export const hash = (value) => createHash('sha256').update(value).digest('hex');
export function fixtures() {
  const images = Object.fromEntries(['api', 'web', 'runtime'].map((name) => [name, `registry.example.test/cma-${name}@sha256:${'a'.repeat(64)}`]));
  const request = { schema: 'cma-staging-attestation-request-v1', nonce: '00000000-0000-4000-8000-000000000001', expected: { origin: 'https://staging.example.test', commit: 'b'.repeat(40), nyxidCommit: 'c'.repeat(40), images } };
  const receipt = { schema: 'cma-staging-attestation-v1', nonce: request.nonce, observedAt: new Date().toISOString(), origin: request.expected.origin, commit: request.expected.commit, nyxidCommit: request.expected.nyxidCommit, runtimePin: images.runtime, images: {} };
  for (const name of Object.keys(images)) receipt.images[name] = {
    reference: images[name], rootDigest: `sha256:${'a'.repeat(64)}`,
    platformDigest: `sha256:${'d'.repeat(64)}`, configDigest: `sha256:${'e'.repeat(64)}`,
    source: 'https://github.com/ChronoAIProject/cma', revision: request.expected.commit,
    ...(name === 'runtime' ? {} : { runningImageID: `docker-pullable://registry.example.test/cma-${name}@sha256:${'d'.repeat(64)}` }),
  };
  return { request, receipt };
}

export const canaries = ['CANARY_ENV_ORIGIN', 'CANARY_ENV_OWNER', 'f'.repeat(40), 'CANARY_FIXTURE_BODY', 'CANARY_OBSERVER_STDOUT', 'CANARY_OBSERVER_STDERR'];
export const publicDiagnostic = 'Synthetic command failure\n';
export const line = (mode, phase, code = phase, step = 'attestation') => `Staging authority or final evidence refused (mode=${mode}${step ? ` step=${step}` : ''} phase=${phase} code=${code}); private inputs withheld.\n`;
export function noCanaries(stdout, stderr) {
  for (const canary of canaries) { assert.ok(!stdout.includes(canary)); assert.ok(!stderr.includes(canary)); }
}
export function harness(t) {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'cma-authority-diagnostics-')));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const path = (name) => join(directory, name);
  const write = (name, value) => writeFileSync(path(name), typeof value === 'string' ? value : JSON.stringify(value), { mode: 0o600 });
  const before = fixtures(), after = fixtures();
  after.request.nonce = after.receipt.nonce = '00000000-0000-4000-8000-000000000002';
  write('before.json', before); write('after.json', after); write('output', '');
  const { results, journal: proofJournal, connectionFixture } = campaignFixture(requiredCases);
  const identity = { run: 'test-run', at: new Date().toISOString(), kind: 'sandboxes', id: 'test-sandbox', subject: 'test-owner' };
  const journal = [{ ...identity, action: 'created' }, { ...identity, action: 'cleanup_complete' }, ...proofJournal];
  write('results.json', results); write('cleanup.ndjson', journal.map((row) => JSON.stringify(row)).join('\n'));
  const fixture = { ...connectionFixture, origin: before.request.expected.origin, fixtureOwner: 'test-owner', provenance: {
    commit: before.request.expected.commit, nyxidCommit: before.request.expected.nyxidCommit,
    ...Object.fromEntries(Object.entries(before.request.expected.images).map(([key, value]) => [`${key}Image`, value])),
  } };
  write('fixture.json', fixture); write('config.json', '{}');
  const observer = `import { readFileSync, writeFileSync } from 'node:fs';
    const request = JSON.parse(readFileSync(0, 'utf8'));
    const receipt = ${JSON.stringify(before.receipt)};
    receipt.nonce = request.nonce;
    process.stdout.write(JSON.stringify(receipt));`;
  write('observer.mjs', observer);
  const state = {
    policy: { name: 'staging-e2e-mutation', can_admins_bypass: false, deployment_branch_policy: { protected_branches: false, custom_branch_policies: true }, protection_rules: [{ type: 'required_reviewers', prevent_self_review: true, reviewers: [{}] }] },
    branches: { total_count: 1, branch_policies: [{ name: 'develop', type: 'branch' }] }, head: before.request.expected.commit,
    listing: { errors: [], suites: [{ specs: requiredCases.map((title) => ({ title, tests: [{ expectedStatus: 'passed', annotations: [] }] })) }] },
  };
  // Intercept every external command before loading the CLI; gh/git/Playwright never run.
  write('commands.mjs', `import child from 'node:child_process';
    import { syncBuiltinESMExports } from 'node:module';
    import { readFileSync } from 'node:fs';
    const original = child.execFileSync;
    const state = JSON.parse(readFileSync(${JSON.stringify(path('state.json'))}, 'utf8'));
    child.execFileSync = (program, args, options) => {
      if (state.fail === program) throw Object.assign(Error('${canaries[5]}'), { stderr: '${canaries[5]}' });
      if (state.diagnosticProgram === program) return original(process.execPath, ['-e', ${JSON.stringify(`process.stderr.write(${JSON.stringify(publicDiagnostic)}); process.exit(1);`)}], options);
      if (program === 'gh') return state.ghRaw ?? JSON.stringify(args[1].endsWith('/develop') ? { object: { sha: state.head } } : args[1].endsWith('/deployment-branch-policies') ? state.branches : state.policy);
      if (program === 'git') return state.head + '\\n';
      if (program === process.execPath && args.includes('--list')) return state.listingRaw ?? JSON.stringify(state.listing);
      if (program === process.execPath && args[0] === ${JSON.stringify(path('observer.mjs'))}) return original(program, args, options);
      throw Error('unexpected test command');
    };
    syncBuiltinESMExports();`);
  const env = {
    PATH: directory, GITHUB_REPOSITORY: 'ChronoAIProject/cma', GITHUB_REF: 'refs/heads/develop',
    REVIEWED_SHA: state.head, GITHUB_SHA: state.head, GITHUB_ACTION: 'attestation', GITHUB_OUTPUT: path('output'),
    CMA_ATTESTATION_VERIFIER: path('observer.mjs'), CMA_ATTESTATION_CONFIG: path('config.json'),
    VERIFIER_SHA256: hash(observer), CONFIG_SHA256: hash('{}'), CMA_E2E_FIXTURE_FILE: path('fixture.json'),
    CMA_E2E_MUTATION_TARGET: fixture.origin, CMA_E2E_BASE_URL: fixture.origin, CMA_E2E_FIXTURE_OWNER: fixture.fixtureOwner,
  };
  const run = (mode, args = mode === 'attest' ? [path('attested.json')] : mode === 'audit' ? [directory, path('before.json'), path('after.json')] : []) => {
    write('state.json', state);
    const result = spawnSync(process.execPath, ['--import', pathToFileURL(path('commands.mjs')).href, resolve('.github/scripts/staging-authority.mjs'), mode, ...args], { env, encoding: 'utf8', timeout: 10000 });
    assert.ifError(result.error); assert.equal(result.signal, null);
    noCanaries(result.stdout, result.stderr);
    return result;
  };
  const setObserver = (script) => { write('observer.mjs', script); env.VERIFIER_SHA256 = hash(script); };
  return { path, write, env, run, state, before, after, fixture, observer, setObserver, results, journal };
}
