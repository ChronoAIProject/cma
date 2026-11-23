// @vitest-environment node
import { realpathSync } from 'node:fs';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { afterEach, expect, it, vi } from 'vitest';
import { build, normalizePath, type ResolvedConfig } from 'vite';
import { optionalTopographicImport } from './topographicImport';

const roots: string[] = [];
afterEach(async () => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

async function fixture(
  flag: string | undefined,
  source = 'export function createTopographicSurface() { return 42; }',
) {
  vi.stubEnv('VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED', flag);
  const root = await mkdtemp(join(tmpdir(), 'cma-flow-build-'));
  roots.push(root);
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
  return {
    root,
    decoration,
    compile: () =>
      build({
        root,
        configFile: false,
        logLevel: 'silent',
        plugins: [optionalTopographicImport()],
        build: {
          minify: false,
          rollupOptions: { input: join(root, 'entry.ts'), preserveEntrySignatures: 'strict' },
        },
      }),
  };
}

it('emits a native optional import with retained exports while ordinary imports keep fatal reporting', async () => {
  const { root, compile } = await fixture('true');
  const result = await compile();
  if (Array.isArray(result) || !('output' in result)) throw new Error('Expected one build');
  const entry = result.output.find((output) => output.type === 'chunk' && output.name === 'entry');
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
});

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

it('keeps an emitted reference that contains dollar pairs verbatim', async () => {
  // Rollup reference IDs hash the emitted path and spell '-' as '$', so a build root can yield
  // '$$'. A string replacement would collapse it and fail the build with an unknown file.
  const root = await mkdtemp(join(tmpdir(), 'cma-flow-reference-'));
  roots.push(root);
  const plugin = optionalTopographicImport();
  const configResolved = plugin.configResolved as (config: ResolvedConfig) => void;
  configResolved({
    root,
    env: { VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED: 'true' },
  } as unknown as ResolvedConfig);
  const transform = plugin.transform as unknown as (
    this: { emitFile: () => string; error: (message: string) => never },
    code: string,
    id: string,
  ) => { code: string };
  const reference = 'DR$$Gkk6';
  const result = transform.call(
    {
      emitFile: () => reference,
      error: (message) => {
        throw new Error(message);
      },
    },
    await readFile(new URL('../src/decoration/topographicLoader.ts', import.meta.url), 'utf8'),
    normalizePath(resolve(realpathSync(root), 'src/decoration/topographicLoader.ts')),
  );
  expect(result.code).toContain(`import.meta.ROLLUP_FILE_URL_${reference})`);
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
