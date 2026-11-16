import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import type { Page, TestInfo } from '@playwright/test';

const sha256 = (value: string | Buffer) => createHash('sha256').update(value).digest('hex');

/** Observe served bytes; a source label alone cannot identify a compiled app. */
export async function bindOpeningBuild(page: Page) {
  const manifestPath = process.env.CMA_OPEN_BINDING;
  if (manifestPath === undefined) return async () => undefined;
  const bytes = await readFile(manifestPath);
  const manifest = JSON.parse(bytes.toString()) as { assets: Record<string, string> };
  const pending: Promise<void>[] = [];
  const assets: { path: string; sha256: string }[] = [];
  const failures: string[] = [];
  const observe = (response: import('@playwright/test').Response) => {
    const path = new URL(response.url()).pathname;
    if (!/\.(js|css)$/.test(path)) return;
    pending.push(
      (async () => {
        const digest = sha256(await response.body());
        if (response.status() !== 200 || manifest.assets[path] !== digest)
          failures.push(`Compiled asset binding mismatch: ${path}`);
        assets.push({ path, sha256: digest });
      })().catch(() => {
        failures.push(`Compiled asset body unavailable: ${path}`);
      }),
    );
  };
  page.on('response', observe);
  return async (info: TestInfo) => {
    page.off('response', observe);
    await Promise.all(pending);
    if (!assets.some((asset) => asset.path.startsWith('/assets/index-')))
      failures.push('Main compiled asset was not observed.');
    const receipt = {
      manifestSha256: sha256(bytes),
      nodeVersion: process.version,
      nodeExecutableSha256: sha256(await readFile(process.execPath)),
      assets,
      failures,
    };
    await info.attach('compiled-binding', {
      body: JSON.stringify(receipt, null, 2),
      contentType: 'application/json',
    });
    if (failures.length > 0) throw new Error(failures.join('\n'));
  };
}
