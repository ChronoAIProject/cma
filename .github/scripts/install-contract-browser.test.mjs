import test from 'node:test';
import assert from 'node:assert/strict';
import './install-contract-browser-process.cases.mjs';
import {
  downloadAttempts,
  downloadBackoffMs,
  downloadBrowser,
  downloadHostPlatform,
  downloadTimeoutMs,
  ensureBrowser,
  missingLibraries,
  outputTail,
  probeAttempts,
  requiredPackages,
  repairDependencies,
} from './install-contract-browser.mjs';

const unsupportedOutput =
  'Failed to install browsers\nError: ERROR: Playwright does not support chromium on ubuntu26.04-x64\n';

function downloaderError(message, output, code) {
  return Object.assign(new Error(message), { output, code });
}

function fixture(overrides = {}) {
  const calls = [],
    logs = [];
  return {
    calls,
    logs,
    candidates: ['/cached/chrome', '/installed/chrome'],
    pinned: '/cached/chrome',
    exists: () => true,
    probe: async () => 'test-version',
    missing: () => [],
    mapping: () => ({ 'libnss3.so': 'libnss3', 'libasound.so.2': 'libasound2t64' }),
    download: () => {
      calls.push(['download']);
    },
    run: (...args) => {
      calls.push(args);
    },
    log: (message) => logs.push(message),
    ...overrides,
  };
}

test('usable cached browser never invokes apt or download', async () => {
  const ops = fixture({
    run: () => assert.fail('apt must not run'),
    download: () => assert.fail('download must not run'),
  });
  assert.deepEqual(await ensureBrowser(ops), {
    path: '/cached/chrome',
    version: 'test-version',
    source: 'cached',
  });
  assert.match(
    ops.logs.join('\n'),
    /Selected cached Playwright Chromium.*apt not needed \(0 calls\)/,
  );
});

test('preinstalled Chrome is used when the Playwright cache is absent', async () => {
  const ops = fixture({ exists: (path) => path === '/installed/chrome' });
  assert.equal((await ensureBrowser(ops)).path, '/installed/chrome');
  assert.deepEqual(ops.calls, []);
});

test('a usable alternative avoids apt despite missing dependencies on the first browser', async () => {
  const ops = fixture({
    probe: async (path) => {
      if (path === '/cached/chrome') throw new Error('library unavailable');
      return 'working';
    },
    missing: () => ['libnss3.so'],
  });
  assert.equal((await ensureBrowser(ops)).path, '/installed/chrome');
  assert.deepEqual(ops.calls, []);
});

test('absent browsers download the pinned browser without any apt', async () => {
  let downloaded = false;
  const ops = fixture({
    exists: () => downloaded,
    download: () => {
      downloaded = true;
    },
  });
  const result = await ensureBrowser(ops);
  assert.deepEqual([result.path, result.source], ['/cached/chrome', 'downloaded']);
  assert.equal(downloaded, true);
  assert.deepEqual(ops.calls, []);
  assert.match(
    ops.logs.at(-1),
    /^Selected downloaded Playwright Chromium \/cached\/chrome \(version test-version\): no installed browser passed.*different browser/,
  );
});

test('download failure is fatal; it does not skip tests or invoke apt', async () => {
  const ops = fixture({
    exists: () => false,
    download: () => {
      throw new Error('download timeout');
    },
  });
  await assert.rejects(ensureBrowser(ops), /download timeout/);
  assert.deepEqual(ops.calls, []);
});

test('a successful downloader that leaves no executable is refused', async () => {
  const ops = fixture({ exists: () => false });
  await assert.rejects(ensureBrowser(ops), /absent after download/);
});

test('parse only actual missing shared libraries and deduplicate package mapping', () => {
  const output =
    ' libnss3.so => not found\n libasound.so.2 => not found\n libnss3.so => not found\n libc.so.6 => /lib/libc.so.6 (0x12)\n';
  assert.deepEqual(missingLibraries(output), ['libasound.so.2', 'libnss3.so']);
  assert.deepEqual(
    requiredPackages(['libnss3.so', 'libnssutil3.so'], {
      'libnss3.so': 'libnss3',
      'libnssutil3.so': 'libnss3',
    }),
    ['libnss3'],
  );
});

