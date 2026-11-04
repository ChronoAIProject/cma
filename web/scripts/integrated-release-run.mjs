import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { openSync, existsSync } from 'node:fs';
import { mkdir, readFile, writeFile, copyFile, chmod, symlink } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { retainBinary, verifySources } from './integrated-release-seal.mjs';

const workspace = resolve(import.meta.dirname, '../..');
const root = process.env.CMA_INTEGRATED_ATTEMPT;
assert(root?.startsWith('/Users/liuyitian/code/cma-ui-worktrees/ticket19-proof/attempt-'));
assert(!existsSync(root), 'Every attempt needs a new evidence directory');
const identifier = 'cma-ui-ticket19-' + root.split('/').at(-1);
const state = '/tmp/' + identifier + '-state';
const runner = '/Users/liuyitian/code/cma-ui-worktrees/runtime-proof/runsc-probe/runsc';
const image =
  'cma-ui-runtime-proof@sha256:4ebc67d07fef6c471b433df01723b277a2db51a432df47a9523a5f8e68d8ca37';
const environment = {
  ...process.env,
  PATH: '/Users/liuyitian/.cargo/bin:' + process.env.PATH,
  CARGO_TARGET_DIR: '/Users/liuyitian/code/cma-ui-worktrees/ticket19-cargo-target',
  CARGO_BUILD_JOBS: '2',
  RUST_MIN_STACK: '33554432',
  CHRONO_TEST_MONGODB_URI: 'mongodb://127.0.0.1:27027/?directConnection=true&replicaSet=ticket11',
  CMA_DESKTOP_RUNTIME_IMAGE: image,
  CMA_INTEGRATED_RELEASE_PROOF: '1',
  CMA_DESKTOP_RELAY_DIRECTORY: join(root, 'relay'),
  CMA_BROWSER_PREVIEW_URL: 'http://127.0.0.1:4319',
  CMA_E2E_BROWSER_PATH: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  ...(process.env.CMA_INTEGRATED_WEB_BUILD === '1'
    ? { CMA_INTEGRATED_WEB_DIST: join(root, 'dist') }
    : {}),
};
const children = [];
const executions = [];
function launch(command, args, file, cwd = workspace) {
  const log = openSync(join(root, file), 'w');
  const record = { command, args, cwd, log: file, started: new Date().toISOString() };
  executions.push(record);
  const child = spawn(command, args, {
    cwd,
    env: environment,
    detached: true,
    stdio: ['ignore', log, log],
  });
  child.result = new Promise((resolveResult, reject) => {
    child.on('error', reject);
    child.on('exit', (code, signal) => {
      Object.assign(record, { code, signal, finished: new Date().toISOString() });
      resolveResult(code);
    });
  });
  children.push(child);
  return child;
}
async function waitFile(name, child) {
  for (let attempt = 0; attempt < 1200; attempt += 1) {
    if (existsSync(join(root, 'relay', name)))
      return JSON.parse(await readFile(join(root, 'relay', name), 'utf8'));
    assert.equal(child.exitCode, null, name + ' process exited');
    await delay(100);
  }
  throw new Error(name + ' readiness timeout');
}
const git = (args) => execFileSync('git', args, { cwd: workspace, maxBuffer: 256 * 1024 * 1024 });
await mkdir(join(root, 'relay'), { recursive: true });
await chmod(join(root, 'relay'), 0o777);
await mkdir(join(root, 'source'));
execFileSync('tar', ['-xf', '-', '-C', join(root, 'source')], {
  cwd: workspace,
  input: git(['archive', 'HEAD']),
  maxBuffer: 128 * 1024 * 1024,
});
const changed = [
  ...new Set(
    git(['ls-files', '-m', '-o', '--exclude-standard', '-z'])
      .toString()
      .split('\0')
      .filter(Boolean),
  ),
];
for (const path of changed) {
  await mkdir(dirname(join(root, 'source', path)), { recursive: true });
  await copyFile(join(workspace, path), join(root, 'source', path));
}
const paths = [...new Set([...git(['ls-files', '-z']).toString().split('\0'), ...changed])]
  .filter(Boolean)
  .sort();
const hashes = [];
for (const path of paths)
  hashes.push({
    path,
    sha256: createHash('sha256')
      .update(await readFile(join(root, 'source', path)))
      .digest('hex'),
  });
