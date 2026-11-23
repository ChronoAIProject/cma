// @vitest-environment node
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { realpathSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { afterEach, expect, it, vi } from 'vitest';
import { build } from 'vite';
import { optionalTopographicImport } from './topographicImport';

const require = createRequire(import.meta.url);
const { xxhashBase64Url } = require(
  join(dirname(require.resolve('rollup/package.json')), 'dist/native.js'),
) as { xxhashBase64Url: (input: Buffer) => string };

function dollarPairRoot(parent: string) {
  // Match pinned Rollup's chunk-reference hash without replacing emitFile or the build.
  const directory = realpathSync(parent);
  for (let index = 0; index < 100_000; index++) {
    const root = join(directory, `reference-${index}`);
    const id = join(root, 'src/decoration/vendor/reactdrops-topographic/topographic-surface.ts');
    const reference = xxhashBase64Url(Buffer.from(id)).slice(0, 8).replaceAll('-', '$');
    if (reference.includes('$$')) return root;
  }
  throw new Error('Could not select a Rollup chunk reference containing a dollar pair.');
}

const roots: string[] = [];
afterEach(async () => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

async function fixture(
  flag: string | undefined,
  source = 'export function createTopographicSurface() { return 42; }',
  dollarPair = false,
) {
  vi.stubEnv('VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED', flag);
  const parent = await mkdtemp(join(tmpdir(), 'cma-flow-build-'));
  roots.push(parent);
  const root = dollarPair ? dollarPairRoot(parent) : parent;
  const decoration = join(root, 'src/decoration');
  await mkdir(join(decoration, 'vendor/reactdrops-topographic'), { recursive: true });
  await writeFile(join(root, 'package.json'), '{"type":"module"}');
  await writeFile(
    join(decoration, 'topographicLoader.ts'),
    await readFile(new URL('../src/decoration/topographicLoader.ts', import.meta.url)),
  );
  await writeFile(join(decoration, 'vendor/reactdrops-topographic/topographic-surface.ts'), source);
  await writeFile(
    join(root, 'ordinary.ts'),
    "throw new Error('synthetic ordinary failure'); export const ordinary = true;",
  );
  await writeFile(
    join(root, 'entry.ts'),
    `
    import { loadTopographicSurface } from './src/decoration/topographicLoader';
    export const load = import.meta.env.VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED === 'true' ? loadTopographicSurface : null;
    export const loadOrdinary = () => import('./ordinary');
  `,
  );
  const references: string[] = [];
  return {
    root,
    decoration,
    references,
    compile: () =>
      build({
        root,
        configFile: false,
        logLevel: 'silent',
        plugins: [
          optionalTopographicImport(),
          {
            name: 'record-emitted-references',
            resolveFileUrl({ referenceId }) {
              references.push(referenceId);
              return null;
            },
          },
        ],
        build: {
          minify: false,
          rollupOptions: { input: join(root, 'entry.ts'), preserveEntrySignatures: 'strict' },
        },
      }),
  };
}

it.each([false, true])(
  'emits a native optional import with retained exports while ordinary imports keep fatal reporting (dollar pair: %s)',
  async (dollarPair) => {
    const { root, compile, references } = await fixture('true', undefined, dollarPair);
    const result = await compile();
    if (dollarPair) expect(references.some((reference) => reference.includes('$$'))).toBe(true);
    if (Array.isArray(result) || !('output' in result)) throw new Error('Expected one build');
    const entry = result.output.find(
      (output) => output.type === 'chunk' && output.name === 'entry',
    );
    const surface = result.output.find(
      (output) => output.type === 'chunk' && output.name === 'topographic-surface',
    );
    if (entry?.type !== 'chunk' || surface?.type !== 'chunk') throw new Error('Missing entry');
    expect(entry.code).toContain('vite:preloadError');
    expect(surface.exports).toContain('createTopographicSurface');
    const module = await import(
      /* @vite-ignore */ pathToFileURL(join(root, 'dist', entry.fileName)).href
    );
    expect((await module.load())()).toBe(42);
  },
);

it('contains a real emitted module failure without dispatching an application preload event', async () => {
  const dispatchEvent = vi.fn();
  vi.stubGlobal('window', { dispatchEvent });
  const { root, compile } = await fixture(
    'true',
    "throw new Error('synthetic optional failure'); export function createTopographicSurface() {} ",
  );
  const result = await compile();
  if (Array.isArray(result) || !('output' in result)) throw new Error('Expected one build');
  const entry = result.output.find((output) => output.type === 'chunk' && output.name === 'entry')!;
  const module = await import(
    /* @vite-ignore */ pathToFileURL(join(root, 'dist', entry.fileName)).href
  );
  await expect(module.load()).resolves.toBeNull();
  expect(dispatchEvent).not.toHaveBeenCalled();
  await expect(module.loadOrdinary()).rejects.toThrow('synthetic ordinary failure');
  expect(dispatchEvent).toHaveBeenCalledTimes(1);
  expect(dispatchEvent.mock.calls[0]![0].type).toBe('vite:preloadError');
});

it.each([undefined, 'false', 'TRUE'])('emits no Flow chunk when the flag is %s', async (flag) => {
  const { compile } = await fixture(flag);
  const result = await compile();
  if (Array.isArray(result) || !('output' in result)) throw new Error('Expected one build');
  expect(result.output.some((output) => output.fileName.includes('topographic-surface'))).toBe(
    false,
  );
});

it('fails the build when a loader refactor would bypass the optional boundary', async () => {
  const { decoration, compile } = await fixture('true');
  await writeFile(
    join(decoration, 'topographicLoader.ts'),
    'export const loadTopographicSurface = () => Promise.resolve(null);',
  );
  await expect(compile()).rejects.toThrow(
    'Update the optional Flow import boundary before building.',
  );
});
