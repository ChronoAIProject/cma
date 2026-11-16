import test from 'node:test';
import assert from 'node:assert/strict';
import { performanceClass, p95Allowance, classTableSha256 } from '../lib/performance-policy.mjs';
import { compareCohorts } from '../lib/metrics.mjs';
import { config, cohort } from './fixtures.mjs';

test('A3 keeps critical dependencies critical and refuses unknown/default classifications', () => {
  assert.equal(performanceClass('catalog', 'scorecard-L42').class, 'CRITICAL');
  assert.equal(performanceClass('catalog', 'scorecard-L28').class, 'UX-AFFECTING');
  assert.equal(performanceClass('operation', 'internal-relay-no-user-wait').class, 'NON-SENSITIVE');
  assert.equal(performanceClass('operation', 'internal-relay-no-user-wait', true).class, 'CRITICAL');
  assert.throws(() => performanceClass('route', 'GET /unclassified'), /class_missing/);
  assert.throws(() => performanceClass('unknown', 'send'), /identity_kind/);
  assert.throws(() => performanceClass('catalog', 'scorecard-L42', 'false'), /dependency/);
});

test('A3 allowances use one fixed baseline without adding noise or mixing class caps', () => {
  const ux = p95Allowance({ kind: 'operation', id: 'desktop-connect-viewer-echo' }, 20, { noiseMs: 100 });
  assert.equal(ux.allowance_ms, 2); assert.equal(ux.aim_ms, 1);
  const background = p95Allowance({ kind: 'operation', id: 'background-maintenance' }, 1000, { noiseMs: 100 });
  assert.equal(background.allowance_ms, 30);
  assert.throws(() => p95Allowance({ kind: 'catalog', id: 'scorecard-L42' }, 100, { noiseMs: 10 }), /noise/);
  assert.equal(p95Allowance({ kind: 'catalog', id: 'scorecard-L42' }, 100,
    { noiseMs: 2, noiseQualified: true }).allowance_ms, 2);
  assert.throws(() => p95Allowance({ kind: 'catalog', id: 'scorecard-L28' }, 0), /positive/);
});

test('matched evidence permits UX between five and ten percent while critical regression still fails', () => {
  const values = [cohort('n1'), cohort('n1'), cohort('n1'), cohort('n2'), cohort('scaling')];
  for (const sample of values[3].samples['scorecard-L28']) sample.elapsedMs = 108;
  for (const sample of values[3].samples.send) sample.elapsedMs = 108;
  let rows = compareCohorts(values, config.requiredRoutes);
  const ux = rows.find((r) => r.criterion_id === 'scorecard-L28' && r.phase === 'n2');
  assert.equal(ux.status, 'PASS'); assert.equal(ux.tolerance_ms, 10);
  assert.equal(ux.class_table_sha256, classTableSha256);
  assert.match(ux.optimization_note, /five_percent/);
  assert.equal(rows.find((r) => r.metric === 'send' && r.phase === 'n2').status, 'FAIL');
  for (const sample of values[3].samples['scorecard-L28']) sample.elapsedMs = 111;
  rows = compareCohorts(values, config.requiredRoutes);
  assert.equal(rows.find((r) => r.criterion_id === 'scorecard-L28' && r.phase === 'n2').status, 'FAIL');
});

test('zero baselines, errors and absolute target breaches cannot pass under A3', () => {
  const zeros = [cohort('n1', 0), cohort('n1', 0), cohort('n1', 0), cohort('n2'), cohort('scaling')];
  assert.ok(compareCohorts(zeros, config.requiredRoutes).every((r) => r.status === 'BLOCKED'));
  const values = [cohort('n1'), cohort('n1'), cohort('n1'), cohort('n2'), cohort('scaling')];
  values[3].samples['scorecard-L28'][0].ok = false;
  values[3].samples.create[0].elapsedMs = 5001;
  const rows = compareCohorts(values, config.requiredRoutes);
  assert.equal(rows.find((r) => r.criterion_id === 'scorecard-L28' && r.phase === 'n2').status, 'FAIL');
  assert.equal(rows.find((r) => r.metric === 'create' && r.phase === 'n2').status, 'FAIL');
});
