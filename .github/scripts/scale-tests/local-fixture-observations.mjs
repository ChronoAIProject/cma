import { readFile, realpath, stat, writeFile } from 'node:fs/promises';
import { dirname, join, basename, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { check, hash, safeFailure } from './lib/checks.mjs';
import { prepareEvidenceDirectory } from './lib/evidence-directory.mjs';
import { validateConfig } from './lib/safety.mjs';
import { checkLocalFixtureObservationTarget, collectLocalFixtureObservation, qualifyProcessDeath, qualifyReceiverOutcomes } from './lib/local-fixture-observations.mjs';

async function readPinned(path, expected) {
  const canonical = await realpath(path);
  const info = await stat(canonical);
  check(canonical === path && info.isFile() && (info.mode & 0o077) === 0 && info.size <= 16 * 1024 * 1024,
    'local_observer_evidence_scope_invalid', true);
  const bytes = await readFile(canonical);
  check(hash(bytes) === expected, 'local_observer_evidence_changed', true);
  return JSON.parse(bytes.toString('utf8'));
}

try {
  const [configPath, kind, phase, output, beforePath, beforeHash, bindingPath, bindingHash, ...rest] = process.argv.slice(2);
  check(configPath && ['receiver', 'process'].includes(kind) && phase && output && !rest.length
    && Boolean(beforePath) === Boolean(beforeHash) && Boolean(bindingPath) === Boolean(bindingHash)
    && Boolean(bindingPath) === (phase === 'qualify') && (phase !== 'qualify' || beforePath),
  'local_observer_arguments_invalid', true);
  const config = validateConfig(JSON.parse(await readFile(configPath, 'utf8')));
  const before = beforePath ? await readPinned(beforePath, beforeHash) : undefined;
  const repository = fileURLToPath(new URL('../../..', import.meta.url));
  const directory = await prepareEvidenceDirectory(repository, dirname(resolve(output)));
  let result;
  if (phase === 'qualify') {
    checkLocalFixtureObservationTarget(config, kind, before);
    const binding = await readPinned(bindingPath, bindingHash);
    result = kind === 'receiver' ? qualifyReceiverOutcomes(before, binding) : qualifyProcessDeath(before, binding);
  } else {
    result = await collectLocalFixtureObservation(config, kind, phase, { before });
  }
  const bytes = JSON.stringify(result) + '\n';
  await writeFile(join(directory, basename(output)), bytes, { mode: 0o600, flag: 'wx' });
  console.log(JSON.stringify({ status: 'PASS', kind, phase, sha256: hash(bytes) }));
} catch (error) {
  console.error(JSON.stringify(safeFailure(error)));
  process.exitCode = 2;
}
