import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { openSync, closeSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { artifact } from './worker-runtime-cohort.mjs';

export async function command(program, args, options, output) {
  const fd = openSync(output, 'w', 0o600);
  try {
    const child = spawn(program, args, { ...options, stdio: ['ignore', fd, fd] });
    return await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('exit', (code, signal) => resolve({ code, signal }));
    });
  } finally { closeSync(fd); }
}

export async function buildExecutable(root, directory, target, test, env) {
  const output = join(directory, target + '-build.jsonl');
  const errors = openSync(join(directory, target + '-build.stderr'), 'w', 0o600);
  const fd = openSync(output, 'w', 0o600);
  const args = ['+1.96.1', test ? 'test' : 'build', '--locked', '--manifest-path', join(root, 'api/Cargo.toml'),
    '-p', target, '--bin', target, '--message-format=json', ...(test ? ['--no-run'] : [])];
  let status;
  try {
    const child = spawn('cargo', args, { cwd: root, env: { ...env, CARGO_INCREMENTAL: '0' }, stdio: ['ignore', fd, errors] });
    status = await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('exit', (code, signal) => resolve({ code, signal }));
    });
  } finally { closeSync(fd); closeSync(errors); }
  assert.equal(status.code, 0, target + ' build failed; inspect retained build log');
  assert.equal(status.signal, null);
  return { ...artifact(readFileSync(output, 'utf8'), root, target, test), command: ['cargo', ...args] };
}
