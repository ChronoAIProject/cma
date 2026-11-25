import { createHash } from 'node:crypto';

export const sha256 = (value) => createHash('sha256').update(value).digest('hex');
export const compare = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

export function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

export function object(value, label) {
  requireValue(value && typeof value === 'object' && !Array.isArray(value), `${label}: expected object`);
  return value;
}

export function identifier(value, label) {
  requireValue(typeof value === 'string' && /^[A-Za-z0-9@_+./~-]+$/.test(value), `${label}: invalid identifier`);
  requireValue(value.length <= 240 && !value.includes('..'), `${label}: invalid identifier`);
  return value;
}

export function relativePath(value) {
  identifier(value, 'path');
  requireValue(!value.startsWith('/') && !value.endsWith('/') && !value.includes('//'), 'path: expected relative file');
  return value;
}

// This is conservative triage, not an SPDX expression evaluator or license choice.
// Every named alternative is retained; unfamiliar expressions require human review.
export function assessLicense(value, policy) {
  if (value === undefined || value === null || value === '') {
    return { license: 'NOASSERTION', flags: ['missing-license'], obligations: [] };
  }
  requireValue(typeof value === 'string', 'license: expected string');
  if (!/^[A-Za-z0-9().+ /-]+$/.test(value) || /SEE LICENSE|UNLICENSED|DocumentRef-/.test(value)) {
    return { license: 'NOASSERTION', declarationSha256: sha256(value), flags: ['custom-license-review'], obligations: [] };
  }
  const tokens = [...new Set(value.split(/[()\s/]+/).filter(Boolean))];
  const ids = tokens.filter((token) => !['AND', 'OR', 'WITH'].includes(token));
  const obligations = ids.filter((id) => policy.obligations[id]).sort(compare);
  const unknown = ids.filter((id) => !policy.recognized.includes(id) && !policy.obligations[id]);
  const flags = [];
  if (value.includes('LicenseRef-')) flags.push('custom-license-review');
  if (unknown.length || ids.length === 0) flags.push('unrecognized-license-review');
  if (tokens.includes('OR') || value.includes('/')) flags.push('license-choice-pending');
  if (obligations.length) flags.push('obligation-review');
  return { license: value, flags, obligations };
}

export function normalizeRecords(records) {
  return records.map((record) => ({ ...record, flags: [...new Set(record.flags)].sort(compare) }))
    .sort((a, b) => compare(JSON.stringify(a), JSON.stringify(b)));
}

export function canonicalJson(value) {
  const canonical = (item) => {
    if (Array.isArray(item)) return item.map(canonical);
    if (item && typeof item === 'object') {
      return Object.fromEntries(Object.keys(item).sort(compare).map((key) => [key, canonical(item[key])]));
    }
    return item;
  };
  return `${JSON.stringify(canonical(value), null, 2)}\n`;
}
