#!/usr/bin/env node
// Browser preparation only: callers must still run their complete contract suite.
import { execFileSync } from 'node:child_process';
import { appendFileSync, existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const web = join(root, 'web');
export const aptOptions = [
  '-o',
  'Acquire::Retries=0',
  '-o',
  'Acquire::http::Timeout=10',
  '-o',
  'Acquire::https::Timeout=10',
  '-o',
  'DPkg::Lock::Timeout=10',
  '-o',
  'APT::Update::Error-Mode=any',
];

export function missingLibraries(output) {
  return [
    ...new Set([...output.matchAll(/^\s*(\S+)\s+=>\s+not found\s*$/gm)].map((match) => match[1])),
  ].sort();
}

export function requiredPackages(libraries, mapping) {
  const unknown = libraries.filter((library) => !mapping[library]);
  if (unknown.length) {
    throw new Error(
      `Missing browser libraries have no package mapping: ${unknown.join(', ')}. ` +
        'Update the supported runner/dependency mapping; tests cannot run without these libraries.',
    );
  }
  const packages = [...new Set(libraries.map((library) => mapping[library]))].sort();
  if (packages.some((name) => !/^[a-z0-9][a-z0-9+.-]*$/.test(name))) {
    throw new Error('Invalid browser dependency package name');
  }
  return packages;
}

export function repairDependencies(packages, run, log) {
  if (!packages.length) throw new Error('Refusing apt without demonstrated missing packages');
  for (const command of [
    ['update'],
    ['install', '--yes', '--no-install-recommends', ...packages],
  ]) {
    let lastError;
    for (let attempt = 1; attempt <= 2; attempt += 1) {
      log(`apt ${command[0]} attempt ${attempt}/2; required packages: ${packages.join(', ')}`);
      try {
        // Root-owned timeout terminates apt and its children, including stuck DNS.
        // No source/keyring rewrite, unauthenticated package or broad upgrade.
        run(
          'sudo',
          [
            '-n',
            'env',
            'DEBIAN_FRONTEND=noninteractive',
            'timeout',
            '--kill-after=5s',
            '45s',
            'apt-get',
            ...aptOptions,
            ...command,
          ],
          { timeout: 55_000 },
        );
        lastError = undefined;
        break;
      } catch (error) {
        lastError = error;
      }
    }
    if (lastError) {
      throw new Error(
        `Browser dependencies unavailable: ${packages.join(', ')}. ` +
          `apt ${command[0]} failed after two bounded attempts (45s each). ` +
          'Check Ubuntu mirror/DNS access or supply a runner with these packages. ' +
          `Browser contract tests remain required. ${lastError.message}`,
      );
    }
  }
}

export async function ensureBrowser(ops) {
  let repair;
  const failures = [];
  async function inspect(path) {
    try {
      const version = await ops.probe(path);
      return { path, version };
    } catch (error) {
      failures.push(`${path}: ${error.message}`);
      const libraries = ops.missing(path);
      if (libraries.length && !repair) repair = { path, libraries };
      return undefined;
    }
  }
  for (const path of [...new Set(ops.candidates)]) {
    if (!ops.exists(path)) continue;
    const browser = await inspect(path);
    if (browser) {
      ops.log('Browser and dependencies verified; apt not needed (0 calls).');
      return browser;
    }
  }
  if (!repair) {
    // Use the locked Playwright package, without its unconditional apt installer.
    ops.download();
    if (!ops.exists(ops.pinned))
      throw new Error('Pinned Playwright browser is absent after download');
    const browser = await inspect(ops.pinned);
    if (browser) {
      ops.log('Downloaded browser and dependencies verified; apt not needed (0 calls).');
      return browser;
    }
  }
  if (!repair) {
    throw new Error(
      'No browser passed the launch/render probe; no missing shared libraries were found. ' +
        `Refusing unrelated apt changes. ${failures.join('\n')}`,
    );
  }
  const packages = requiredPackages(repair.libraries, ops.mapping());
  ops.log(`Missing shared libraries: ${repair.libraries.join(', ')}`);
  repairDependencies(packages, ops.run, ops.log);
  const remaining = ops.missing(repair.path);
  if (remaining.length)
    throw new Error(`Browser libraries still missing after apt: ${remaining.join(', ')}`);
  try {
    return { path: repair.path, version: await ops.probe(repair.path) };
  } catch (error) {
    throw new Error(
      `Browser still cannot launch/render after installing ${packages.join(', ')}: ${error.message}`,
    );
  }
}

// Separate process bounds both launch and close; a broken browser cannot hold CI.
async function probe(path) {
  const require = createRequire(join(web, 'package.json'));
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ executablePath: path, headless: true, timeout: 15_000 });
  try {
    const page = await browser.newPage();
    await page.setContent('<title>contract-browser-probe</title><p>ready</p>');
    if ((await page.title()) !== 'contract-browser-probe')
      throw new Error('Browser page evaluation failed');
    await page.screenshot();
    process.stdout.write(browser.version());
  } finally {
    await browser.close();
  }
}