await writeFile(
  join(root, 'source-manifest.json'),
  JSON.stringify(
    { head: git(['rev-parse', 'HEAD']).toString().trim(), dirty: changed, hashes },
    null,
    2,
  ),
);
await writeFile(join(root, 'source.patch'), git(['diff', '--binary', 'HEAD']));
for (const path of ['web/node_modules', 'sdk/typescript/node_modules'])
  await symlink(join(workspace, path), join(root, 'source', path));
const config = {
  ociVersion: '1.0.0',
  process: {
    user: { uid: 10001, gid: 10001 },
    args: ['node', '/immutable/api/crates/layer4-runtime/src/browser/integrated-chain.cjs'],
    env: [
      'PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
      'HOME=/tmp',
      'CMA_CODEX_TEST_BINARY=/usr/local/bin/codex',
      'CMA_ACP_TEST_BRIDGE=/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js',
      'CMA_BROWSER_BINARY=/usr/bin/chromium',
      'CMA_RUNTIME_TEST_BINARY=/usr/local/bin/layer4-runtime',
      'CMA_DESKTOP_RELAY_DIRECTORY=/proof-relay',
      'CMA_DESKTOP_RUNTIME_IMAGE=' + image,
      'CMA_INTEGRATED_REPOSITORY=' + (environment.CMA_INTEGRATED_REPOSITORY ?? '0'),
      'CMA_INTEGRATED_CONTROL=' + (environment.CMA_INTEGRATED_CONTROL ?? '0'),
      'CMA_INTEGRATED_EXTENDED=' + (environment.CMA_INTEGRATED_EXTENDED ?? '0'),
    ],
    cwd: '/tmp',
    capabilities: { bounding: [], effective: [], inheritable: [], permitted: [], ambient: [] },
    noNewPrivileges: true,
    rlimits: [{ type: 'RLIMIT_NOFILE', hard: 1024, soft: 1024 }],
  },
  root: { path: '/tmp/cma-ui-ticket17-control-23d92fdcc/rootfs', readonly: true },
  hostname: 'runsc',
  mounts: [
    { destination: '/proc', type: 'proc', source: 'proc' },
    { destination: '/dev', type: 'tmpfs', source: 'tmpfs' },
    {
      destination: '/tmp',
      type: 'tmpfs',
      source: 'tmpfs',
      options: ['nosuid', 'nodev', 'mode=1777', 'size=536870912'],
    },
    {
      destination: '/sys',
      type: 'sysfs',
      source: 'sysfs',
      options: ['nosuid', 'noexec', 'nodev', 'ro'],
    },
    {
      destination: '/immutable',
      type: 'bind',
      source: join(root, 'source'),
      options: ['rbind', 'ro', 'nosuid', 'nodev'],
    },
    {
      destination: '/proof-relay',
      type: 'bind',
      source: join(root, 'relay'),
      options: ['rbind', 'rw', 'nosuid', 'nodev', 'noexec'],
    },
    {
      destination: '/home',
      type: 'tmpfs',
      source: 'tmpfs',
      options: ['nosuid', 'nodev', 'mode=0700', 'uid=10001', 'gid=10001', 'size=1073741824'],
    },
  ],
  linux: {
    cgroupsPath: '/' + identifier,
    resources: {
      memory: { limit: 2147483648 },
      cpu: { quota: 100000, period: 100000 },
      pids: { limit: 512 },
    },
    namespaces: ['pid', 'network', 'ipc', 'uts', 'mount'].map((type) => ({ type })),
  },
};
await writeFile(join(root, 'config.json'), JSON.stringify(config, null, 2));
await writeFile(
  join(root, 'runtime-provenance.json'),
  JSON.stringify(
    {
      image,
      state,
      identifier,
      runtimeSha256: '6283b369b5bedcd8c8d742786f061cce45fbcbb3ead18a2241db473286f624e3',
      runtimeProductionSource: '556aef8ba35bbcba8cb60e1eb122cde36eda7f7f',
      bakedProofSource: '23d92fdcc6fdd430e5e8ed5a1f6c42a8b27ee294',
      rootfs: config.root.path,
      driver: 'source/api/crates/layer4-runtime/src/browser/integrated-chain.cjs',
    },
    null,
    2,
  ),
);
let passed = false;
let failure;
try {
  const compile = launch(
    'cargo',
    [
      '+1.96.1',
      'test',
      '--offline',
      '--locked',
      '--manifest-path',
      'api/Cargo.toml',
      '-p',
      'cma',
      'initial_task_public_default_environment_delivers_exact_task_after_preparation',
      '--no-run',
    ],
    'compile.log',
  );
  assert.equal(
    await compile.result,
    0,
    'Compile the named host test before starting the native runtime deadline',
  );
  await verifySources(root, workspace, 'compiled');
  const native = launch(
    'colima',
    [
      'ssh',
      '--profile',
      'cma-ui-proof',
      '--',
      'sudo',
      'python3',
      join(root, 'source/web/scripts/integrated-release-sample.py'),
      root,
      identifier,
      state,
    ],
    'native-run.log',
  );
  await waitFile('native-ready.json', native);
  const relay = launch(
    'node',
    ['web/scripts/integrated-native-relay.mjs'],
    'relay.log',
    join(root, 'source'),
  );
  const host = await waitFile('host-ready.json', relay);
  environment.CMA_INTEGRATED_MODEL_ORIGIN = host.origin + '/proof-model';
  const api = launch(
    'cargo',
    [
      '+1.96.1',
      'test',
      '--offline',
      '--locked',
      '--manifest-path',
      'api/Cargo.toml',
      '-p',
      'cma',
      'initial_task_public_default_environment_delivers_exact_task_after_preparation',
      '--',
      '--ignored',
      '--nocapture',
    ],
    'public-run.log',
  );
  await waitFile('public-ready.json', api);
  await verifySources(root, workspace, 'compiled');
  await retainBinary(root);
  if (environment.CMA_INTEGRATED_WEB_DIST) {
    const build = launch(
      'node',
      ['node_modules/vite/bin/vite.js', 'build', '--outDir', environment.CMA_INTEGRATED_WEB_DIST],
      'web-build.log',
      join(root, 'source/web'),
    );
    assert.equal(await build.result, 0, 'Production Web build from the verified source archive');
  }
  const preview = launch(
    'node',
    ['web/scripts/integrated-release-server.mjs'],
    'preview.log',
    join(root, 'source'),
  );
  for (let attempt = 0; ; attempt += 1) {
    try {
      assert((await fetch(environment.CMA_BROWSER_PREVIEW_URL)).ok);
      break;
    } catch {
      assert.equal(preview.exitCode, null);
      assert(attempt < 200);
      await delay(100);
    }
  }
  const web = launch(
    'node',
    [
      'node_modules/@playwright/test/cli.js',
      'test',
      '--config',
      'e2e/integrated-release.config.ts',
    ],
    'web-run.log',
    join(root, 'source/web'),
  );
  const webResult = await web.result;
  assert(
    existsSync(join(root, 'relay/web-result.json')),
    'The real story must reach its final assertions',
  );
  assert.equal(await api.result, 0, 'Named Mongo/public host test');
  assert.equal(await native.result, 0, 'Native runtime proof');
  assert.match(
    await readFile(join(root, 'public-run.log'), 'utf8'),
    /test result: ok\. 1 passed; 0 failed;/,
  );
  assert.equal(
    webResult,
    0,
    'Integrated Web story including strict unexpected-response assertions',
  );
  passed = true;
} catch (error) {
  failure = String(error);
  console.error(error);
  process.exitCode = 1;
} finally {
  if (!passed)
    await new Promise((resolveResult) =>
      spawn(
        'colima',
        [
          'ssh',
          '--profile',
          'cma-ui-proof',
          '--',
          'sudo',
          runner,
          '--root=' + state,
          'kill',
          '--all',
          identifier,
          'KILL',
        ],
        { cwd: workspace, stdio: 'ignore' },
      ).on('exit', resolveResult),
    );
  if (!passed && children[0]) await Promise.race([children[0].result, delay(5000)]);
  for (const child of children) if (child.exitCode === null) process.kill(-child.pid, 'SIGTERM');
  await Promise.allSettled(children.map((child) => child.result));
  try {
    await verifySources(root, join(root, 'source'), 'finished', true);
  } catch (error) {
    passed = false;
    failure = String(error);
    process.exitCode = 1;
  }
  await writeFile(
    join(root, 'result.json'),
    JSON.stringify({ passed, failure, executions, completed: new Date().toISOString() }, null, 2),
  );
}
