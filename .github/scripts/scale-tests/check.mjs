import { readdir, readFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('.', import.meta.url));
const visit = async (directory) => {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (['results', 'node_modules', 'coverage'].includes(entry.name)) continue;
    const path = `${directory}/${entry.name}`;
    if (entry.isDirectory()) await visit(path);
    else if (entry.name.endsWith('.mjs') || entry.name.endsWith('.py')) {
      const source = await readFile(path, 'utf8');
      if (source.split('\n').length > 500) throw new Error('source exceeds 500 lines');
      if (entry.name.endsWith('.mjs')) execFileSync(process.execPath, ['--check', path], { stdio: 'pipe' });
      else execFileSync('python3', ['-c', 'import ast, pathlib, sys; ast.parse(pathlib.Path(sys.argv[1]).read_text())', path],
        { stdio: 'pipe', env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1' } });
    }
  }
};
await visit(root);
console.log('Syntax and source-size checks passed.');
