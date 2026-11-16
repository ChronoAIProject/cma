import test from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {cp, mkdtemp, rm} from 'node:fs/promises';
import {readFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {resetDefaults} from './settings-setup.mjs';

const repository = fileURLToPath(new URL('../../../../../../', import.meta.url));
const source = execFileSync('git', ['rev-parse', 'HEAD'], {cwd: repository, encoding: 'utf8'}).trim();
const defaults = JSON.parse(readFileSync(new URL('./reset-contract.json', import.meta.url))).defaults;

test('an archived coordinator uses the explicit source repository before any public setup request', async () => {
  const temporary = await mkdtemp(join(tmpdir(), 'cma-admin-source-'));
  try {
    // Match the parent's real layout: the coordinator contains source files but
    // no Git metadata. A real Git object read must come from the bound checkout.
    const archived = join(temporary, 'coordinator/.github/scripts/multi-am-features');
    await cp(fileURLToPath(new URL('../../../', import.meta.url)), archived, {
      recursive: true,
      filter: path => !path.split('/').includes('node_modules') && !path.endsWith('.test.mjs'),
    });
    const module = await import(pathToFileURL(join(archived, 'executable/cases/developer-01/settings-setup.mjs')));
    let reads = 0;
    const observations = [];
    const ctx = {am: 'a', request: async () => {reads++; throw Error('public_settings_read_reached');},
      observe: async (label, operation) => {const value = await operation(); observations.push({label, value}); return value;}};
    await assert.rejects(module.settingsPatchSetup(ctx, {config: {expected_api_source: source}}),
      /admin_reset_source_unavailable/);
    assert.equal(reads, 0, 'missing source custody blocks before setup');
    const config = {expected_api_source: source, admin_source_repository: repository};
    await assert.rejects(module.settingsPatchSetup(ctx, {config}), /public_settings_read_reached/);
    assert.equal(reads, 1);
    assert.deepEqual(await module.resetDefaults(source, repository), defaults);
    assert.equal(observations[0].label, 'admin_reset_source_contract');
    assert.equal(observations[0].value.source, source);
    assert.equal(Object.keys(observations[0].value.files).length, 5);
    assert.equal(JSON.stringify(observations).includes(repository), false, 'source paths stay out of evidence');
  } finally {await rm(temporary, {recursive: true, force: true});}
});

test('a cached source cannot authorize a different or invalid repository binding', async () => {
  assert.deepEqual(await resetDefaults(source, repository), defaults);
  for (const path of ['', '.', 42, null]) {
    await assert.rejects(resetDefaults(source, path), /admin_reset_repository_unbound/);
  }
  const temporary = await mkdtemp(join(tmpdir(), 'cma-admin-source-missing-'));
  try {
    await assert.rejects(resetDefaults(source, temporary), /admin_reset_source_unavailable/);
    await assert.rejects(resetDefaults(source, join(temporary, 'absent')), /admin_reset_source_unavailable/);
    await assert.rejects(resetDefaults('0'.repeat(40), repository), /admin_reset_source_unavailable/);
    await assert.rejects(resetDefaults('HEAD', repository), /admin_reset_source_unbound/);
  } finally {await rm(temporary, {recursive: true, force: true});}
});
