// Re-run CMA's complete existing runtime contract on the Arch filesystem.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { combineProofs, pins, validateImage } from './runtime-image-evidence.mjs';
import { prepareProbe } from './omarchy-contract-source.mjs';

const [revision, destination] = process.argv.slice(2);
const source = readFileSync('api/crates/layer4-runtime/Dockerfile', 'utf8');
const expected = pins(source);
for (const key of ['CODEX_VERSION', 'CODEX_NATIVE_SHA256_AMD64', 'CODEX_LAUNCHER_SHA256']) {
  const match = source.match(new RegExp('^ARG DESKTOP_' + key + '=([^\\s]+)$', 'm'));
  assert(match);
  expected[key] = match[1];
}
const docker = args => execFileSync('docker', args, { encoding: 'utf8', timeout: 180000, maxBuffer: 2 * 1024 * 1024 });
const name = 'cma-omarchy-contract-' + randomUUID();
const result = { revision, result: 'fail', cleanup: false };
const probe = resolve('/tmp', name + '.mjs');
prepareProbe(probe, '.github/scripts', Boolean(expected.CODEX_FORK_CODE_MODE_SHA256_AMD64));
mkdirSync(resolve(destination, '..'), { recursive: true });
try {
  const image = JSON.parse(docker(['image', 'inspect', 'cma-runtime-omarchy:ci']))[0];
  validateImage(image, revision);
  result.image = image.Id;
  docker(['volume', 'create', '--label', `dev.chrono.cma.omarchy-test=${name}`, name]);
  const mounts = ['omarchy-codex-aliases.mjs', 'runtime-github-probe.mjs', 'runtime-bootstrap-probe.mjs', 'omarchy-keyring-probe.py']
    .flatMap(file => ['--mount', `type=bind,source=${resolve('.github/scripts', file)},target=/opt/${file},readonly`]);
  mounts.push('--mount', `type=bind,source=${probe},target=/opt/runtime-image-probe.mjs,readonly`);
  mounts.push('--mount', `type=bind,source=${resolve('.github/scripts/omarchy-chromium-config-probe.py')},target=/opt/omarchy-chromium-config-probe.py,readonly`);
  docker(['create', '--name', name, '--network', 'none', '--read-only', '--cap-drop', 'ALL',
    '--security-opt', 'no-new-privileges=true', '--pids-limit', '128', '--memory', '1g', '--cpus', '2',
    '--tmpfs', '/tmp:rw,nosuid,nodev,size=536870912,mode=1777',
    '--tmpfs', '/var/lib/cma-runtime:rw,nosuid,nodev,size=67108864,uid=10001,gid=10001,mode=700',
    '--mount', `type=volume,source=${name},target=/home/cma`,
    ...(expected.CODEX_FORK_CODE_MODE_SHA256_AMD64 ? ['--env', 'CMA_RUNTIME_NATIVE_PACKAGE_PROOF=1'] : []),
    '--env', `CHRONO_SANDBOX_ID=${name}`,
    '--env', 'CHRONO_SANDBOX_REACHES=chrono-llm-public,ornn-api',
    '--env', 'CHRONO_PLATFORM_BASE_URL=http://127.0.0.1:9192', ...mounts, image.Id]);
  docker(['start', name]);
  const legacy = JSON.parse(docker(['exec', name, 'node', '/opt/runtime-image-probe.mjs']));
  docker(['exec', name, '/usr/bin/python', '-I', '/opt/omarchy-keyring-probe.py']);
  const systemRing = docker(['exec', '--user', '0', name, '/usr/local/bin/pacman-conf', 'GPGDir']).trim();
  assert.equal(resolve(systemRing), '/etc/pacman.d/gnupg');
  const bootstrap = JSON.parse(docker(['exec', name, 'node', '/opt/runtime-bootstrap-probe.mjs']));
  docker(['exec', name, '/usr/bin/python', '-I', '/opt/omarchy-chromium-config-probe.py']);
  result.proof = combineProofs(legacy, bootstrap, expected);
  result.result = 'pass';
} catch (error) {
  result.error = String(error.message).slice(0, 2000);
} finally {
  try { docker(['rm', '--force', name]); docker(['volume', 'rm', name]); result.cleanup = true; } catch {}
  writeFileSync(destination, JSON.stringify(result, null, 2));
}
console.log(JSON.stringify(result));
assert.equal(result.result, 'pass');
assert(result.cleanup);
