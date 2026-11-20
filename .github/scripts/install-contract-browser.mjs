#!/usr/bin/env node
// Browser preparation only: callers must still run their complete contract suite.
import { execFileSync, spawn } from 'node:child_process';
import { appendFileSync, existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
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

// Successful CI downloads of the locked Chromium (167 MiB) take 3-5s. Each
// attempt allows 12x that; the download is idempotent, so a transient
// failure is retried after a short backoff.
export const downloadAttempts = 3;
export const downloadTimeoutMs = 60_000;
export const downloadBackoffMs = [5_000, 15_000];
const downloadTailLines = 40;
const unsupportedHost = /does not support \S+ on (\S+)/;
const networkFailure =
  /timed out|Download failed|ECONNRESET|ECONNREFUSED|ENOTFOUND|EAI_AGAIN|ETIMEDOUT|ENETUNREACH|socket hang up|getaddrinfo/i;
const progressRow = /^\|[^|]*\|\s+\d+% of /;

function describe(error) {
  const lines = String(error?.message ?? error)
    .split('\n')
    .map((line) => line.replace(/^::error::/, '').trim())
    .filter(Boolean);
  return (
    lines.find((line) => !line.startsWith('Command failed')) ??
    lines[0] ??
    'unknown error'
  ).slice(0, 300);
}

// Playwright prints progress, paths, URLs and errors. Keep only the last
// progress row of each run, redact URL credentials, and indent every row so
// downloader output can never start a workflow command.
export function outputTail(output, limit = downloadTailLines) {
  const rows = [];
  for (const raw of String(output ?? '').split(/\r?\n|\r/)) {
    const row = raw.trimEnd();
    if (!row) continue;
    if (progressRow.test(row) && progressRow.test(rows.at(-1) ?? '')) rows.pop();
    rows.push(row);
  }
  return rows.slice(-limit).map((row) => `  ${row.replace(/\/\/[^/\s@]+@/g, '//***@')}`);
}

// The downloader forks a worker that writes the archive. Running it as its
// own process group lets a timeout stop that worker too, so nothing it leaves
// behind can keep writing the browser directory.
export function runBounded(command, args, { timeoutMs, ...options }) {
  return new Promise((resolveRun, rejectRun) => {
    let output = '',
      timedOut = false,
      settled = false;
    const child = spawn(command, args, {
      ...options,
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    const stopGroup = () => {
      try {
        process.kill(-child.pid, 'SIGKILL');
      } catch {
        // The whole group has already exited.
      }
    };
    const timer = setTimeout(() => {
      timedOut = true;
      stopGroup();
    }, timeoutMs);
    const settle = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (!error) return resolveRun(output);
      error.output = output;
      rejectRun(error);
    };
    for (const stream of [child.stdout, child.stderr]) {
      stream.setEncoding('utf8');
      stream.on('data', (chunk) => {
        output = (output + chunk).slice(-65_536);
      });
    }
    child.on('error', settle);
    child.on('exit', stopGroup);
    child.on('close', (code, signal) => {
      if (code === 0 && !timedOut) return settle();
      const error = new Error(
        timedOut
          ? `Command timed out after ${timeoutMs / 1000}s: ${command} ${args.join(' ')}`
          : `Command failed with ${signal ? `signal ${signal}` : `exit code ${code}`}: ${command} ${args.join(' ')}`,
      );
      if (timedOut) error.code = 'ETIMEDOUT';
      settle(error);
    });
  });
}

export async function downloadBrowser(attempt, log, wait = delay) {
  let failure;
  for (let count = 1; count <= downloadAttempts; count += 1) {
    log(
      `browser download attempt ${count}/${downloadAttempts}; locked Playwright Chromium, ` +
        `${downloadTimeoutMs / 1000}s limit`,
    );
    const started = Date.now();
    try {
      await attempt(downloadTimeoutMs);
      return;
    } catch (error) {
      failure = error;
      const seconds = ((Date.now() - started) / 1000).toFixed(1);
      log(
        `browser download attempt ${count}/${downloadAttempts} failed after ${seconds}s: ${describe(error)}`,
      );
      // An unsupported host fails identically every time; retrying only delays the report.
      if (unsupportedHost.test(String(error.output ?? '')) || count === downloadAttempts) break;
      const backoff = downloadBackoffMs[count - 1];
      log(`browser download retry in ${backoff / 1000}s`);
      await wait(backoff);
    }
  }
  const tail = outputTail(failure.output);
  log(
    tail.length
      ? `Browser downloader output, last ${tail.length} lines:\n${tail.join('\n')}`
      : 'Browser downloader printed no output.',
  );
  const output = String(failure.output ?? '');
  const host = unsupportedHost.exec(output)?.[1];
  if (host) {
    throw new Error(
      `Pinned Playwright Chromium download refused: the locked Playwright has no Chromium build for ${host}. ` +
        'This is not a network failure, so it was not retried. Update the locked Playwright version ' +
        `in web/package-lock.json or run on a supported runner. Browser contract tests remain required. ${describe(failure)}`,
    );
  }
  const network = failure.code === 'ETIMEDOUT' || networkFailure.test(output);
  throw new Error(
    `Pinned Playwright Chromium download failed after ${downloadAttempts} bounded attempts ` +
      `(${downloadTimeoutMs / 1000}s each); ` +
      (network
        ? 'likely a network or CDN failure. '
        : 'the downloader output above shows no network failure. ') +
      'Re-run the job; if it persists, check runner access to cdn.playwright.dev or the configured proxy, ' +
      'and the locked Playwright version in web/package-lock.json. ' +
      `Browser contract tests remain required. ${describe(failure)}`,
  );
}

const ubuntuPlatform = /^ubuntu(\d+)\.(\d+)-(x64|arm64)$/;

// Playwright names an Ubuntu release newer than its tables after itself and
// then has no download for it (1.58.2: "does not support chromium on
// ubuntu26.04-x64"). For each architecture its Ubuntu Chromium archive is the
// same generic Linux build on every release, so select the newest Ubuntu
// table it knows. The launch/render probe still decides whether it is usable.
export function downloadHostPlatform(hostPlatform, known) {
  const host = ubuntuPlatform.exec(hostPlatform);
  if (!host || known.includes(hostPlatform)) return undefined;
  const release = (match) => Number(match[1]) * 100 + Number(match[2]);
  return known
    .map((platform) => ubuntuPlatform.exec(platform))
    .filter((match) => match && match[3] === host[3] && release(match) < release(host))
    .sort((left, right) => release(right) - release(left))[0]?.[0];
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
    await ops.download();
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
  const env = {
    ...process.env,
    DEBIAN_FRONTEND: 'noninteractive',
    PLAYWRIGHT_DOWNLOAD_CONNECTION_TIMEOUT: '30000',
  };
  const tables = () => ({
    hostPlatform: require(join(core, 'lib/server/utils/hostPlatform.js')).hostPlatform,
    deps: require(join(core, 'lib/server/registry/nativeDeps.js')).deps,
  });
  const run = (command, args, options = {}) =>
    execFileSync(command, args, {
      cwd: web,
      stdio: 'pipe',
      encoding: 'utf8',
      killSignal: 'SIGKILL',
      env,
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
      const { hostPlatform, deps } = tables();
      const mapping = deps[hostPlatform]?.lib2package;
      if (!mapping) throw new Error(`No browser package mapping for ${hostPlatform}`);
      return mapping;
    },
    download: () => {
      console.log('No usable installed browser; download locked Playwright Chromium without apt.');
      const cli = join(dirname(require.resolve('playwright/package.json')), 'cli.js');
      const { hostPlatform, deps } = tables();
      const fallback = env.PLAYWRIGHT_HOST_PLATFORM_OVERRIDE
        ? undefined
        : downloadHostPlatform(hostPlatform, Object.keys(deps));
      if (fallback) {
        console.log(
          `Locked Playwright has no Chromium download for ${hostPlatform}; using its newest ` +
            `Ubuntu build (${fallback}), the same generic Linux archive. The probe still decides.`,
        );
      }
      return downloadBrowser(
        (timeoutMs) =>
          runBounded(process.execPath, [cli, 'install', '--no-shell', 'chromium'], {
            cwd: web,
            env: fallback ? { ...env, PLAYWRIGHT_HOST_PLATFORM_OVERRIDE: fallback } : env,
            timeoutMs,
          }),
        console.log,
      );
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
