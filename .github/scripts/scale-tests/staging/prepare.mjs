import { createRequire } from 'node:module';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { hash, check } from '../lib/checks.mjs';
import { ROOT, outputDirectory } from './authority.mjs';

const requireTool = createRequire(import.meta.url);
export async function sourceHash() {
  const files = [];
  async function visit(path) {
    for (const entry of await readdir(join(ROOT, path), { withFileTypes: true })) {
      const child = `${path}/${entry.name}`;
      if (entry.isDirectory()) await visit(child);
      else if (/\.(mjs|ts|json)$/.test(child)) files.push(child);
    }
  }
  for (const path of ['.github/scripts/scale-tests/staging', '.github/scripts/scale-tests/lib',
    '.github/scripts/scale-tests/timing/lib', 'web/src/api-client']) await visit(path);
  files.push('web/package-lock.json', '.github/scripts/scale-tests/package-lock.json');
  const parts = await Promise.all(files.sort().map(async (path) => `${path}\0${hash(await readFile(join(ROOT, path)))}`));
  return hash(parts.join('\n'));
}
export async function prepareWire(path) {
  await outputDirectory(path);
  const { build } = requireTool('esbuild');
  const result = await build({ stdin: { contents: [
    "export { agentFrames } from './web/src/api-client/agent-events-v2/stream.ts';",
    "export { v2AgentEventHead } from './web/src/api-client/agent-events-v2/types.ts';",
  ].join('\n'), resolveDir: ROOT, loader: 'ts' }, bundle: true, platform: 'node', format: 'esm',
  write: false, logLevel: 'silent', nodePaths: [join(ROOT, '.github/scripts/scale-tests/node_modules')] });
  const bytes = result.outputFiles[0].contents;
  await writeFile(path, bytes, { mode: 0o600, flag: 'wx' });
  return { driverHash: await sourceHash(), wireHash: hash(bytes) };
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    check(process.argv.length === 4 && process.argv[2] === '--out', 'prepare_arguments');
    console.log(JSON.stringify(await prepareWire(resolve(process.argv[3]))));
  } catch { console.error('Staging parser preparation failed; private details withheld.'); process.exitCode = 1; }
}
