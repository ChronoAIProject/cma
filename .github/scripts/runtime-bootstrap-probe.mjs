import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { existsSync, lstatSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { createConnection, createServer } from 'node:net';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { assertPublicHomeSeed, installRequest, sha256 } from './runtime-image-probe.mjs';

const executable = '/usr/local/bin/layer4-runtime';
export const bootstrapChecks = [
  'bootstrap-native-initialization', 'bootstrap-waiting-no-listener',
  'bootstrap-wrong-pod-refused', 'bootstrap-stdin-assignment',
  'bootstrap-exact-replay', 'bootstrap-conflicting-config-refused',
  'bootstrap-http-auth-status', 'bootstrap-agent-resource-capability', 'bootstrap-install-once',
  'bootstrap-fresh-process-waiting', 'bootstrap-old-assignment-refused',
  'bootstrap-retained-installation', 'bootstrap-retained-install-replay',
];
export const bootstrapStages = [...bootstrapChecks, 'bootstrap-process-stop', 'bootstrap-complete'];
const fields = (value, keys) => assert.deepEqual(Object.keys(value).sort(), [...keys].sort());

export function assignmentDigest(request) {
  const canonical = { version: request.version, pod_uid: request.pod_uid,
    environment: Object.fromEntries(Object.entries(request.environment).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)) };
  return sha256(Buffer.concat([Buffer.from('cma-provider-effect-v1\0'), Buffer.from(JSON.stringify(canonical))]));
}

export function assertAssignmentReceipt(receipt, request) {
  assert.deepEqual(receipt, { version: 1, pod_uid: request.pod_uid, request_sha256: assignmentDigest(request) });
}

// A pool process starts without any tenant identity or credential in its environment.
export function waitingEnvironment(podUid, socket, temporary) {
  return { PATH: '/usr/local/bin:/usr/bin:/bin', LANG: 'C.UTF-8',
    CHRONO_RUNTIME_POD_UID: podUid, CHRONO_RUNTIME_BOOTSTRAP_SOCKET: socket, TMPDIR: temporary };
}

export function assignRuntime(request, environment, accepted = true, command = spawnSync) {
  const result = command(executable, ['--assign-runtime'], {
    env: environment, input: JSON.stringify(request), encoding: 'utf8',
    timeout: 15000, maxBuffer: 65536, stdio: ['pipe', 'pipe', 'pipe'],
  });
  assert.ok(!result.error && !result.signal, 'assignment command did not finish');
  for (const output of [result.stdout, result.stderr]) {
    assert.equal(typeof output, 'string');
    assert.ok(!output.includes(request.environment.CHRONO_RUNTIME_CONTROL_TOKEN));
    assert.ok(!output.includes('CHRONO_RUNTIME_CONTROL_TOKEN'));
  }
  assert.equal(result.status, accepted ? 0 : 2);
  if (!accepted) {
    assert.equal(result.stdout, '');
    assert.ok(result.stderr.includes('runtime assignment was rejected'), 'assignment was not refused by the running server');
    return;
  }
  const receipt = JSON.parse(result.stdout);
  assertAssignmentReceipt(receipt, request);
  return receipt;
}

export function statusRequest(request) {
  return { t: 'runtime_status', sandbox_id: request.environment.CHRONO_SANDBOX_ID,
    generation: Number(request.environment.CHRONO_RUNTIME_GENERATION),
    home_identity: request.environment.CHRONO_RUNTIME_HOME_IDENTITY };
}

export function assertRuntimeStatus(status, request, installed, previousBoot) {
  assert.match(status.boot_id, /^[0-9a-f]{32}$/);
  if (previousBoot) assert.notEqual(status.boot_id, previousBoot);
  assert.deepEqual(status, { ...statusRequest(request), boot_id: status.boot_id,
    installation_ready: installed, credential_ready: false, restore_pending: false, agent_processes: 0, agent_skill_install: true, agent_model_catalog: true });
  return status.boot_id;
}

export function assertInstallReceipt(receipt, request) {
  fields(receipt, ['t', 'operation_id', 'request_sha256', 'commands']);
  assert.equal(receipt.t, 'install');
  assert.equal(receipt.operation_id, request.operation_id);
  assert.equal(receipt.request_sha256, request.request_sha256);
  assert.equal(receipt.commands.length, request.commands.length);
  for (const [index, command] of receipt.commands.entries()) {
    fields(command, ['index', 'exit_code', 'stdout_sha256', 'stderr_sha256']);
    assert.equal(command.index, index);
    assert.equal(command.exit_code, 0);
    assert.match(command.stdout_sha256, /^[0-9a-f]{64}$/);
    assert.match(command.stderr_sha256, /^[0-9a-f]{64}$/);
  }
}

