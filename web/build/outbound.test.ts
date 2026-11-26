// @vitest-environment node

import { afterEach, expect, it, vi } from 'vitest';
import { build } from 'vite';

const revision = '0123456789abcdef'.repeat(2) + '01234567';

afterEach(() => vi.unstubAllEnvs());

it.each([
  ['local', undefined, false],
  ['local', 'true', true],
  ['promotable', undefined, true],
  ['promotable', 'false', false],
  ['promotable', 'true', true],
  ['promotable', 'TRUE', false],
])('compiles outbound recovery in %s builds with override %s', async (artifact, flag, expected) => {
  vi.stubEnv('CHRONO_BUILD_ARTIFACT', artifact);
  vi.stubEnv('CHRONO_BUILD', artifact === 'promotable' ? `git-${revision.slice(0, 12)}` : 'dev');
  vi.stubEnv('CHRONO_BUILD_SHA', artifact === 'promotable' ? revision : undefined);
  vi.stubEnv('VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED', flag);

  const result = await build({
    configFile: new URL('../vite.config.ts', import.meta.url).pathname,
    logLevel: 'silent',
    plugins: [
      {
        name: 'outbound-build-fixture',
        resolveId: (id) => (id === 'virtual:outbound-build' ? id : null),
        load: (id) =>
          id === 'virtual:outbound-build'
            ? 'globalThis.outboundRecoveryFixture = import.meta.env.VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED === "true";'
            : null,
      },
    ],
    build: {
      write: false,
      minify: false,
      rollupOptions: { input: 'virtual:outbound-build' },
    },
  });
  if ('on' in result || Array.isArray(result)) throw new Error('Expected one completed bundle');
  const chunk = result.output.find((file) => file.type === 'chunk');
  expect(chunk?.code).toContain(`globalThis.outboundRecoveryFixture = ${expected};`);
});
