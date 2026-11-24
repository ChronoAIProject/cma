import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { collectCargo, collectNpm, collectVendored } from './collectors.mjs';
import { assessLicense, canonicalJson, relativePath, sha256 } from './model.mjs';
import { makeInventory, renderNotices, renderReview } from './report.mjs';

const policy = JSON.parse(await readFile(new URL('./policy.json', import.meta.url)));
const lock = (packages) => ({ lockfileVersion: 3, packages: { '': { name: 'fixture-root' }, ...packages } });
const pkg = (license = 'MIT') => ({ version: '1.0.0', license, integrity: 'fixture-integrity' });
const cargo = () => ({ version: 1, workspace_members: ['own'], resolve: { nodes: [] }, packages: [
  { id: 'own', name: 'fixture-own', version: '0.1.0', source: null, manifest_path: '/fixture/api/Cargo.toml', license: null },
  { id: 'dep', name: 'fixture-dep', version: '1.0.0', source: 'registry+fixture', manifest_path: '/cache/fixture/Cargo.toml', license: 'MIT OR MPL-2.0' },
] });

test('npm includes nested, scoped, optional, dev and bundled locations without deduplication', () => {
  const rows = collectNpm(lock({
    'node_modules/@fixture/pkg': { ...pkg('MPL-2.0'), optional: true },
    'node_modules/outer/node_modules/pkg': { ...pkg('MIT'), dev: true, inBundle: true },
    'node_modules/pkg': { ...pkg('MIT'), devOptional: true },
    'node_modules/alias': { ...pkg('MIT'), name: '@fixture/actual' },
  }), 'web/package-lock.json', policy);
  assert.equal(rows.length, 4);
  assert.equal(rows[0].name, '@fixture/pkg');
  assert.ok(rows[0].flags.includes('optional'));
  assert.ok(rows[1].flags.includes('inBundle'));
  assert.ok(rows[1].flags.includes('dev'));
  assert.ok(rows[2].flags.includes('devOptional'));
  assert.equal(rows[3].name, '@fixture/actual');
});

test('npm preserves missing licenses and links as unresolved rows', () => {
  const rows = collectNpm(lock({
    'node_modules/unknown': { version: '1.0.0' },
    'node_modules/link': { link: true, resolved: '../fixture' },
  }), 'web/package-lock.json', policy);
  assert.deepEqual(rows[0].flags, ['missing-license', 'integrity-missing']);
  assert.equal(rows[1].version, 'UNRESOLVED');
  assert.ok(rows[1].flags.includes('linked-package-review'));
});

test('npm rejects malformed locks, records and unsafe identifiers', () => {
  for (const bad of [null, [], { lockfileVersion: 1 }, { lockfileVersion: 3, packages: {} },
    lock({ 'node_modules/pkg': null }), lock({ 'node_modules/pkg': { version: 'bad value' } }),
    lock({ '../escape': pkg() }), lock({ 'node_modules/pkg': pkg({ type: 'MIT' }) })]) {
    assert.throws(() => collectNpm(bad, 'web/package-lock.json', policy));
  }
  const v2 = lock({ 'node_modules/pkg': pkg() }); v2.lockfileVersion = 2;
  assert.equal(collectNpm(v2, 'web/package-lock.json', policy).length, 1);
});

test('triage retains whole expressions and never selects a permissive alternative', () => {
  for (const expression of ['MIT OR MPL-2.0', '(MIT OR Apache-2.0) AND MPL-2.0', 'MIT/MPL-2.0']) {
    const row = assessLicense(expression, policy);
    assert.equal(row.license, expression);
    assert.deepEqual(row.obligations, ['MPL-2.0']);
    assert.ok(row.flags.includes('license-choice-pending'));
  }
  assert.deepEqual(assessLicense('Apache-2.0 WITH LLVM-exception', policy).flags, []);
  assert.ok(assessLicense('Unfamiliar-1.0', policy).flags.includes('unrecognized-license-review'));
  assert.ok(assessLicense('()', policy).flags.includes('unrecognized-license-review'));
  assert.ok(assessLicense('LicenseRef-Example-1.0', policy).flags.includes('custom-license-review'));
  assert.equal(assessLicense('SEE LICENSE IN a-file', policy).license, 'NOASSERTION');
  assert.equal(assessLicense('https://example.invalid/license', policy).license, 'NOASSERTION');
  assert.equal(assessLicense('UNLICENSED', policy).license, 'NOASSERTION');
});