test('only demonstrated missing packages are installed, followed by a real probe', async () => {
  let repaired = false,
    probes = 0;
  const ops = fixture({
    candidates: ['/cached/chrome'],
    probe: async () => {
      probes += 1;
      if (!repaired) throw new Error('missing library');
      return 'repaired';
    },
    missing: () => (repaired ? [] : ['libasound.so.2']),
  });
  ops.run = (...args) => {
    ops.calls.push(args);
    if (args[1].includes('install')) repaired = true;
  };
  assert.equal((await ensureBrowser(ops)).version, 'repaired');
  assert.equal(probes, 2);
  assert.equal(ops.calls.length, 2);
  const [command, args, options] = ops.calls[1];
  assert.equal(command, 'sudo');
  assert.deepEqual(args.slice(0, 8), [
    '-n',
    'env',
    'DEBIAN_FRONTEND=noninteractive',
    'timeout',
    '--kill-after=5s',
    '45s',
    'apt-get',
    '-o',
  ]);
  assert.deepEqual(args.slice(args.indexOf('install')), [
    'install',
    '--yes',
    '--no-install-recommends',
    'libasound2t64',
  ]);
  for (const option of [
    'Acquire::Retries=0',
    'Acquire::http::Timeout=10',
    'Acquire::https::Timeout=10',
    'DPkg::Lock::Timeout=10',
    'APT::Update::Error-Mode=any',
  ])
    assert.ok(args.includes(option));
  assert.equal(options.timeout, 55_000);
  assert.ok(!args.some((arg) => /unauthenticated|insecure|upgrade/.test(arg)));
});

test('unknown shared library fails clearly before apt', async () => {
  const ops = fixture({
    probe: async () => {
      throw new Error('missing');
    },
    missing: () => ['libunknown.so'],
  });
  await assert.rejects(ensureBrowser(ops), /no package mapping: libunknown.so/);
  assert.deepEqual(ops.calls, []);
});

test('invalid package mapping and empty repair cannot enter apt', () => {
  assert.throws(() => requiredPackages(['lib.so'], { 'lib.so': '--unsafe' }), /Invalid/);
  assert.throws(
    () =>
      repairDependencies(
        [],
        () => assert.fail(),
        () => {},
      ),
    /Refusing apt/,
  );
});

test('apt DNS outage stops after two short attempts and names required packages', async () => {
  const ops = fixture({
    probe: async () => {
      throw new Error('missing');
    },
    missing: () => ['libnss3.so'],
  });
  ops.run = (...args) => {
    ops.calls.push(args);
    throw new Error('Temporary failure resolving archive.ubuntu.com');
  };
  await assert.rejects(
    ensureBrowser(ops),
    /libnss3.*apt update failed after two bounded attempts.*Browser contract tests remain required/s,
  );
  assert.equal(ops.calls.length, 2);
  assert.ok(ops.calls.every(([, args]) => args.at(-1) === 'update'));
});

test('transient apt failure retries each operation at most twice', () => {
  const attempts = { update: 0, install: 0 };
  repairDependencies(
    ['libnss3'],
    (_cmd, args) => {
      const op = args.includes('install') ? 'install' : 'update';
      attempts[op] += 1;
      if (attempts[op] === 1) throw new Error('temporary DNS failure');
    },
    () => {},
  );
  assert.deepEqual(attempts, { update: 2, install: 2 });
});

test('persistent package install failure is fatal after two attempts', () => {
  let count = 0;
  assert.throws(
    () =>
      repairDependencies(
        ['libnss3'],
        (_cmd, args) => {
          count += 1;
          if (args.includes('install')) throw new Error('package unavailable');
        },
        () => {},
      ),
    /libnss3.*apt install failed after two bounded attempts/,
  );
  assert.equal(count, 3);
});

test('remaining missing libraries after package installation are fatal', async () => {
  const ops = fixture({
    probe: async () => {
      throw new Error('missing');
    },
    missing: () => ['libnss3.so'],
  });
  await assert.rejects(ensureBrowser(ops), /still missing after apt: libnss3.so/);
});

test('a failed launch after dependency repair never becomes success', async () => {
  let repaired = false;
  const ops = fixture({
    probe: async () => {
      throw new Error('cannot render');
    },
    missing: () => (repaired ? [] : ['libnss3.so']),
    run: () => {
      repaired = true;
    },
  });
  await assert.rejects(ensureBrowser(ops), /still cannot launch\/render after installing libnss3/);
});

test('non-library browser failure cannot trigger arbitrary apt installation', async () => {
  const ops = fixture({
    probe: async () => {
      throw new Error('protocol incompatible');
    },
  });
  await assert.rejects(
    ensureBrowser(ops),
    /Refusing unrelated apt changes.*protocol incompatible/s,
  );
  assert.deepEqual(ops.calls, [['download']]);
});

