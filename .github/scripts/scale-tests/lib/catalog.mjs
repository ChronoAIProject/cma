import { readFileSync } from 'node:fs';
import { check, hash } from './checks.mjs';

export const catalogSha256 = 'a47475609633f3f736147dbded6dace5864d174b0cf58283425e639aba3ad951';
const bytes = readFileSync(new URL('../latency-route-catalog-a2.json', import.meta.url));
check(hash(bytes) === catalogSha256, 'frozen_catalog_changed', true);
export const catalog = JSON.parse(bytes);
const clientMetrics = { 15: 'create', 16: 'create_desktop', 23: 'open', 42: 'send', 43: 'interrupt', 52: 'usage' };
export const userLimits = Object.freeze({ create: 5000, create_desktop: 5000, open: 1000,
  send: 1000, interrupt: 1000, usage: 2000 });
// Criterion IDs preserve repeated route membership and conditional/alternate paths.
// Never merge two rows just because their HTTP method and path are the same.
export const criteria = Object.freeze(catalog.rows.map((row) => Object.freeze({
  ...row, metric: clientMetrics[row.source_line] ?? row.id,
  limitMs: userLimits[clientMetrics[row.source_line]] ?? null,
})));
export const requiredRoutes = Object.freeze(criteria.filter((row) => row.method).map((row) => row.metric));
export const criterionFor = (metric) => criteria.find((row) => row.metric === metric);
