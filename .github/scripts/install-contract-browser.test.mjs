import test from 'node:test';
import assert from 'node:assert/strict';
import {
  ensureBrowser,
  missingLibraries,
  requiredPackages,
  repairDependencies,
} from './install-contract-browser.mjs';

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
  assert.deepEqual(await ensureBrowser(ops), { path: '/cached/chrome', version: 'test-version' });
  assert.match(ops.logs.join('\n'), /apt not needed \(0 calls\)/);
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
  assert.equal((await ensureBrowser(ops)).path, '/cached/chrome');
  assert.equal(downloaded, true);
  assert.deepEqual(ops.calls, []);
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
