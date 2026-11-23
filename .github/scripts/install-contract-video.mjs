#!/usr/bin/env node
import { existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { downloadHostPlatform, outputTail, runBounded } from './install-contract-browser.mjs';

export async function ensureVideoEncoder({ exists, download, probe }) {
  if (!exists()) await download();
  if (!exists()) throw new Error('Pinned Playwright video encoder is absent after download');
  await probe();
}

export async function main() {
  const web = fileURLToPath(new URL('../../web/', import.meta.url));
  const require = createRequire(join(web, 'package.json'));
  const core = dirname(require.resolve('playwright-core/package.json'));
  const { registry } = require(join(core, 'lib/server/registry/index.js'));
  const encoder = registry.findExecutable('ffmpeg').executablePath();
  const { hostPlatform } = require(join(core, 'lib/server/utils/hostPlatform.js'));
  const { deps } = require(join(core, 'lib/server/registry/nativeDeps.js'));
  const fallback = process.env.PLAYWRIGHT_HOST_PLATFORM_OVERRIDE
    ? undefined
    : downloadHostPlatform(hostPlatform, Object.keys(deps));
  const env = {
    ...process.env,
    PLAYWRIGHT_DOWNLOAD_CONNECTION_TIMEOUT: '30000',
    ...(fallback ? { PLAYWRIGHT_HOST_PLATFORM_OVERRIDE: fallback } : {}),
  };
  const cli = join(dirname(require.resolve('playwright/package.json')), 'cli.js');
  await ensureVideoEncoder({
    exists: () => existsSync(encoder),
    download: async () => {
      console.log('Install the locked Playwright video encoder for recorded browser contracts.');
      const output = await runBounded(process.execPath, [cli, 'install', 'ffmpeg'], {
        cwd: web,
        env,
        timeoutMs: 60_000,
      });
      console.log(outputTail(output).join('\n'));
    },
    probe: () => runBounded(encoder, ['-version'], { cwd: web, env, timeoutMs: 10_000 }),
  });
  console.log('Pinned Playwright video encoder verified; recorded contracts remain required.');
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    await main();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
