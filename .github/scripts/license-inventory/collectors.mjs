import { relative, dirname } from 'node:path';
import { assessLicense, identifier, object, relativePath, requireValue, sha256 } from './model.mjs';

export const NPM_LOCKS = [
  '.github/scripts/scale-tests/package-lock.json',
  'api/crates/layer4-runtime/image/desktop-provider/package-lock.json',
  'api/crates/layer4-runtime/image/npm/package-lock.json',
  'api/crates/layer4-runtime/image/provider/package-lock.json',
  'web/package-lock.json',
];

export function collectNpm(lock, path, policy) {
  object(lock, 'npm lock');
  requireValue([2, 3].includes(lock.lockfileVersion), 'npm lock: expected version 2 or 3');
  const packages = object(lock.packages, 'npm packages');
  requireValue(Object.hasOwn(packages, ''), 'npm lock: missing root package');
  return Object.entries(packages).filter(([location]) => location !== '').map(([location, value]) => {
    relativePath(location);
    const pkg = object(value, 'npm package');
    if (pkg.link) {
      return { ecosystem: 'npm', name: 'workspace-link', version: 'UNRESOLVED', evidence: `${path}#${location}`,
        license: 'NOASSERTION', obligations: [], flags: ['linked-package-review'] };
    }
    const name = identifier(pkg.name ?? location.split('node_modules/').at(-1), 'npm name');
    const version = identifier(pkg.version, 'npm version');
    const license = assessLicense(pkg.license, policy);
    const flags = [...license.flags];
    for (const flag of ['dev', 'optional', 'devOptional', 'inBundle']) {
      if (pkg[flag]) flags.push(flag);
    }
    if (!pkg.integrity) flags.push('integrity-missing');
    // Registry URLs may contain credentials or private hosts. Only publish their digest.
    return { ecosystem: 'npm', name, version, evidence: `${path}#${location}`, ...license, flags,
      sourceSha256: sha256(JSON.stringify([pkg.resolved ?? null, pkg.integrity ?? null])) };
  });
}

export function collectCargo(metadata, root, policy) {
  object(metadata, 'Cargo metadata');
  requireValue(metadata.version === 1 && Array.isArray(metadata.packages), 'Cargo metadata: expected format version 1');
  requireValue(Array.isArray(metadata.workspace_members) && metadata.workspace_members.length > 0, 'Cargo metadata: missing workspace');
  requireValue(metadata.resolve && Array.isArray(metadata.resolve.nodes), 'Cargo metadata: dependency graph required (no --no-deps)');
  const workspace = new Set(metadata.workspace_members);
  const ids = new Set(metadata.packages.map((pkg) => pkg.id));
  requireValue(ids.size === metadata.packages.length, 'Cargo metadata: duplicate package IDs');
  requireValue([...workspace].every((id) => ids.has(id)), 'Cargo metadata: missing workspace member');
  const records = metadata.packages.map((value) => {
    const pkg = object(value, 'Cargo package');
    const name = identifier(pkg.name, 'Cargo name');
    const version = identifier(pkg.version, 'Cargo version');
    const local = pkg.source === null;
    requireValue(typeof pkg.manifest_path === 'string', 'Cargo package: missing manifest path');
    const evidence = local ? relativePath(relative(root, pkg.manifest_path)) : 'api/Cargo.lock';
    const license = assessLicense(pkg.license, policy);
    const firstParty = workspace.has(pkg.id);
    const flags = license.flags.map((flag) => firstParty && flag === 'missing-license'
      ? 'workspace-license-metadata-missing' : flag);
    if (pkg.license_file) flags.push('license-file-review');
    return { ecosystem: firstParty ? 'workspace' : 'cargo', name, version, evidence, ...license, flags,
      sourceSha256: sha256(local ? dirname(evidence) : String(pkg.source)),
      licenseFileDeclared: Boolean(pkg.license_file) };
  });
  requireValue(records.some((record) => record.ecosystem === 'cargo'), 'Cargo metadata: no external dependencies');
  return records;
}

export async function collectVendored(catalog, files, read, policy) {
  requireValue(Array.isArray(catalog) && catalog.length > 0, 'vendor catalog: expected entries');
  const records = [];
  const inputHashes = {};
  for (const entry of catalog) {
    const name = identifier(entry.name, 'vendor name');
    const version = identifier(entry.version, 'vendor version');
    requireValue(Array.isArray(entry.evidence) && entry.evidence.length > 0, 'vendor: missing evidence');
    const matches = files.filter((path) => entry.prefixes?.some((prefix) => path.startsWith(`${relativePath(prefix)}/`)) || entry.files?.includes(path));
    requireValue(matches.length > 0, `vendor ${name}: no tracked files matched`);
    const missing = entry.evidence.filter((path) => !files.includes(relativePath(path)));
    for (const path of [...new Set([...matches, ...entry.evidence.filter((path) => !missing.includes(path))])]) {
      inputHashes[path] = sha256(await read(path));
    }
    const license = assessLicense(entry.license, policy);
    records.push({ ecosystem: 'vendored', name, version, evidence: entry.evidence.join('; '), ...license,
      flags: [...license.flags, ...(missing.length ? ['missing-vendored-notice'] : []), ...(entry.review ?? [])],
      files: matches.sort(), missingEvidence: missing });
  }
  return { records, inputHashes };
}
