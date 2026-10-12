import { access, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import ts from 'typescript';

const packageRoot = fileURLToPath(new URL('..', import.meta.url));
const docsDirectory = join(packageRoot, 'docs');
const rustDocsDirectory = join(packageRoot, '..', 'rust', 'docs');
const declarations = join(packageRoot, 'dist', 'index.d.ts');
const expectedRustDocuments = [
  'README.md',
  'agents.md',
  'authentication.md',
  'errors.md',
  'operations.md',
  'profiles.md',
  'quickstart.md',
  'reference.md',
  'sandboxes.md',
  'streaming.md',
  'triggers.md',
];
const expectedTypeScriptDocuments = [...expectedRustDocuments].sort();

await access(declarations).catch(() => {
  throw new Error('dist/index.d.ts is absent; run npm run build before checking documentation');
});

const documents = (await readdir(docsDirectory)).filter((name) => name.endsWith('.md')).sort();
if (JSON.stringify(documents) !== JSON.stringify(expectedTypeScriptDocuments)) {
  throw new Error(
    `SDK documentation must contain exactly ${expectedTypeScriptDocuments.join(', ')}; found ${documents.join(', ')}`,
  );
}

const navOrders = new Set();
const navLabels = new Set();
const examples = [];
for (const document of documents) {
  const markdown = await readFile(join(docsDirectory, document), 'utf8');
  const nav = /^# [^\n]+\n\n<!-- nav: ([1-9]\d*) ([^\n]+) -->\n/.exec(markdown);
  if (nav === null) {
    throw new Error(`${document} must begin with an H1 followed by <!-- nav: <order> <label> -->`);
  }
  const [, order, label] = nav;
  if (navOrders.has(order) || navLabels.has(label)) {
    throw new Error(`${document} repeats documentation navigation metadata`);
  }
  navOrders.add(order);
  navLabels.add(label);

  const fence = /```(ts|tsx|typescript)\s*\n([\s\S]*?)```/g;
  for (const match of markdown.matchAll(fence)) {
    examples.push({ document, source: match[2], jsx: match[1] === 'tsx' });
  }
}

const rustDocuments = (await readdir(rustDocsDirectory))
  .filter((name) => name.endsWith('.md'))
  .sort();
if (JSON.stringify(rustDocuments) !== JSON.stringify(expectedRustDocuments)) {
  throw new Error(
    `Rust documentation must contain exactly ${expectedRustDocuments.join(', ')}; found ${rustDocuments.join(', ')}`,
  );
}
const rustNavOrders = new Set();
const rustNavLabels = new Set();
for (const document of rustDocuments) {
  const markdown = await readFile(join(rustDocsDirectory, document), 'utf8');
  const nav = /^# [^\n]+\n\n<!-- nav: ([1-9]\d*) ([^\n]+) -->\n/.exec(markdown);
  if (nav === null) {
    throw new Error(
      `Rust ${document} must begin with an H1 followed by <!-- nav: <order> <label> -->`,
    );
  }
  const [, order, label] = nav;
  if (rustNavOrders.has(order) || rustNavLabels.has(label)) {
    throw new Error(`Rust ${document} repeats documentation navigation metadata`);
  }
  rustNavOrders.add(order);
  rustNavLabels.add(label);
}

if (examples.length < 4) {
  throw new Error(`expected at least four TypeScript examples, found ${String(examples.length)}`);
}

const scratch = await mkdtemp(join(tmpdir(), 'cma-sdk-docs-'));
try {
  await writeFile(join(scratch, 'package.json'), JSON.stringify({ type: 'module' }));
  const roots = [];
  for (const [index, example] of examples.entries()) {
    const path = join(
      scratch,
      `${basename(example.document, '.md')}-${String(index + 1)}.${example.jsx ? 'tsx' : 'mts'}`,
    );
    await writeFile(path, `export {};\n${example.source}`, 'utf8');
    roots.push(path);
  }

  const options = {
    target: ts.ScriptTarget.ES2022,
    lib: ['lib.es2022.d.ts', 'lib.dom.d.ts', 'lib.dom.iterable.d.ts'],
    module: ts.ModuleKind.NodeNext,
    moduleResolution: ts.ModuleResolutionKind.NodeNext,
    moduleDetection: ts.ModuleDetectionKind.Force,
    strict: true,
    jsx: ts.JsxEmit.ReactJSX,
    noEmit: true,
    skipLibCheck: true,
    verbatimModuleSyntax: true,
    baseUrl: packageRoot,
    paths: {
      '@chronoai/cma-sdk': ['./dist/index.d.ts'],
      '@chronoai/cma-sdk/react': ['./dist/react.d.ts'],
      react: ['./node_modules/@types/react/index.d.ts'],
      'react/*': ['./node_modules/@types/react/*.d.ts'],
    },
    types: ['node'],
    typeRoots: [join(packageRoot, 'node_modules', '@types')],
  };
  const program = ts.createProgram(roots, options);
  const diagnostics = ts.getPreEmitDiagnostics(program);
  if (diagnostics.length > 0) {
    throw new Error(
      ts.formatDiagnosticsWithColorAndContext(diagnostics, {
        getCanonicalFileName: (name) => name,
        getCurrentDirectory: () => packageRoot,
        getNewLine: () => '\n',
      }),
    );
  }
} finally {
  await rm(scratch, { recursive: true, force: true });
}

console.log(
  `Checked ${String(examples.length)} TypeScript examples across ${String(documents.length)} documents.`,
);