export async function main() {
  const require = createRequire(join(web, 'package.json'));
  const { chromium } = require('playwright');
  const core = dirname(require.resolve('playwright-core/package.json'));
  const pinned = chromium.executablePath();
  const run = (command, args, options = {}) =>
    execFileSync(command, args, {
      cwd: web,
      stdio: 'pipe',
      encoding: 'utf8',
      killSignal: 'SIGKILL',
      env: {
        ...process.env,
        DEBIAN_FRONTEND: 'noninteractive',
        PLAYWRIGHT_DOWNLOAD_CONNECTION_TIMEOUT: '30000',
      },
      ...options,
    });
  const result = await ensureBrowser({
    candidates: [
      pinned,
      '/opt/google/chrome/chrome',
      '/usr/bin/google-chrome',
      '/usr/lib/chromium/chromium',
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    ],
    pinned,
    exists: existsSync,
    log: console.log,
    run: (command, args, options) => {
      const output = run(command, args, options);
      if (output) process.stdout.write(output);
    },
    probe: async (path) =>
      run(process.execPath, [fileURLToPath(import.meta.url), '--probe', path], {
        timeout: 25_000,
      }).trim(),
    missing: (path) => {
      if (process.platform !== 'linux') return [];
      // Some preinstalled Chrome entry points are shell wrappers; their failed
      // probe is retained, while the real ELF or pinned binary supplies ldd proof.
      try {
        return missingLibraries(run('ldd', [path], { timeout: 10_000 }));
      } catch (error) {
        return missingLibraries(String(error.stdout ?? ''));
      }
    },
    mapping: () => {
      // Reuse the locked Playwright version's distro-specific t64/package map.
      const { hostPlatform } = require(join(core, 'lib/server/utils/hostPlatform.js'));
      const { deps } = require(join(core, 'lib/server/registry/nativeDeps.js'));
      const mapping = deps[hostPlatform]?.lib2package;
      if (!mapping) throw new Error(`No browser package mapping for ${hostPlatform}`);
      return mapping;
    },
    download: () => {
      console.log('No usable installed browser; download locked Playwright Chromium without apt.');
      const cli = join(dirname(require.resolve('playwright/package.json')), 'cli.js');
      run(process.execPath, [cli, 'install', '--no-shell', 'chromium'], { timeout: 120_000 });
    },
  });
  if (/[\r\n]/.test(result.path)) throw new Error('Invalid browser executable path');
  if (process.env.GITHUB_ENV) {
    appendFileSync(
      process.env.GITHUB_ENV,
      `CMA_E2E_BROWSER_PATH=${result.path}\nCHROME_PATH=${result.path}\n`,
    );
  }
  console.log(
    `Contract browser ready: ${result.path}; version ${result.version}. All contract tests remain mandatory.`,
  );
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv[2] === '--probe') await probe(process.argv[3]);
    else await main();
  } catch (error) {
    console.error(`::error::${error.message}`);
    process.exitCode = 1;
  }
}