test('downloader tail keeps the last rows, one progress row per run, and redacts URL credentials', () => {
  const progress = [0, 50, 100].map((p) => `|■■■■                | ${p}% of 167.3 MiB`).join('\n');
  const output = `Downloading Chrome from https://user:token@proxy.test/cft.zip\n${progress}\r\n\nError: Download failed: server closed connection.\n`;
  assert.deepEqual(outputTail(output), [
    '  Downloading Chrome from https://***@proxy.test/cft.zip',
    '  |■■■■                | 100% of 167.3 MiB',
    '  Error: Download failed: server closed connection.',
  ]);
  const long = Array.from({ length: 50 }, (_, index) => `::error::row ${index}`).join('\n');
  const tail = outputTail(long);
  assert.equal(tail.length, 40);
  assert.equal(tail[0], '  ::error::row 10');
  assert.ok(tail.every((row) => row.startsWith('  ')));
});

function downloadFixture(outcomes) {
  const logs = [],
    waits = [],
    limits = [];
  return {
    logs,
    waits,
    limits,
    attempt: async (timeoutMs) => {
      limits.push(timeoutMs);
      const outcome = outcomes[limits.length - 1];
      if (outcome) throw outcome;
    },
    log: (message) => logs.push(message),
    wait: async (ms) => {
      waits.push(ms);
    },
  };
}

test('a transient download failure is retried after a bounded backoff, then succeeds', async () => {
  const run = downloadFixture([
    downloaderError(
      'Command failed with exit code 1: node cli.js install',
      'Error: socket hang up\n',
    ),
  ]);
  await downloadBrowser(run.attempt, run.log, run.wait);
  assert.deepEqual(run.limits, [downloadTimeoutMs, downloadTimeoutMs]);
  assert.deepEqual(run.waits, [downloadBackoffMs[0]]);
  assert.match(
    run.logs[0],
    /^browser download attempt 1\/3; locked Playwright Chromium, 60s limit$/,
  );
  assert.match(run.logs[1], /^browser download attempt 1\/3 failed after \d+\.\ds: Command failed/);
  assert.match(run.logs.at(-1), /^browser download attempt 2\/3;/);
  assert.ok(!run.logs.some((row) => /downloader output/.test(row)));
});

test('downloads stop at the attempt cap and report the last output tail and guidance', async () => {
  const failure = downloaderError(
    'Command timed out after 60s: node cli.js install',
    'Downloading Chrome for Testing from https://cdn.playwright.dev/cft.zip\n|■ | 10% of 167.3 MiB\n',
    'ETIMEDOUT',
  );
  const run = downloadFixture([failure, failure, failure, undefined]);
  await assert.rejects(
    downloadBrowser(run.attempt, run.log, run.wait),
    /after 3 bounded attempts \(60s each\); likely a network or CDN failure\. Re-run the job.*cdn\.playwright\.dev.*web\/package-lock\.json.*remain required.*timed out after 60s/s,
  );
  assert.equal(downloadAttempts, 3);
  assert.equal(run.limits.length, downloadAttempts);
  assert.deepEqual(run.waits, downloadBackoffMs);
  assert.equal(run.logs.filter((row) => /^browser download attempt \d\/3;/.test(row)).length, 3);
  assert.equal(
    run.logs.at(-1),
    'Browser downloader output, last 2 lines:\n' +
      '  Downloading Chrome for Testing from https://cdn.playwright.dev/cft.zip\n' +
      '  |■ | 10% of 167.3 MiB',
  );
});

test('an unsupported host is reported as such at once, with the downloader output', async () => {
  const run = downloadFixture([
    downloaderError('Command failed with exit code 1: node cli.js install', unsupportedOutput),
  ]);
  await assert.rejects(
    downloadBrowser(run.attempt, run.log, run.wait),
    /no Chromium build for ubuntu26\.04-x64.*not a network failure, so it was not retried.*web\/package-lock\.json.*remain required/s,
  );
  assert.equal(run.limits.length, 1);
  assert.deepEqual(run.waits, []);
  assert.match(
    run.logs.join('\n'),
    /last 2 lines:\n {2}Failed to install browsers\n {2}Error: ERROR: Playwright does not support/,
  );
});

