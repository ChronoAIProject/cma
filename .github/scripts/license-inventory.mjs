#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { dirname, resolve, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { collectCargo, collectNpm, collectVendored, NPM_LOCKS } from './license-inventory/collectors.mjs';
import { canonicalJson, compare, relativePath, requireValue, sha256 } from './license-inventory/model.mjs';
import { makeInventory, renderNotices, renderReview } from './license-inventory/report.mjs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const CONFIG = '.github/scripts/license-inventory';
const NOTICE_NAME = /^(?:(?:LICENSE|LICENCE|COPYING|NOTICE)(?:\.(?:txt|md)|[-_].*\.(?:txt|md))?|OFL-.*\.txt|.*-LICENSE)$/i;

export function runCommand(command, args, cwd) {
  const result = spawnSync(command, args, {
    cwd, encoding: 'utf8', timeout: 180_000, maxBuffer: 64 * 1024 * 1024,
    env: { ...process.env, CARGO_INCREMENTAL: '0', RUSTUP_AUTO_INSTALL: '0' },
  });
  // Cargo diagnostics may expose private source URLs and local paths.
  requireValue(!result.error && result.status === 0,
    `${command} failed; check the pinned toolchain, locked inputs and cache/network availability`);
  return result.stdout;
}

function parseJson(bytes, label) {
  try { return JSON.parse(bytes); }
  catch { throw new Error(`${label}: invalid JSON`); }
}

export async function generate({ root = ROOT, offline = false, run = runCommand, readSource = readFile } = {}) {
  const files = run('git', ['ls-files', '-z'], root).split('\0').filter(Boolean).sort(compare);
  files.forEach(relativePath);
  const locks = files.filter((path) => basename(path) === 'package-lock.json');
  requireValue(JSON.stringify(locks) === JSON.stringify(NPM_LOCKS), 'npm lock set changed; review and update NPM_LOCKS');
  const snapshot = new Map();
  const read = async (path) => {
    relativePath(path);
    if (!snapshot.has(path)) snapshot.set(path, await readSource(resolve(root, path)));
    return snapshot.get(path);
  };
  const policy = parseJson(await read(`${CONFIG}/policy.json`), 'policy');
  const catalog = parseJson(await read(`${CONFIG}/vendors.json`), 'vendor catalog');
  const inputHashes = {};
  const inputFiles = files.filter((path) => path === 'api/Cargo.lock' || path === 'api/rust-toolchain.toml' ||
    (path.startsWith('api/') && path.endsWith('/Cargo.toml')) || locks.includes(path) ||
    path === '.github/scripts/license-inventory.mjs' || path.startsWith(`${CONFIG}/`) ||
    NOTICE_NAME.test(basename(path)));
  for (const path of inputFiles) inputHashes[path] = sha256(await read(path));
  const pin = (await read('api/rust-toolchain.toml')).toString().match(/^channel\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"\s*$/m)?.[1];
  requireValue(pin, 'expected an exact Rust toolchain pin');
  requireValue(!process.env.RUSTUP_TOOLCHAIN, 'remove RUSTUP_TOOLCHAIN; use api/rust-toolchain.toml');
  const cargoVersion = run('cargo', ['--version'], resolve(root, 'api')).trim();
  requireValue(cargoVersion.startsWith(`cargo ${pin} `), 'Cargo version differs from the exact repository pin');
  const args = ['metadata', '--locked', '--all-features', '--format-version', '1'];
  if (offline) args.push('--offline');
  const metadata = parseJson(run('cargo', args, resolve(root, 'api')), 'Cargo metadata');
  const records = collectCargo(metadata, root, policy);
  for (const path of locks) records.push(...collectNpm(parseJson(await read(path), 'npm lock'), path, policy));
  const vendored = await collectVendored(catalog, files, read, policy);
  records.push(...vendored.records);
  Object.assign(inputHashes, vendored.inputHashes);
  // Hash manifests actually consumed from the Cargo cache without disclosing its paths.
  for (const pkg of metadata.packages.filter((pkg) => pkg.source !== null)) {
    const key = `cargo:${pkg.name}@${pkg.version}:${sha256(String(pkg.source))}`;
    inputHashes[key] = sha256(await readFile(pkg.manifest_path));
  }
  const mapped = new Set(catalog.flatMap((entry) => entry.evidence));
  const unmapped = files.filter((path) => NOTICE_NAME.test(basename(path)) &&
    !['LICENSE', 'NOTICE'].includes(path) && !mapped.has(path));
  // Catch a concurrent lock/catalog/manifest edit before producing a mixed snapshot.
  for (const [path, digest] of Object.entries(inputHashes).filter(([path]) => !path.startsWith('cargo:'))) {
    requireValue(sha256(await readSource(resolve(root, path))) === digest, 'inventory input changed during collection; rerun');
  }
  return { ...makeInventory(records, inputHashes, { node: process.versions.node, cargo: cargoVersion, metadataFormat: 1 }, unmapped), policy };
}

export async function writeDrafts(output, inventory) {
  // An existing directory is refused so stale files cannot masquerade as current output.
  await mkdir(output, { recursive: false, mode: 0o700 });
  for (const [name, content] of [
    ['inventory.json', canonicalJson(inventory)],
    ['THIRD_PARTY_NOTICES.md', renderNotices(inventory)],
    ['COPYLEFT_REVIEW.md', renderReview(inventory, inventory.policy)],
  ]) await writeFile(resolve(output, name), content, { flag: 'wx', mode: 0o600 });
}

export async function main(args, options = {}) {
  if (args.length === 1 && args[0] === '--help') {
    console.log('Usage: node .github/scripts/license-inventory.mjs --output NEW_DIRECTORY [--offline]');
    return;
  }
  const offline = args.includes('--offline');
  const remaining = args.filter((arg) => arg !== '--offline');
  requireValue(args.length === remaining.length + Number(offline) && remaining.length === 2 && remaining[0] === '--output' && !remaining[1].startsWith('--'),
    'expected --output NEW_DIRECTORY [--offline]');
  console.error('license-inventory: collecting locked source dependencies and vendored notices');
  const inventory = await generate({ ...options, offline });
  const output = resolve(remaining[1]);
  await writeDrafts(output, inventory);
  console.error(`license-inventory: wrote draft; ${JSON.stringify(inventory.counts)}; third-party distribution review remains`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((error) => {
    // File-system errors can carry local paths. Publish only our fixed diagnostics.
    console.error(`license-inventory: ${error.code ? 'file operation failed; verify inputs and a new output directory' : error.message}`);
    process.exitCode = 1;
  });
}