export function assertInstallReplay(receipt, original, marker) {
  assert.deepEqual(receipt, original);
  assert.equal(marker, 'installed\n');
}

export function validateBootstrapProof(proof, runtimeDigest) {
  fields(proof, ['checks', 'runtime']);
  assert.deepEqual(proof.checks, bootstrapChecks);
  assert.match(proof.runtime, /^[0-9a-f]{64}$/);
  assert.equal(proof.runtime, runtimeDigest);
}

async function availablePort() {
  const server = createServer();
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const port = server.address().port;
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  return port;
}

async function connects(options) {
  return new Promise((resolve, reject) => {
    const socket = createConnection(options);
    socket.setTimeout(1000);
    socket.once('connect', () => { socket.destroy(); resolve(true); });
    socket.once('error', (error) => {
      if (['ECONNREFUSED', 'ENOENT'].includes(error.code)) resolve(false);
      else reject(Error('socket connection failed'));
    });
    socket.once('timeout', () => { socket.destroy(); reject(Error('socket connection timed out')); });
  });
}

function startRuntime(environment) {
  const child = spawn(executable, ['--wait-for-assignment'], { env: environment, stdio: 'ignore' });
  const running = { child, result: null };
  running.exited = new Promise((resolve) => {
    child.once('error', () => { running.result = { error: true }; resolve(running.result); });
    child.once('exit', (code, signal) => { running.result = { code, signal }; resolve(running.result); });
  });
  return running;
}

function assertAlive(running) { assert.equal(running.result, null, 'runtime process exited before proof completed'); }

async function waitFor(running, predicate) {
  const deadline = performance.now() + 10000;
  do {
    assertAlive(running);
    if (await predicate()) { assertAlive(running); return; }
    await delay(50);
  } while (performance.now() < deadline);
  assert.fail('runtime did not become ready before the deadline');
}

async function waitForAssignment(running, socket, port) {
  await waitFor(running, async () => {
    if (!existsSync(socket) || !(await connects({ path: socket }))) return false;
    const metadata = lstatSync(socket);
    assert.ok(metadata.isSocket());
    assert.equal(metadata.uid, 10001);
    assert.equal(metadata.mode & 0o777, 0o600);
    return true;
  });
  assert.equal(await connects({ host: '127.0.0.1', port }), false, 'waiting runtime exposed a TCP listener');
}

async function stopRuntime(running, graceful = false) {
  if (!running) return;
  if (!running.result) {
    running.child.kill('SIGTERM');
    for (let attempt = 0; !running.result && attempt < 100; attempt += 1) await delay(50);
    if (!running.result) running.child.kill('SIGKILL');
    await running.exited;
  }
  if (graceful) assert.deepEqual(running.result, { code: 0, signal: null });
}

async function requestHttp(url, body, token) {
  return fetch(url, { method: body ? 'POST' : 'GET', redirect: 'error', signal: AbortSignal.timeout(5000),
    headers: { ...(body ? { 'content-type': 'application/json' } : {}), ...(token ? { 'x-cma-runtime-token': token } : {}) },
    ...(body ? { body: JSON.stringify(body) } : {}) });
}

async function control(base, body, token) {
  const response = await requestHttp(`${base}/runtime/control`, body, token);
  assert.equal(response.status, 200);
  const bytes = await response.text();
  assert.ok(bytes.length < 65536 && !bytes.includes(token));
  return JSON.parse(bytes);
}

async function authenticatedStatus(running, base, request, installed, previousBoot, oldToken) {
  await waitFor(running, async () => {
    try {
      const response = await requestHttp(`${base}/ready`);
      await response.arrayBuffer();
      return response.status === 200;
    }
    catch { return false; }
  });
  assert.deepEqual(await (await requestHttp(`${base}/ready`)).json(), {
    status: 'ready', phase: installed ? 'installed' : 'provisioned',
  });
  const frame = statusRequest(request);
  for (const token of [undefined, 'wrong', ...(oldToken ? [oldToken] : [])]) {
    const refused = await requestHttp(`${base}/runtime/control`, frame, token);
    assert.equal(refused.status, 401);
    await refused.arrayBuffer();
  }
  return assertRuntimeStatus(await control(base, frame, request.environment.CHRONO_RUNTIME_CONTROL_TOKEN), request, installed, previousBoot);
}

