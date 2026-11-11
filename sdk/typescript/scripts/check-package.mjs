import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = fileURLToPath(new URL('..', import.meta.url));
const scratch = await mkdtemp(join(tmpdir(), 'cma-sdk-package-'));
const run = (program, args, cwd = scratch) => execFileSync(program, args, { cwd, stdio: 'pipe' });

try {
  const packed = JSON.parse(
    run('npm', ['pack', '--json', '--pack-destination', scratch], packageRoot),
  );
  const archive = join(scratch, packed[0].filename);
  const pkg = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'));
  const lock = JSON.parse(await readFile(join(packageRoot, 'package-lock.json'), 'utf8'));
  const manifest = {
    name: 'cma-sdk-package-consumer',
    version: '0.0.0',
    private: true,
    type: 'module',
    dependencies: { ...pkg.dependencies, '@chronoai/cma-sdk': `file:${archive}` },
    devDependencies: pkg.devDependencies,
  };
  const consumerLock = {
    ...lock,
    name: manifest.name,
    version: manifest.version,
    packages: {
      ...lock.packages,
      '': manifest,
      'node_modules/@chronoai/cma-sdk': {
        version: pkg.version,
        resolved: `file:${archive}`,
        integrity: `sha512-${createHash('sha512')
          .update(await readFile(archive))
          .digest('base64')}`,
        dependencies: pkg.dependencies,
        peerDependencies: pkg.peerDependencies,
        peerDependenciesMeta: pkg.peerDependenciesMeta,
      },
    },
  };
  await writeFile(join(scratch, 'package.json'), JSON.stringify(manifest));
  await writeFile(join(scratch, 'package-lock.json'), JSON.stringify(consumerLock));
  run('npm', ['ci', '--ignore-scripts', '--no-audit', '--no-fund']);

  const descriptor = JSON.parse(
    await readFile(
      new URL(
        '../../../api/crates/layer1-public/tests/fixtures/trigger-embed/descriptor.json',
        import.meta.url,
      ),
      'utf8',
    ),
  );
  const separator = descriptor.snippets.react.indexOf('\n\n');
  if (separator < 0) throw new Error('React descriptor must contain an import and expression');
  const source = `${descriptor.snippets.react.slice(0, separator)}\nexport function PublishedButton() {\nreturn (${descriptor.snippets.react.slice(separator + 2)});\n}\n`;
  await writeFile(join(scratch, 'embed.tsx'), source);
  await writeFile(
    join(scratch, 'tsconfig.json'),
    JSON.stringify({
      compilerOptions: {
        target: 'ES2022',
        module: 'NodeNext',
        moduleResolution: 'NodeNext',
        jsx: 'react-jsx',
        strict: true,
        skipLibCheck: false,
        outDir: 'dist',
      },
      include: ['embed.tsx'],
    }),
  );
  run(process.execPath, [
    join(scratch, 'node_modules/typescript/bin/tsc'),
    '--project',
    'tsconfig.json',
  ]);
  await writeFile(join(scratch, 'descriptor.json'), JSON.stringify(descriptor));
  const navigationBudget = JSON.parse(
    await readFile(
      new URL(
        '../../../api/crates/layer1-public/tests/fixtures/browser-auth-limits.json',
        import.meta.url,
      ),
      'utf8',
    ),
  );
  await writeFile(
    join(scratch, 'smoke.mjs'),
    `
import { readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createClient, MAX_LOGIN_URL_BYTES } from '@chronoai/cma-sdk';
if (MAX_LOGIN_URL_BYTES !== ${JSON.stringify(navigationBudget.max_login_url_bytes)})
  throw new Error('Packaged login URL budget export does not match the public contract');
import { PublishedButton } from './dist/embed.js';
const descriptor = JSON.parse(readFileSync(new URL('./descriptor.json', import.meta.url), 'utf8'));
let fetched = false;
globalThis.fetch = async () => { fetched = true; throw new Error('Rendering cannot fetch'); };
const html = renderToStaticMarkup(createElement(PublishedButton));
if (fetched || !html.includes('href="' + descriptor.public_url + '"') ||
    !html.includes('src="' + descriptor.badge_url.replaceAll('&', '&amp;') + '"'))
  throw new Error('Packaged React snippet did not match the backend descriptor');
const client = createClient({ baseUrl: 'https://example.invalid', fetch: async () => Response.json({
  service: 'cma', status: 'rebuilding', interfaces: [],
}) });
if ((await client.readConformance()).service !== 'cma') throw new Error('Root export failed');
`,
  );
  run(process.execPath, ['smoke.mjs']);
  console.log(
    'Packaged SDK root, login URL budget and backend React embed pass in an isolated npm ci consumer.',
  );
} catch (error) {
  if (error.stdout) process.stderr.write(error.stdout);
  if (error.stderr) process.stderr.write(error.stderr);
  throw error;
} finally {
  await rm(scratch, { recursive: true, force: true });
}
