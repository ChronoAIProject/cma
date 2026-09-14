import { lstatSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { BrowserContext, Page } from '@playwright/test';

export function isJsonMediaType(contentType: string): boolean {
  return /^application\/(?:[a-z0-9!#$&^_.+-]+\+)?json(?:\s*;|$)/i.test(contentType.trim());
}

export function prepareEvidenceDirectory(output: string, run: string) {
  if (!/^[a-zA-Z0-9_-]{16,128}$/.test(run)) throw new Error('Invalid E2E evidence run identity');
  try {
    mkdirSync(output, { mode: 0o700 });
  } catch {
    throw new Error('E2E output must be a fresh private directory under an existing parent');
  }
  return () => writeFileSync(join(output, '.validated-run'), run, { mode: 0o600, flag: 'wx' });
}

export function evidenceDirectoryIsReady(output: string, run: string | undefined): boolean {
  if (run === undefined || !/^[a-zA-Z0-9_-]{16,128}$/.test(run)) return false;
  try {
    const directory = lstatSync(output);
    const marker = join(output, '.validated-run');
    const file = lstatSync(marker);
    return (
      directory.isDirectory() &&
      !directory.isSymbolicLink() &&
      (directory.mode & 0o077) === 0 &&
      file.isFile() &&
      !file.isSymbolicLink() &&
      (file.mode & 0o077) === 0 &&
      readFileSync(marker, 'utf8') === run
    );
  } catch {
    return false;
  }
}

export function credentialValues(path?: string): string[] {
  if (path === undefined) return [];
  const value: unknown = JSON.parse(readFileSync(path, 'utf8'));
  if (
    !Array.isArray(value) ||
    value.length === 0 ||
    value.some((entry) => typeof entry !== 'string' || entry.length < 8)
  )
    throw new Error(
      'forbiddenValuesFile must contain nonempty credential sentinels, each at least 8 characters',
    );
  return value as string[];
}
export function assertCredentialFree(text: string, forbidden: readonly string[]) {
  const credentialField =
    /(?:access_token|refresh_token|client_secret|private_key|api_key)["']?\s*[:=]\s*["']?[^\s"',}\]]+/i;
  const privateKey = /-----BEGIN [A-Z ]*PRIVATE KEY-----/;
  if (
    credentialField.test(text) ||
    privateKey.test(text) ||
    forbidden.some((value) => text.includes(value))
  )
    throw new Error(
      'Credential material found in a CMA browser-visible surface; evidence capture refused',
    );
}
export async function assertPageSafe(page: Page, baseURL: string, forbidden: readonly string[]) {
  const url = new URL(page.url());
  if (url.origin !== baseURL || url.pathname.startsWith('/auth/') || url.search || url.hash)
    throw new Error(
      'Screenshots and DOM inspection are restricted to canonical CMA application pages',
    );
  const surfaces = await page.evaluate(() => ({
    html: document.documentElement.outerHTML,
    local: JSON.stringify(localStorage),
    session: JSON.stringify(sessionStorage),
    passwordFields: document.querySelectorAll('input[type="password"]').length,
  }));
  if (surfaces.passwordFields) throw new Error('Credential-entry forms cannot be captured');
  for (const surface of [surfaces.html, surfaces.local, surfaces.session])
    assertCredentialFree(surface, forbidden);
}
export async function readOnlyGuard(context: BrowserContext, blocked: string[]) {
  await context.route('**/*', async (route) => {
    if (!['GET', 'HEAD', 'OPTIONS'].includes(route.request().method())) {
      blocked.push(route.request().method());
      await route.abort('blockedbyclient');
    } else await route.fallback();
  });
}