test('a non-network downloader failure says so and shows that it printed nothing', async () => {
  const failure = downloaderError('Command failed with exit code 1: node cli.js install', '');
  const run = downloadFixture([failure, failure, failure]);
  await assert.rejects(
    downloadBrowser(run.attempt, run.log, run.wait),
    /after 3 bounded attempts.*shows no network failure.*Re-run the job/s,
  );
  assert.equal(run.logs.at(-1), 'Browser downloader printed no output.');
});

test('newer Ubuntu hosts download the newest Ubuntu build the locked Playwright knows', () => {
  // Object.keys(deps) from Playwright 1.58.2 nativeDeps.
  const known = [
    'ubuntu20.04-x64',
    'ubuntu22.04-x64',
    'ubuntu24.04-x64',
    'debian11-x64',
    'debian12-x64',
    'debian13-x64',
    'ubuntu20.04-arm64',
    'ubuntu22.04-arm64',
    'ubuntu24.04-arm64',
    'debian11-arm64',
    'debian12-arm64',
    'debian13-arm64',
  ];
  assert.equal(downloadHostPlatform('ubuntu26.04-x64', known), 'ubuntu24.04-x64');
  assert.equal(downloadHostPlatform('ubuntu26.10-arm64', known), 'ubuntu24.04-arm64');
  assert.equal(downloadHostPlatform('ubuntu24.04-x64', known), undefined);
  assert.equal(downloadHostPlatform('ubuntu18.04-x64', known), undefined);
  assert.equal(downloadHostPlatform('debian14-x64', known), undefined);
  assert.equal(downloadHostPlatform('mac15-arm64', known), undefined);
  assert.equal(downloadHostPlatform('ubuntu26.04-x64', ['debian13-x64']), undefined);
});

function timeout(path) {
  return Object.assign(
    new Error(
      `Command failed: node install-contract-browser.mjs --probe ${path}\n` +
        '::error::Browser probe exceeded 25s; closing its browser',
    ),
    { timedOut: true },
  );
}

test('a cold-start probe timeout is re-probed once and selects the installed browser', async () => {
  const probes = [];
  const ops = fixture({
    exists: (path) => path === '/installed/chrome',
    probe: async (path) => {
      probes.push(path);
      if (probes.length === 1) throw timeout(path);
      return 'Chrome 157';
    },
    download: () => assert.fail('a warm re-probe must avoid the download'),
  });
  assert.deepEqual(await ensureBrowser(ops), {
    path: '/installed/chrome',
    version: 'Chrome 157',
    source: 'installed',
  });
  assert.deepEqual(probes, ['/installed/chrome', '/installed/chrome']);
  assert.match(
    ops.logs[0],
    /^browser probe attempt 1\/2 timed out after \d+\.\ds: \/installed\/chrome: Browser probe exceeded 25s; closing its browser; re-probing once$/,
  );
  assert.match(
    ops.logs[1],
    /^browser probe attempt 2\/2 passed after \d+\.\ds: \/installed\/chrome$/,
  );
  assert.match(
    ops.logs[2],
    /^Selected installed browser \/installed\/chrome \(version Chrome 157\): it passed/,
  );
  assert.ok(ops.logs.every((row) => !row.startsWith('::')));
  assert.deepEqual(ops.calls, []);
});

test('each candidate gets at most two timed-out probes before the next is tried', async () => {
  const probes = [];
  const ops = fixture({
    probe: async (path) => {
      probes.push(path);
      if (path === '/cached/chrome') throw timeout(path);
      return 'working';
    },
  });
  assert.equal((await ensureBrowser(ops)).path, '/installed/chrome');
  assert.equal(probeAttempts, 2);
  assert.deepEqual(probes, ['/cached/chrome', '/cached/chrome', '/installed/chrome']);
  assert.match(
    ops.logs[1],
    /^browser probe attempt 2\/2 timed out after .*: \/cached\/chrome: .*closing its browser$/,
  );
});

test('a probe failure that is not a timeout is not re-probed', async () => {
  const probes = [];
  const ops = fixture({
    probe: async (path) => {
      probes.push(path);
      if (path === '/cached/chrome') throw new Error('protocol incompatible');
      return 'working';
    },
  });
  assert.equal((await ensureBrowser(ops)).path, '/installed/chrome');
  assert.deepEqual(probes, ['/cached/chrome', '/installed/chrome']);
  assert.match(
    ops.logs[0],
    /^browser probe attempt 1\/2 failed after .*: \/cached\/chrome: protocol incompatible$/,
  );
});
