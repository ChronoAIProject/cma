import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import { systemText } from './build/systemText';
import { devFixtures } from './build/devFixtures';
import { fileURLToPath } from 'node:url';

export default defineConfig(({ command, mode }) => ({
  // `npm run dev:fixtures` only: answers /auth and /api locally (build/devFixtures.ts).
  plugins: [react(), ...(command === 'serve' && mode === 'fixtures' ? [devFixtures()] : [])],
  css: { postcss: { plugins: [systemText()] } },
  server: {
    fs: {
      // Shared public documentation and SDK sources are outside the web package.
      allow: ['.', '../sdk/typescript/src', '../docs', '../api/crates/cmaeg/assets/docs'].map(
        (path) => fileURLToPath(new URL(path, import.meta.url)),
      ),
    },
  },
  resolve: {
    alias: {
      '@chronoai/cma-sdk': fileURLToPath(
        new URL('../sdk/typescript/src/index.ts', import.meta.url),
      ),
    },
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
    // Components under test render real DOM and use browser focus behavior.
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
}));
