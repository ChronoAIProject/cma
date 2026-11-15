import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { catalogSha256, criteria, requiredRoutes, userLimits } from '../lib/catalog.mjs';
import { compareCohorts } from '../lib/metrics.mjs';
import { missingPerformance, saveSamples } from '../lib/report.mjs';
import { validateConfig } from '../lib/safety.mjs';
import { cohort, config } from './fixtures.mjs';

test('A2 preserves all 43 criteria, repeated route memberships and six exact client boundaries', () => {
  assert.equal(criteria.length, 43);
  assert.equal(requiredRoutes.length, 37);
  assert.equal(new Set(criteria.filter((r) => r.method).map((r) => `${r.method} ${r.route}`)).size, 34);
  assert.equal(new Set(criteria.map((r) => r.metric)).size, 43);
  assert.deepEqual(userLimits, { create: 5000, create_desktop: 5000, open: 1000, send: 1000,
    interrupt: 1000, usage: 2000 });
  assert.equal(missingPerformance().length, 129);
  assert.throws(() => validateConfig({ ...config, catalogSha256: 'unfrozen' }), /frozen_catalog_required/);
});

test('missing and undersampled routes stay in comparisons; both create boundaries enforce 5s', () => {
  const cohorts = ['n1', 'n1', 'n1', 'n2', 'scaling'].map((phase) => cohort(phase));
  delete cohorts[3].samples['scorecard-L32'];
  cohorts[4].samples['scorecard-L33'] = cohorts[4].samples['scorecard-L33'].slice(0, 1);
  delete cohorts[0].samples['scorecard-L57'];
  cohorts[3].samples.create_desktop[0].elapsedMs = 5001;
  const rows = compareCohorts(cohorts, requiredRoutes);
  assert.equal(rows.length, 43 * 5);
  assert.equal(rows.find((r) => r.metric === 'scorecard-L32' && r.phase === 'n2').status, 'BLOCKED');
  assert.equal(rows.find((r) => r.metric === 'scorecard-L33' && r.phase === 'scaling').n, 1);
  assert.ok(rows.filter((r) => r.metric === 'scorecard-L57').every((r) => r.status === 'BLOCKED'));
  const desktop = rows.find((r) => r.metric === 'create_desktop' && r.phase === 'n2');
  assert.equal(desktop.limit_ms, 5000); assert.equal(desktop.breaches_5s, 1); assert.equal(desktop.status, 'FAIL');
  assert.equal(rows.find((r) => r.metric === 'create' && r.phase === 'n2').status, 'PASS');
});

test('raw sample artifacts retain failures and missing routes, reject overwrite and omit private fields', async () => {
  const dir = await mkdtemp(join(import.meta.dirname, '.samples-'));
  try {
    const value = cohort('n1');
    value.samples.send[0] = { elapsedMs: 5100, ok: false, content: 'private-payload' };
    delete value.samples['scorecard-L57'];
    const receipt = await saveSamples(dir, value, 0);
    const text = await readFile(join(dir, receipt.file), 'utf8');
    const saved = JSON.parse(text);
    assert.equal(saved.catalog_sha256, catalogSha256);
    assert.equal(saved.observedReplicas, 1);
    assert.equal(saved.samples.send[0].ok, false);
    assert.deepEqual(saved.samples['scorecard-L57'], []);
    assert.ok(!text.includes('private-payload'));
    await assert.rejects(saveSamples(dir, value, 0), /EEXIST/);
    await assert.rejects(saveSamples(dir, { ...value, observedReplicas: 0 }, 1), /cohort_replica_evidence_missing/);
    value.samples.send[0].elapsedMs = NaN;
    await assert.rejects(saveSamples(dir, value, 1), /invalid_measurement/);
  } finally { await rm(dir, { recursive: true }); }
});
