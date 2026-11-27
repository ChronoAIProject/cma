import { lstat, mkdir, realpath } from 'node:fs/promises';
import { basename, dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { check } from './checks.mjs';

async function destination(path) {
  try { return await realpath(path); }
  catch (error) {
    if (error.code !== 'ENOENT' || dirname(path) === path) throw error;
    try {
      const entry = await lstat(path);
      check(!entry.isSymbolicLink(), 'evidence_directory_unresolvable');
    } catch (error) { if (error.code !== 'ENOENT') throw error; }
    return resolve(await destination(dirname(path)), basename(path));
  }
}

export async function prepareEvidenceDirectory(repository, path) {
  const root = await realpath(repository);
  const outside = (output) => {
    const location = relative(root, output);
    check(location === '..' || location.startsWith(`..${sep}`) || isAbsolute(location),
      'evidence_must_be_outside_repository');
  };
  const output = await destination(resolve(path));
  outside(output);
  await mkdir(output, { recursive: true, mode: 0o700 });
  const created = await realpath(output);
  outside(created);
  return created;
}
