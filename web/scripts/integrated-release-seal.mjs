import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, createReadStream, existsSync } from 'node:fs';
import { readFile, writeFile, mkdir, copyFile, readdir } from 'node:fs/promises';
import { dirname, join } from 'node:path';

async function archivedPaths(directory, prefix = '') {
  const paths = [];
  for (const entry of await readdir(join(directory, prefix), { withFileTypes: true })) {
    const path = join(prefix, entry.name);
    if (entry.isSymbolicLink()) {
      assert(['web/node_modules', 'sdk/typescript/node_modules'].includes(path));
    } else if (entry.isDirectory()) paths.push(...(await archivedPaths(directory, path)));
    else paths.push(path);
  }
  return paths;
}

export async function verifySources(root, workspace, phase, archived = false) {
  const manifest = JSON.parse(await readFile(join(root, 'source-manifest.json'), 'utf8'));
  const differences = [];
  for (const entry of manifest.hashes) {
    const bytes = await readFile(join(workspace, entry.path)).catch(() => undefined);
    if (!bytes || createHash('sha256').update(bytes).digest('hex') !== entry.sha256)
      differences.push(entry.path);
  }
  const paths = archived
    ? await archivedPaths(workspace)
    : execFileSync('git', ['ls-files', '-c', '-o', '--exclude-standard', '-z'], {
        cwd: workspace,
      })
        .toString()
        .split('\0')
        .filter(Boolean);
  const recorded = new Set(manifest.hashes.map((entry) => entry.path));
  const added = paths.filter((path) => !recorded.has(path));
  const result = {
    phase,
    sourceDirectory: workspace,
    checked: manifest.hashes.length,
    differences,
    added,
    verifiedAt: new Date().toISOString(),
  };
  await writeFile(
    join(root, 'source-verification-' + phase + '.json'),
    JSON.stringify(result, null, 2),
  );
  assert.equal(differences.length + added.length, 0, 'Acceptance sources changed during execution');
  return result;
}

export async function retainBinary(root) {
  const log = await readFile(join(root, 'public-run.log'), 'utf8');
  const binary = log.match(/Running unittests src\/main.rs \(([^)]+)\)/)?.[1];
  assert(binary?.startsWith('/Users/liuyitian/code/cma-ui-worktrees/ticket19-cargo-target/'));
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(binary)) hash.update(chunk);
  const sha256 = hash.digest('hex');
  const destination = join(dirname(root), 'host-binaries', 'cma-' + sha256);
  await mkdir(dirname(destination), { recursive: true });
  if (!existsSync(destination)) await copyFile(binary, destination, constants.COPYFILE_FICLONE);
  const result = { binary, destination, sha256, verifiedAt: new Date().toISOString() };
  await writeFile(join(root, 'host-binary.json'), JSON.stringify(result, null, 2));
  return result;
}
