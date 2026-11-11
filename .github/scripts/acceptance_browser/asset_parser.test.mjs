import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import test from "node:test";
import { references } from "./asset_parser.mjs";

const bin = process.env.PATH.split(path.delimiter).find((item) =>
  item.endsWith("node_modules/.bin"),
);
const require = createRequire(path.join(path.dirname(bin), "parser-test.cjs"));
const { parse } = require("acorn");

test("extract executable imports, exports, worker URLs and Vite preloads", () => {
  const result = references(
    `
    import './entry.js'; export { x } from './next.js';
    const lazy = () => import('./lazy.js');
    const worker = new Worker(new URL('/assets/worker.js', import.meta.url));
    const __vite__mapDeps = (i, m = ['assets/preload.js', 'assets/page.css']) => i.map(i => m[i]);
  `,
    parse,
  );
  assert.deepEqual(
    new Set(result),
    new Set([
      "./entry.js",
      "./next.js",
      "./lazy.js",
      "/assets/worker.js",
      "assets/preload.js",
      "assets/page.css",
    ]),
  );
});

test("documentation strings and commented imports are not dependencies", () => {
  assert.deepEqual(
    references(
      `
    const sample = "import './sample.js'";
    const extension = './sample.png';
    // import './commented.js';
    const unused = ['./not-a-preload.css'];
  `,
      parse,
    ),
    [],
  );
});
