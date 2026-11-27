import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { desktopSeccomp } from './desktop-seccomp.mjs';
import { runtimeToolVersions } from './runtime-image-probe.mjs';

const [imageRef, revision, destination] = process.argv.slice(2);
assert(imageRef && /^[0-9a-f]{40}$/.test(revision) && destination);
const docker = args => execFileSync('docker', args, { encoding: 'utf8', timeout: 90000, maxBuffer: 2 * 1024 * 1024 });
const name = 'cma-desktop-test-' + randomUUID();
const token = randomBytes(32).toString('hex');
const temporary = mkdtempSync(resolve(tmpdir(), 'cma-desktop-seccomp-'));
let startupDiagnostics = '';
const result = { schema: 'cma-desktop-evidence-v1', revision, image: null, result: 'fail', cleanup: false };
async function waitReady() {
  for (let i = 0; i < 45; i++) {
    const check = docker(['exec', name, 'python3', '-c',
      'import socket; s=socket.socket(); s.settimeout(1); print(s.connect_ex(("127.0.0.1",9090)))']).trim();
    if (check === '0') return;
    if (i % 5 === 0) {
      startupDiagnostics = docker(['exec', name, 'sh', '-c',
        'tail -c 4000 /tmp/cma-chromium.log /tmp/cma-xvfb.log /tmp/cma-xfce.log /tmp/cma-websockify.log /tmp/cma-x11vnc.log 2>/dev/null || true']);
    }
    await delay(1000);
  }
  assert.fail('desktop runtime did not become ready');
}
try {
  const inspected = JSON.parse(docker(['image', 'inspect', imageRef]))[0];
  assert.equal(inspected.Config.Labels['org.opencontainers.image.revision'], revision);
  assert.equal(inspected.Config.Labels['dev.chrono.cma.runtime-desktop'], '1');
  assert.equal(inspected.Config.User, '10001:10001');
  result.image = inspected.Id;
  const seccomp = resolve(temporary, 'seccomp.json');
  await desktopSeccomp(seccomp);
  const mounts = [
    ['.github/scripts/desktop-browser-reopen.py', '/opt/desktop-browser-reopen.py'],
    ['.github/scripts/desktop-native-probe.cjs', '/opt/desktop-native-probe.cjs'],
    ['.github/scripts/desktop-browser-input.py', '/opt/desktop-browser-input.py'],
    ['.github/scripts/desktop-browser-page.cjs', '/opt/desktop-browser-page.cjs'],
    ['.github/scripts/desktop-vnc-probe.py', '/opt/desktop-vnc-probe.py'],
    ['.github/scripts/desktop_ownership_probe.py', '/opt/desktop_ownership_probe.py'],
    ['.github/scripts/desktop-lifecycle-probe.py', '/opt/desktop-lifecycle-probe.py'],
    ['.github/scripts/desktop-shell-probe.py', '/opt/desktop-shell-probe.py'],
    ['api/crates/layer3-adapter/src/codex', '/opt/codex-fixtures'],
  ].flatMap(([source, target]) => ['--mount', `type=bind,source=${resolve(source)},target=${target},readonly`]);
  docker(['volume', 'create', '--label', `dev.chrono.cma.desktop-test=${name}`, name]);
  docker(['create', '--name', name, '--network', 'none', '--read-only', '--cap-drop', 'ALL',
    '--security-opt', `seccomp=${seccomp}`,
    '--security-opt', 'no-new-privileges=true', '--pids-limit', '512', '--memory', '8g', '--cpus', '2',
    '--tmpfs', '/tmp:rw,nosuid,nodev,size=1073741824,mode=1777',
    '--mount', `type=volume,source=${name},target=/home/cma`,
    '--tmpfs', '/var/lib/cma-runtime:rw,nosuid,nodev,size=67108864,uid=10001,gid=10001,mode=700',
    '--env', `CHRONO_SANDBOX_ID=${name}`, '--env', 'CMA_MACHINE_INCLUDED=1',
    '--env', `CHRONO_RUNTIME_CONTROL_TOKEN=${token}`, ...mounts, inspected.Id]);
  docker(['start', name]);
  await waitReady();
  // The desktop variant inherits the default image's repository harness tools.
  result.tools = runtimeToolVersions((command, args, input = '') => {
    const { status, stdout, error } = spawnSync('docker', ['exec', '-i', name, command, ...args],
      { input, encoding: 'utf8', timeout: 30000 });
    return { status: error ? null : status, stdout: stdout ?? '' };
  });
  docker(['exec', name, 'python3', '-c',
    'import urllib.request,urllib.error;\ntry: urllib.request.urlopen("http://127.0.0.1:9090/runtime/desktop")\nexcept urllib.error.HTTPError as e: assert e.code==401\nelse: raise AssertionError("desktop was not authenticated")']);
  // No direct desktop listener may accept traffic from the Pod interface.
  docker(['exec', name, 'python3', '-c',
    'from pathlib import Path\nfor row in Path("/proc/net/tcp").read_text().splitlines()[1:]:\n a=row.split()[1]; port=int(a.split(":")[1],16)\n if port in (5900,6080): assert a.split(":")[0]=="0100007F", a']);
  result.shell = JSON.parse(docker(['exec', '--env', 'DISPLAY=:99',
    name, 'python3', '/opt/desktop-shell-probe.py']).trim());
  result.proof = JSON.parse(docker(['exec', '--env', 'DISPLAY=:99', '--env', `CMA_DESKTOP_TEST_TOKEN=${token}`,
    name, 'node', '/opt/desktop-native-probe.cjs']).trim());
  result.lifecycle = JSON.parse(docker(['exec', '--env', 'DISPLAY=:99', '--env', `CMA_DESKTOP_TEST_TOKEN=${token}`,
    name, 'python3', '/opt/desktop-lifecycle-probe.py']).trim());
  writeFileSync(destination + '.browser-closed.png', execFileSync('docker',
    ['exec', name, 'cat', '/tmp/cma-desktop-browser-closed.png'],
    { timeout: 10000, maxBuffer: 8 * 1024 * 1024 }));
  docker(['restart', '--time', '10', name]);
  await waitReady();
  result.retainedShell = JSON.parse(docker(['exec', '--env', 'DISPLAY=:99',
    '--env', 'CMA_DESKTOP_EXPECT_RETAINED=1', name, 'python3', '/opt/desktop-shell-probe.py']).trim());
  result.retained = JSON.parse(docker(['exec', '--env', 'DISPLAY=:99', '--env', `CMA_DESKTOP_TEST_TOKEN=${token}`,
    '--env', 'CMA_DESKTOP_EXPECT_RETAINED=1', name, 'node', '/opt/desktop-native-probe.cjs']).trim());
  for (const screen of ['shell', 'file-manager']) {
    writeFileSync(destination + '.' + screen + '.png', execFileSync('docker',
      ['exec', name, 'cat', '/tmp/cma-desktop-' + screen + '.png'],
      { timeout: 10000, maxBuffer: 8 * 1024 * 1024 }));
  }
  result.result = 'pass';
} catch (error) {
  result.error = error.message.slice(0, 1000).replaceAll(token, '[fixture-token]');
  process.stderr.write('Desktop acceptance failed: ' + result.error + '\n');
  process.stderr.write(startupDiagnostics.replaceAll(token, '[fixture-token]'));
  try { process.stderr.write(docker(['logs', '--tail', '40', name]).replaceAll(token, '[fixture-token]')); }
  catch { /* The container may not have been created. */ }
  try {
    const diagnostics = docker(['exec', name, 'sh', '-c', 'tail -c 4000 /tmp/cma-chromium.log /tmp/cma-xvfb.log /tmp/cma-xfce.log /tmp/cma-websockify.log /tmp/cma-x11vnc.log 2>/dev/null']);
    process.stderr.write(diagnostics.replaceAll(token, '[fixture-token]'));
  } catch { /* The container may already have stopped. */ }
  try {
    // Docker's archive endpoint cannot reliably read a container's tmpfs.
    // Read the live mount through exec without decoding the PNG as text.
    const screen = execFileSync('docker', ['exec', name, 'cat', '/tmp/cma-desktop-failed-screen.png'],
      { timeout: 10000, maxBuffer: 8 * 1024 * 1024 });
    writeFileSync(destination + '.png', screen);
  }
  catch { /* A failure before browser execution has no screenshot. */ }
} finally {
  try {
    docker(['rm', '--force', name]);
    docker(['volume', 'rm', name]);
    result.cleanup = true;
  } catch { /* Retain failure. */ }
  rmSync(temporary, { recursive: true, force: true });
  writeFileSync(destination, JSON.stringify(result, null, 2) + '\n');
}
assert.equal(result.result, 'pass');
assert(result.cleanup);
