import { configDefaults, defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import { buildIdentity, buildIdentityEvidence } from './build/identity';
import { replicaRecoveryEnabled, recoveryEvidence } from './build/recovery';
import { systemText } from './build/systemText';
import { devFixtures } from './build/devFixtures';
import { fileURLToPath } from 'node:url';

export default defineConfig(({ command, mode }) => {
  const identity = buildIdentity(process.env);
  const recovery = replicaRecoveryEnabled(process.env);
  return {
    // `npm run dev:fixtures` only: answers /auth and /api locally (build/devFixtures.ts).
    plugins: [
      react(),
      buildIdentityEvidence(identity),
      recoveryEvidence(identity, recovery),
      ...(command === 'serve' && mode === 'fixtures' ? [devFixtures()] : []),
    ],
    css: { postcss: { plugins: [systemText()] } },
    server: {
      fs: {
        // Public gateway assets are owned by the backend component.
        allow: ['.', '../api/crates/cmaeg/assets/docs'].map((path) =>
          fileURLToPath(new URL(path, import.meta.url)),
        ),
      },
    },
    resolve: {
      alias: {
        '@cma/client': fileURLToPath(new URL('./src/api-client/index.ts', import.meta.url)),
      },
    },
    // Bundle constants and artifact evidence share one validated identity.
    define: {
      __CHRONO_BUILD__: JSON.stringify(identity.version),
      __CHRONO_BUILD_SHA__: JSON.stringify(identity.revision),
      __CHRONO_BUILD_LOCAL__: JSON.stringify(identity.local),
      __CHRONO_WEB_REPLICA_RECOVERY_ENABLED__: JSON.stringify(recovery),
    },
    test: {
      include:
        mode === 'approval-contract'
          ? ['src/api-client/tests/external-approval-wire.contract.ts']
          : configDefaults.include,
      // Web CI runs this browser fixture suite with node:test.
      exclude: [...configDefaults.exclude, 'e2e/owner-timing/run.test.mjs'],
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
  };
});