export async function bootstrapProbe(onStage = () => {}) {
  const completed = [];
  const complete = (check) => { assert.equal(check, bootstrapChecks[completed.length]); completed.push(check); };
  let volume, socketRoot, running;
  const temporaries = [];
  try {
    onStage('bootstrap-native-initialization');
    assert.equal(process.getuid(), 10001);
    const runtime = sha256(readFileSync(executable));
    volume = mkdtempSync('/home/cma/ci-bootstrap-');
    socketRoot = mkdtempSync('/var/lib/cma-runtime/ci-bootstrap-');
    const socket = join(socketRoot, 'assignment.sock');
    const port = await availablePort();
    const base = `http://127.0.0.1:${port}`;
    const freshEnvironment = (podUid) => {
      const temporary = mkdtempSync('/tmp/cma-bootstrap-');
      temporaries.push(temporary);
      assert.deepEqual(readdirSync(temporary), []);
      const environment = waitingEnvironment(podUid, socket, temporary);
      // Probe the setting inherited by a real process in this image.
      const restart = process.env.CHRONO_RUNTIME_AGENT_PROCESS_RESTART_ENABLED;
      assert.equal(restart, 'true');
      environment.CHRONO_RUNTIME_AGENT_PROCESS_RESTART_ENABLED = restart;
      return environment;
    };
    const firstEnvironment = freshEnvironment(`ci-${randomUUID()}`);
    const initialize = spawnSync(executable, ['--initialize-home-tree', volume], {
      env: firstEnvironment, encoding: 'utf8', timeout: 30000, stdio: ['ignore', 'pipe', 'pipe'],
    });
    assert.ok(!initialize.error && !initialize.signal);
    assert.equal(initialize.status, 0);
    for (const name of ['home', 'home/cma', 'home/.codex', 'state', 'transfer', '.cma-home-layout-v2']) {
      const metadata = lstatSync(join(volume, name));
      assert.ok(metadata.isDirectory() && !metadata.isSymbolicLink());
      assert.equal(metadata.uid, 10001);
    }
    assertPublicHomeSeed(join(volume, 'home', 'cma'));
    assert.deepEqual(readdirSync(join(volume, 'state')), []);
    assert.deepEqual(readdirSync(join(volume, 'transfer')), []);
    const request = { version: 1, pod_uid: firstEnvironment.CHRONO_RUNTIME_POD_UID, environment: {
      CHRONO_SANDBOX_ID: `ci-bootstrap-${randomUUID()}`, CHRONO_RUNTIME_GENERATION: '1',
      CHRONO_RUNTIME_HOME_IDENTITY: randomBytes(32).toString('base64url'),
      CHRONO_RUNTIME_CONTROL_TOKEN: randomBytes(32).toString('hex'),
      CHRONO_RUNTIME_HOME: join(volume, 'home', 'cma'), CHRONO_RUNTIME_STATE_ROOT: join(volume, 'state'),
      CHRONO_RUNTIME_BIND: `127.0.0.1:${port}`, TMPDIR: join(volume, 'transfer'),
    } };
    complete('bootstrap-native-initialization');
    onStage('bootstrap-waiting-no-listener');
    running = startRuntime(firstEnvironment);
    await waitForAssignment(running, socket, port);
    complete('bootstrap-waiting-no-listener');
    onStage('bootstrap-wrong-pod-refused');
    assignRuntime({ ...request, pod_uid: 'wrong-pod' }, firstEnvironment, false);
    await waitForAssignment(running, socket, port);
    complete('bootstrap-wrong-pod-refused');
    onStage('bootstrap-stdin-assignment');
    const assigned = assignRuntime(request, firstEnvironment);
    complete('bootstrap-stdin-assignment');
    onStage('bootstrap-exact-replay');
    assert.deepEqual(assignRuntime(request, firstEnvironment), assigned);
    complete('bootstrap-exact-replay');
    onStage('bootstrap-conflicting-config-refused');
    assignRuntime({ ...request, environment: { ...request.environment, CHRONO_SANDBOX_ID: 'conflicting-sandbox' } }, firstEnvironment, false);
    assert.deepEqual(assignRuntime(request, firstEnvironment), assigned);
    complete('bootstrap-conflicting-config-refused');
    onStage('bootstrap-http-auth-status');
    const firstBoot = await authenticatedStatus(running, base, request, false);
    complete('bootstrap-http-auth-status');
    onStage('bootstrap-agent-resource-capability');
    const extendedResponse = await fetch(`${base}/runtime/control`, {
      method: 'POST', headers: { 'content-type': 'application/json',
        accept: 'application/vnd.chrono.cma.agent-resources-status+json, ' +
          'application/vnd.chrono.cma.agent-process-restart-status.v1+json',
        'x-cma-runtime-token': request.environment.CHRONO_RUNTIME_CONTROL_TOKEN },
      body: JSON.stringify(statusRequest(request)), signal: AbortSignal.timeout(15000), redirect: 'error',
    });
    assert.equal(extendedResponse.status, 200);
    const extended = await extendedResponse.json();
    assert.equal(extended.agent_resource_preparation, true);
    assert.equal(extended.agent_process_restart, true);
    delete extended.agent_resource_preparation;
    delete extended.agent_process_restart;
    assert.equal(assertRuntimeStatus(extended, request, false), firstBoot);
    // The opt-in does not change the response for subsequent old readers.
    assert.equal(await authenticatedStatus(running, base, request, false), firstBoot);
    complete('bootstrap-agent-resource-capability');
    onStage('bootstrap-install-once');
    const install = installRequest(['printf \'installed\\n\' >> "$HOME/ci-bootstrap-install-count"']);
    const installed = await control(base, install, request.environment.CHRONO_RUNTIME_CONTROL_TOKEN);
    assertInstallReceipt(installed, install);
    const marker = () => readFileSync(join(volume, 'home', 'cma', 'ci-bootstrap-install-count'), 'utf8');
    assertInstallReplay(await control(base, install, request.environment.CHRONO_RUNTIME_CONTROL_TOKEN), installed, marker());
    assert.equal(await authenticatedStatus(running, base, request, true), firstBoot);
    for (const temporary of temporaries) assert.equal(existsSync(join(temporary, 'cma-checkpoints')), false);
    complete('bootstrap-install-once');
    onStage('bootstrap-process-stop');
    await stopRuntime(running, true);
    running = null;
    onStage('bootstrap-fresh-process-waiting');
    const secondEnvironment = freshEnvironment(`ci-${randomUUID()}`);
    assert.notEqual(secondEnvironment.TMPDIR, firstEnvironment.TMPDIR);
    running = startRuntime(secondEnvironment);
    await waitForAssignment(running, socket, port);
    complete('bootstrap-fresh-process-waiting');
    onStage('bootstrap-old-assignment-refused');
    assignRuntime(request, secondEnvironment, false);
    await waitForAssignment(running, socket, port);
    complete('bootstrap-old-assignment-refused');
    onStage('bootstrap-retained-installation');
    const resumed = { ...request, pod_uid: secondEnvironment.CHRONO_RUNTIME_POD_UID, environment: {
      ...request.environment, CHRONO_RUNTIME_GENERATION: '2', CHRONO_RUNTIME_CONTROL_TOKEN: randomBytes(32).toString('hex'),
    } };
    assignRuntime(resumed, secondEnvironment);
    await authenticatedStatus(running, base, resumed, true, firstBoot, request.environment.CHRONO_RUNTIME_CONTROL_TOKEN);
    complete('bootstrap-retained-installation');
    onStage('bootstrap-retained-install-replay');
    assertInstallReplay(await control(base, install, resumed.environment.CHRONO_RUNTIME_CONTROL_TOKEN), installed, marker());
    for (const temporary of temporaries) assert.equal(existsSync(join(temporary, 'cma-checkpoints')), false);
    complete('bootstrap-retained-install-replay');
    onStage('bootstrap-process-stop');
    await stopRuntime(running, true);
    running = null;
    onStage('bootstrap-complete');
    const proof = { checks: completed, runtime };
    validateBootstrapProof(proof, runtime);
    return proof;
  } finally {
    await stopRuntime(running);
    for (const directory of [volume, socketRoot, ...temporaries].filter(Boolean)) rmSync(directory, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  let stage = bootstrapChecks[0];
  try { process.stdout.write(`${JSON.stringify(await bootstrapProbe((value) => { stage = value; }))}\n`); }
  catch {
    process.stderr.write(`runtime bootstrap probe failed at ${stage}; raw runtime data withheld\n`);
    process.exitCode = 1;
  }
}
