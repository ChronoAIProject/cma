import { readFileSync } from 'node:fs';
import { check, finite, hash } from './checks.mjs';
import { catalogSha256 } from './catalog.mjs';

export const classTableSha256 = 'ba77e15a30a70a3817a78f1ef873b66265671dc15b9d971ccd602f6e3672eb87';
export const originalClassTableSha256 = 'effce4365936c4029b2e9b2c27edeec05c396d918d3e36ea806e17e40c1823f9';
const bytes = readFileSync(new URL('../performance-classes-a3.json', import.meta.url));
check(hash(bytes) === classTableSha256, 'performance_class_table_changed', true);
const table = JSON.parse(bytes);
check(table.version === 'A3' && table.catalog_sha256 === catalogSha256, 'performance_policy_mismatch', true);

// The class/context is fixed before comparison; unknown operations get no
// permissive default. A synchronous critical dependency inherits CRITICAL.
export function performanceClass(kind, id, criticalDependency = false) {
  check(typeof criticalDependency === 'boolean', 'critical_dependency_required', true);
  const collections = { catalog: ['catalog_memberships', 'id'], route: ['routes', 'method_route'],
    operation: ['operations', 'id'] };
  check(Object.hasOwn(collections, kind), 'performance_identity_kind_invalid', true);
  const [collection, field] = collections[kind];
  const match = table[collection].find((row) => row[field] === id);
  check(match, 'performance_class_missing', true);
  return { class: criticalDependency ? 'CRITICAL' : match.class, context: match.context,
    kind, id, critical_dependency: criticalDependency, class_table_sha256: classTableSha256 };
}

export function p95Allowance(identity, baselineP95, { noiseMs, noiseQualified = false } = {}) {
  const classification = performanceClass(identity.kind, identity.id, identity.criticalDependency);
  finite(baselineP95);
  check(baselineP95 > 0, 'positive_performance_baseline_required', true);
  if (classification.class === 'CRITICAL') {
    check(noiseQualified === true, 'critical_noise_not_registered', true);
    finite(noiseMs);
    return { ...classification, allowance_ms: noiseMs, aim_ms: null };
  }
  if (classification.class === 'UX-AFFECTING') {
    return { ...classification, allowance_ms: baselineP95 * 0.10, aim_ms: baselineP95 * 0.05 };
  }
  check(classification.class === 'NON-SENSITIVE', 'performance_class_invalid', true);
  return { ...classification, allowance_ms: 30, aim_ms: null };
}