test('Cargo includes first-party packages separately and keeps license-file review', () => {
  const data = cargo(); data.packages[1].license_file = 'COPYING';
  const rows = collectCargo(data, '/fixture', policy);
  assert.equal(rows[0].ecosystem, 'workspace');
  assert.ok(rows[0].flags.includes('workspace-license-metadata-missing'));
  assert.ok(!rows[0].flags.includes('project-license-pending'));
  data.packages[0].license = 'Apache-2.0';
  assert.deepEqual(collectCargo(data, '/fixture', policy)[0].flags, []);
  assert.equal(rows[1].ecosystem, 'cargo');
  assert.ok(rows[1].flags.includes('license-file-review'));
  assert.ok(!JSON.stringify(rows).includes('/cache/'));
  const moved = cargo(); moved.packages[0].manifest_path = '/elsewhere/api/Cargo.toml';
  assert.deepEqual(collectCargo(cargo(), '/fixture', policy), collectCargo(moved, '/elsewhere', policy));
});

test('Cargo refuses incomplete graphs and local path escapes', () => {
  const cases = [null, { ...cargo(), version: 2 }, { ...cargo(), resolve: null },
    { ...cargo(), workspace_members: [] }, { ...cargo(), workspace_members: ['absent'] }];
  const duplicate = cargo(); duplicate.packages.push(duplicate.packages[0]); cases.push(duplicate);
  const escape = cargo(); escape.packages[0].manifest_path = '/outside/Cargo.toml'; cases.push(escape);
  const noManifest = cargo(); delete noManifest.packages[1].manifest_path; cases.push(noManifest);
  const noDeps = cargo(); noDeps.packages.pop(); cases.push(noDeps);
  for (const data of cases) assert.throws(() => collectCargo(data, '/fixture', policy));
});

test('vendored assets retain missing notice evidence and exact file digests', async () => {
  const catalog = [{ name: 'fixture-vendor', version: 'UNRESOLVED', license: 'MIT', prefixes: ['vendor/pkg'],
    files: ['assets/mask.svg'], evidence: ['vendor/pkg/NOTICE', 'vendor/pkg/LICENSE'], review: ['scope-review'] }];
  const files = ['vendor/pkg/code.js', 'vendor/pkg/NOTICE', 'vendor/pkg-other/ignored.js', 'assets/mask.svg'];
  const result = await collectVendored(catalog, files, async () => 'fixture bytes', policy);
  assert.equal(result.records[0].files.length, 3);
  assert.deepEqual(result.records[0].missingEvidence, ['vendor/pkg/LICENSE']);
  assert.ok(result.records[0].flags.includes('missing-vendored-notice'));
  assert.equal(result.inputHashes['assets/mask.svg'], sha256('fixture bytes'));
  await assert.rejects(collectVendored(catalog, [], async () => '', policy), /no tracked files/);
  await assert.rejects(collectVendored([], files, async () => '', policy), /expected entries/);
  await assert.rejects(collectVendored([{ ...catalog[0], evidence: [] }], files, async () => '', policy), /missing evidence/);
  await assert.rejects(collectVendored(catalog, files, async () => { throw new Error('fixture read failure'); }, policy), /read failure/);
});

test('reports are deterministic, escaped and explicit about limitations', () => {
  const rows = collectCargo(cargo(), '/fixture', policy);
  rows.push(...collectNpm(lock({ 'node_modules/pkg': pkg('MIT') }), 'web/package-lock.json', policy));
  const a = makeInventory(rows, { z: 'two', a: 'one' }, { node: 'fixture' }, []);
  const b = makeInventory([...rows].reverse(), { a: 'one', z: 'two' }, { node: 'fixture' }, []);
  assert.equal(canonicalJson(a), canonicalJson(b));
  assert.equal(renderNotices(a), renderNotices(b));
  assert.match(renderReview(a, policy), /MPL-2.0 \(weak-copyleft\)/);
  assert.match(renderNotices(a), /not a release SBOM/);
  assert.doesNotMatch(canonicalJson(a), /pending owner decisions|project-license-pending/);
  assert.doesNotMatch(renderNotices(a), /license decision pending/);
  assert.match(renderNotices(a), /No additional notice filenames/);
  a.unmappedNotices = ['fixture/NOTICE.txt'];
  a.records[0].name = '<fixture|name>';
  assert.match(renderNotices(a), /&lt;fixture&#124;name&gt;/);
  assert.match(renderNotices(a), /fixture\/NOTICE.txt/);
  assert.match(renderReview(a, policy), /workspace-license-metadata-missing/);
});

test('path validation rejects traversal, absolute paths and malformed segments', () => {
  for (const path of ['/absolute', '../parent', 'double//slash', 'trailing/', 'bad\nline', 'a'.repeat(241)]) {
    assert.throws(() => relativePath(path));
  }
  assert.equal(relativePath('web/file.json'), 'web/file.json');
});
