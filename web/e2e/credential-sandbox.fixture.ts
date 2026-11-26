import { fileURLToPath } from 'node:url';
import { build } from 'vite';
import type { Page } from '@playwright/test';

export const sandboxHarnessPath = '/__credential-sandbox__/';

export async function buildSandboxHarness() {
  const bundle = await build({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: fileURLToPath(new URL('../vite.config.ts', import.meta.url)),
    base: sandboxHarnessPath,
    publicDir: false,
    logLevel: 'warn',
    build: {
      write: false,
      rollupOptions: {
        input: fileURLToPath(new URL('./credential-sandbox.entry.ts', import.meta.url)),
        output: { inlineDynamicImports: true },
      },
    },
  });
  if (Array.isArray(bundle) || !('output' in bundle))
    throw new Error('Expected one retained Sandbox browser bundle.');
  const entry = bundle.output.find((file) => file.type === 'chunk' && file.isEntry);
  if (!entry) throw new Error('Retained Sandbox browser entry missing.');
  const styles = bundle.output
    .filter((file) => file.fileName.endsWith('.css'))
    .map((file) => `<link rel="stylesheet" href="${sandboxHarnessPath}${file.fileName}">`)
    .join('');
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Retained Sandbox component test</title>${styles}</head><body>
    <div id="root"></div><script type="module" src="${sandboxHarnessPath}${entry.fileName}"></script>
    </body></html>`;
  return async (page: Page, baseURL: string) => {
    await page.route(`${baseURL}${sandboxHarnessPath}**`, (route) => {
      const name = new URL(route.request().url()).pathname.slice(sandboxHarnessPath.length);
      if (name === '') return route.fulfill({ contentType: 'text/html', body: html });
      const file = bundle.output.find((asset) => asset.fileName === name);
      if (!file) throw new Error(`Unexpected retained Sandbox asset: ${name}`);
      return route.fulfill({
        contentType:
          file.type === 'chunk'
            ? 'text/javascript'
            : name.endsWith('.css')
              ? 'text/css'
              : 'application/octet-stream',
        body: file.type === 'chunk' ? file.code : Buffer.from(file.source),
      });
    });
  };
}
