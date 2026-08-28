import { readdir, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import type { Plugin } from 'vite';
import react from '@vitejs/plugin-react';
import { compileSdkDocuments, SDK_LANGUAGES } from './src/developer/docSet';

const repositoryRoot = fileURLToPath(new URL('..', import.meta.url));
const sdkRoot = resolve(repositoryRoot, 'sdk');

async function rawSdkDocuments(): Promise<Record<string, string>> {
  const sources: Record<string, string> = {};
  for (const sdk of SDK_LANGUAGES) {
    const directory = resolve(sdkRoot, sdk, 'docs');
    const entries = await readdir(directory, { withFileTypes: true });
    await Promise.all(
      entries
        .filter((entry) => entry.isFile() && entry.name.endsWith('.md'))
        .map(async (entry) => {
          sources[`/sdk/${sdk}/docs/${entry.name}`] = await readFile(
            resolve(directory, entry.name),
            'utf8',
          );
        }),
    );
  }
  return sources;
}

/** Reject an incomplete bundle before Vite emits any asset. */
function validateSdkDocumentation(): Plugin {
  return {
    name: 'validate-sdk-documentation',
    async buildStart() {
      compileSdkDocuments(await rawSdkDocuments());
    },
  };
}

export default defineConfig({
  plugins: [validateSdkDocumentation(), react()],
  server: {
    // The raw documentation glob sits beside web/, under the repository root.
    fs: { allow: [repositoryRoot] },
  },
  /*
   * Which build is on screen, for the status band (Web_Style.md §6). Baked in
   * rather than fetched: a build identifier the running page asks a server for
   * is one that can disagree with the code answering the question. Unset — a
   * local run, and CI's image build — leaves the honest `dev`.
   */
  define: {
    __CHRONO_BUILD__: JSON.stringify(process.env.CHRONO_BUILD ?? 'dev'),
  },
  test: {
    globals: true,
    // Components under test render real DOM: the intro prompt is a genuine
    // <input> and the profile menu is exercised through keyboard events.
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    coverage: {
      provider: 'v8',
      // CLAUDE.md and the global engineering rules require >= 80%.
      thresholds: { lines: 80, functions: 80, branches: 80, statements: 80 },
      include: ['src/**/*.{ts,tsx}'],
      exclude: ['src/**/*.test.{ts,tsx}', 'src/test/**', 'src/main.tsx'],
    },
  },
});
