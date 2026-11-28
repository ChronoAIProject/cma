// Real desktop and native Codex processes; model responses are offline fixtures.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { mkdirSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { desktopSeccomp } from './desktop-seccomp.mjs';

const [revision, destination, imageReference = 'cma-runtime-omarchy:ci'] = process.argv.slice(2);
assert.match(revision, /^[0-9a-f]{40}$/);
mkdirSync(destination, { recursive: true });
const name = 'cma-omarchy-test-' + randomUUID();
const token = randomBytes(32).toString('hex');
const docker = args => execFileSync('docker', args, { encoding: 'utf8', timeout: 120000, maxBuffer: 4 * 1024 * 1024 });
const result = { scope: 'isolated-docker-offline-model', revision, result: 'fail', cleanup: false };
const devices = [];
const imageTemporaryDirectory = process.env.CMA_OMARCHY_TEST_IMAGE_TMP === '1';
result.temporaryFilesystem = imageTemporaryDirectory ? 'image' : 'tmpfs';
const run = (command, extra = []) => docker(['exec', '--env', 'DISPLAY=:99',
  '--env', 'XDG_RUNTIME_DIR=/tmp/cma-desktop-runtime', ...extra, name, ...command]);
async function ready() {
  for (let i = 0; i < 90; i++) {
    if (JSON.parse(docker(['inspect', name]))[0].State.Running !== true) throw new Error('runtime exited during startup');
    const port = run(['python', '-c', 'import socket; s=socket.socket(); print(s.connect_ex(("127.0.0.1",9090)))']).trim();
    if (port === '0') return;
    await delay(1000);
  }
  throw new Error('runtime desktop startup exceeded 90 seconds');
}
function screenshot(label) {
  run(['scrot', '--overwrite', '/tmp/omarchy-proof.png']);
  writeFileSync(resolve(destination, label + '.png'), execFileSync('docker', ['exec', name, 'cat', '/tmp/omarchy-proof.png'], { maxBuffer: 8 * 1024 * 1024 }));
}
try {
  if (process.env.CMA_OMARCHY_TEST_DRM === '1') {
    result.graphicsInventory = JSON.parse(execFileSync('sudo', ['python3',
      'api/crates/layer4-runtime/src/omarchy/graphics-device.py', '--json'], { encoding: 'utf8' }));
    const device = result.graphicsInventory.find(item => item.driver === 'vkms');
    assert(device, 'No VKMS primary DRM device identified by DRM_IOCTL_VERSION');
    devices.push('--device', device.path, '--group-add', String(statSync(device.path).gid));
    result.graphicsDriver = 'vkms';
  }
  result.graphicsDevices = devices.filter(x => x.startsWith('/dev/dri/'));
  const image = JSON.parse(docker(['image', 'inspect', imageReference]))[0];
  assert.equal(image.Config.Labels['org.opencontainers.image.revision'], revision);
  assert.equal(image.Config.Labels['dev.chrono.cma.runtime-contract'], '3');
  assert.equal(image.Config.Labels['dev.chrono.cma.desktop-environment'], 'omarchy-4.0.6');
  assert.equal(image.Config.User, '10001:10001');
  result.image = image.Id;
  writeFileSync(resolve(destination, 'image.json'), JSON.stringify(image, null, 2));
  const seccomp = resolve(destination, 'seccomp.json');
  await desktopSeccomp(seccomp);
  docker(['volume', 'create', '--label', `dev.chrono.cma.omarchy-test=${name}`, name]);
  const creation = ['create', '--name', name, '--network', 'none',
    ...(imageTemporaryDirectory ? [] : ['--read-only']), '--cap-drop', 'ALL',
    '--security-opt', `seccomp=${seccomp}`, '--security-opt', 'no-new-privileges=true',
    '--pids-limit', '768', '--memory', '8g', '--cpus', '2',
    ...(imageTemporaryDirectory ? [] : ['--tmpfs', '/tmp:rw,nosuid,nodev,size=1073741824,mode=1777']),
    '--mount', `type=volume,source=${name},target=/home/cma`,
    '--tmpfs', '/var/lib/cma-runtime:rw,nosuid,nodev,size=67108864,uid=10001,gid=10001,mode=700',
    '--env', `CHRONO_SANDBOX_ID=${name}`, '--env', 'CMA_MACHINE_INCLUDED=1',
    '--env', `CHRONO_RUNTIME_CONTROL_TOKEN=${token}`,
    ...devices,
    '--mount', `type=bind,source=${resolve('.github/scripts')},target=/opt/probes,readonly`,
    '--mount', `type=bind,source=${resolve('api/crates/layer3-adapter/src/codex')},target=/opt/codex-fixtures,readonly`, image.Id];
  docker(creation);
  docker(['start', name]);
  await ready();
  if (imageTemporaryDirectory) {
    assert.equal(run(['stat', '-c', '%u:%g:%a', '/tmp/.X11-unix']).trim(), '0:0:1777');
    result.rootOwnedX11Directory = true;
  }
  result.environment = run(['sh', '-c', 'cat /etc/os-release; cat /usr/share/omarchy/version; cat /usr/share/cma-omarchy/hyprland-version.txt; pgrep -a quickshell']);
  writeFileSync(resolve(destination, 'packages.txt'), run(['cat', '/usr/share/cma-omarchy/packages.txt']));
  writeFileSync(resolve(destination, 'package-sha256.txt'), run(['cat', '/usr/share/cma-omarchy/package-sha256.txt']));
  screenshot('desktop-started');
  run(['python', '-c', 'import urllib.request,urllib.error\ntry: urllib.request.urlopen("http://127.0.0.1:9090/runtime/desktop")\nexcept urllib.error.HTTPError as e: assert e.code==401\nelse: raise AssertionError("desktop bypassed authentication")']);
  run(['python', '-c', 'from pathlib import Path\nfor row in Path("/proc/net/tcp").read_text().splitlines()[1:]:\n a=row.split()[1]; port=int(a.split(":")[1],16)\n if port in (5900,6080): assert a.split(":")[0]=="0100007F"']);
  result.first = JSON.parse(run(['node', '/opt/probes/omarchy-native-probe.cjs'], ['--env', `CMA_DESKTOP_TEST_TOKEN=${token}`]));
  writeFileSync(resolve(destination, 'agent-terminal.png'), execFileSync('docker', ['exec', name, 'cat', '/tmp/cma-omarchy-terminal-proof.png'], { maxBuffer: 8 * 1024 * 1024 }));
  screenshot('agent-and-user-input');
  // Close Chromium cleanly to persist the profile and prove the desktop survives.
  run(['python3', '/opt/probes/omarchy-close-browser.py']);
  result.browserClosed = true;
  run(['pgrep', '-a', 'quickshell']);
  screenshot('desktop-browser-closed');
  if (imageTemporaryDirectory) {
    // Kubernetes replaces the container's writable image filesystem while
    // retaining its home volume. Exercise that production recovery shape.
    docker(['rm', '--force', name]);
    docker(creation);
    docker(['start', name]);
    result.containerReplaced = true;
  } else {
    docker(['restart', '--time', '10', name]);
  }
  await ready();
  result.retained = JSON.parse(run(['node', '/opt/probes/omarchy-native-probe.cjs'],
    ['--env', `CMA_DESKTOP_TEST_TOKEN=${token}`, '--env', 'CMA_DESKTOP_EXPECT_RETAINED=1']));
  screenshot('retained-desktop');
  result.result = 'pass';
} catch (error) {
  result.error = String(error.message).replaceAll(token, '[fixture-token]').slice(0, 4000);
  try { screenshot('failed'); } catch {}
  try {
    const captured = spawnSync('docker', ['logs', name], { encoding: 'utf8', timeout: 10000 });
    const log = captured.stdout + captured.stderr;
    writeFileSync(resolve(destination, 'diagnostics.txt'), log.replaceAll(token, '[fixture-token]'));
  } catch {
    try { writeFileSync(resolve(destination, 'diagnostics.txt'), docker(['logs', name]).replaceAll(token, '[fixture-token]')); } catch {}
  }
} finally {
  // Preserve completed native-tool evidence even if a later input check fails.
  try {
    writeFileSync(resolve(destination, result.first ? 'retained-agent-terminal.png' : 'agent-terminal.png'), execFileSync('docker',
      ['exec', name, 'cat', '/tmp/cma-omarchy-terminal-proof.png'], { maxBuffer: 8 * 1024 * 1024 }));
  } catch {}
  // A later interaction failure leaves the compositor alive. Preserve its logs
  // too, before disposing of the test's exact container and home volume.
  try {
    const logs = run(['python3', '-c', 'from pathlib import Path\nfor p in sorted(Path("/tmp").glob("cma-*.log")):\n print("\\n" + str(p) + "\\n" + p.read_text(errors="replace")[-20000:])']);
    writeFileSync(resolve(destination, 'desktop-processes.txt'), logs.replaceAll(token, '[fixture-token]'));
  } catch {}
  try { docker(['rm', '--force', name]); docker(['volume', 'rm', name]); result.cleanup = true; } catch {}
  writeFileSync(resolve(destination, 'verification.json'), JSON.stringify(result, null, 2));
}
console.log(JSON.stringify(result));
assert.equal(result.result, 'pass');
assert(result.cleanup);
