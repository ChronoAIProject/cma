// Exercise the prior supported binary against an isolated V2 home; never a live home.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { writeFileSync } from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { validateImage } from './runtime-image-evidence.mjs';

export function verifyRefusal(state, logs) {
  assert.equal(state.Running, false);
  assert.equal(state.ExitCode, 2);
  assert.equal(state.OOMKilled, false);
  assert(logs.includes('installation state directory contains an unknown file'));
  assert(!logs.includes('runtime supervisor listening'));
}

export async function downgrade(currentRef, currentRevision, priorRef, priorRevision, destination) {
  const docker = args => execFileSync('docker', args, { encoding: 'utf8', timeout: 120000, maxBuffer: 1024 * 1024 }).trim();
  const inspect = ref => JSON.parse(docker(['image', 'inspect', ref]))[0];
  const current = inspect(currentRef), prior = inspect(priorRef);
  validateImage(current, currentRevision); validateImage(prior, priorRevision);
  const name = `cma-ledger-downgrade-${randomUUID()}`;
  const currentName = `${name}-current`, oldName = `${name}-old`;
  const result = { schema: 'cma-runtime-ledger-downgrade-v1', currentRevision, priorRevision, currentImage: current.Id, priorImage: prior.Id, result: 'fail', cleanup: false };
  const label = `dev.chrono.cma.ledger-downgrade=${name}`;
  const common = ['--label', label, '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges=true', '--pids-limit', '64', '--memory', '256m', '--cpus', '2', '--mount', `type=volume,source=${name},target=/fixture`, '--tmpfs', '/tmp:rw,nosuid,nodev,size=16777216,mode=1777', '--env', 'CHRONO_SANDBOX_ID=ledger-downgrade-fixture', '--env', 'CHRONO_RUNTIME_GENERATION=1', '--env', `CHRONO_RUNTIME_HOME_IDENTITY=${'a'.repeat(43)}`, '--env', 'CHRONO_RUNTIME_HOME=/fixture/home', '--env', 'CHRONO_RUNTIME_STATE_ROOT=/fixture/state', '--env', 'TMPDIR=/fixture/transfer', '--env', `CHRONO_RUNTIME_CONTROL_TOKEN=${randomBytes(32).toString('hex')}`];
  const read = path => docker(['run', '--rm', '--label', label, '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--mount', `type=volume,source=${name},target=/fixture,readonly`, '--entrypoint', '/bin/cat', current.Id, path]);
  const hash = bytes => createHash('sha256').update(bytes).digest('hex');
  let volume = false, running = false, old = false;
  try {
    docker(['volume', 'create', '--label', `dev.chrono.cma.ledger-downgrade=${name}`, name]); volume = true;
    docker(['run', '--rm', '--label', label, '--network', 'none', '--read-only', '--user', '0:0', '--mount', `type=volume,source=${name},target=/fixture`, '--entrypoint', '/bin/sh', current.Id, '-c', 'mkdir /fixture/home /fixture/state /fixture/transfer && chown -R 10001:10001 /fixture && chmod 700 /fixture /fixture/home /fixture/state /fixture/transfer']);
    docker(['create', '--name', currentName, ...common, current.Id]); running = true;
    docker(['start', currentName]);
    let ready = false;
    for (let attempt = 0; attempt < 40; attempt++) {
      const state = JSON.parse(docker(['inspect', currentName]))[0].State;
      assert(state.Running, 'new runtime exited before ready');
      const response = spawnSync('docker', ['exec', currentName, 'node', '-e', 'fetch("http://127.0.0.1:9090/livez").then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))'], { timeout: 5000, stdio: 'pipe' });
      if (response.status === 0) { ready = true; break; }
      await delay(250);
    }
    assert(ready, 'new runtime did not become ready');
    docker(['stop', '--time', '5', currentName]);
    const manifest = '/fixture/state/mutation-ledger-v2.current.json';
    const barrier = '/fixture/transfer/cma-checkpoints/runtime-state/mutation-ledger-v2.required.json';
    const before = read(manifest), marker = read(barrier);
    assert.equal(JSON.parse(before).version, 2);
    assert.equal(marker, before);
    docker(['create', '--name', oldName, ...common, '--env', 'CMA_MACHINE_INCLUDED=1', prior.Id]); old = true;
    docker(['start', oldName]);
    let state;
    for (let attempt = 0; attempt < 40; attempt++) {
      state = JSON.parse(docker(['inspect', oldName]))[0].State;
      if (!state.Running) break;
      await delay(250);
    }
    const logs = spawnSync('docker', ['logs', oldName], { encoding: 'utf8', timeout: 10000, maxBuffer: 65536 });
    assert.equal(logs.status, 0);
    verifyRefusal(state, logs.stdout + logs.stderr);
    assert.equal(read(manifest), before);
    assert.equal(read(barrier), marker);
    result.manifestSha256 = hash(before);
    result.refusal = 'installation-unknown-file-before-control-desktop-recovery';
    result.result = 'pass';
  } finally {
    if (old) docker(['rm', '--force', oldName]);
    if (running) docker(['rm', '--force', currentName]);
    for (const id of docker(['ps', '--all', '--quiet', '--filter', `label=${label}`]).split(/\s+/).filter(Boolean)) {
      assert.match(id, /^[0-9a-f]{12,64}$/); docker(['rm', '--force', id]);
    }
    if (volume) docker(['volume', 'rm', name]);
    result.cleanup = true;
    writeFileSync(destination, JSON.stringify(result, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await downgrade(...process.argv.slice(2));
}
