// @vitest-environment node

import { afterEach, expect, it, vi } from 'vitest';
import { build } from 'vite';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const revision = '0123456789abcdef'.repeat(2) + '01234567';

afterEach(() => vi.unstubAllEnvs());

it.each([
  ['local', undefined, undefined, false],
  ['local', 'true', undefined, true],
  ['promotable', undefined, undefined, true],
  ['promotable', 'false', undefined, false],
  ['promotable', 'true', undefined, true],
  ['promotable', 'TRUE', undefined, false],
  ['promotable', undefined, 'false', false],
  ['promotable', 'true', 'false', true],
  ['promotable', 'false', 'true', false],
])(
  'compiles outbound recovery in %s builds with process %s and file %s',
  async (artifact, flag, file, expected) => {
    vi.stubEnv('CHRONO_BUILD_ARTIFACT', artifact);
    vi.stubEnv('CHRONO_BUILD', artifact === 'promotable' ? `git-${revision.slice(0, 12)}` : 'dev');
    vi.stubEnv('CHRONO_BUILD_SHA', artifact === 'promotable' ? revision : undefined);
    vi.stubEnv('VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED', flag);
    const envDir = mkdtempSync(join(tmpdir(), 'cma-outbound-build-'));
    try {
      if (file !== undefined)
        writeFileSync(
          join(envDir, '.env.production'),
          `VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED=${file}\n`,
        );

      const result = await build({
        configFile: new URL('../vite.config.ts', import.meta.url).pathname,
        mode: 'production',
        envDir,
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
    } finally {
      rmSync(envDir, { recursive: true, force: true });
    }
  },
);

it('passes the outbound flag from the canonical image build argument to Vite', () => {
  const dockerfile = readFileSync(new URL('../Dockerfile', import.meta.url), 'utf8');
  const stage = dockerfile.split(/^FROM /m)[1]!;
  expect(stage).toContain('ARG VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED=true');
  expect(stage).toContain(
    'VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED=$VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED',
  );
  expect(
    stage.indexOf('VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED=$VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED'),
  ).toBeLessThan(stage.indexOf('RUN npm run build'));
});
