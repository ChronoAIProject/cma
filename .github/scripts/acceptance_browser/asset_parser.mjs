import { createRequire } from "node:module";
import path from "node:path";
import { pathToFileURL } from "node:url";

export function references(source, parse) {
  const tree = parse(source, { ecmaVersion: "latest", sourceType: "module" });
  const found = new Set();
  const add = (node) => {
    if (node?.type === "Literal" && typeof node.value === "string")
      found.add(node.value);
  };
  const stack = [[tree, false]];
  while (stack.length) {
    const [node, preload] = stack.pop();
    const inPreload =
      preload ||
      (node.type === "VariableDeclarator" &&
        node.id?.name === "__vite__mapDeps");
    if (
      [
        "ImportDeclaration",
        "ExportNamedDeclaration",
        "ExportAllDeclaration",
        "ImportExpression",
      ].includes(node.type)
    )
      add(node.source);
    if (node.type === "NewExpression" && node.callee?.name === "URL") {
      const value = node.arguments[0];
      if (
        value?.type === "Literal" &&
        typeof value.value === "string" &&
        /\.(js|css|woff2?|ttf|otf|png|jpe?g|svg|webp|gif|ico|wasm|avif)$/.test(
          value.value,
        )
      )
        add(value);
    }
    if (inPreload && node.type === "ArrayExpression") {
      for (const value of node.elements) add(value);
    }
    for (const value of Object.values(node)) {
      if (Array.isArray(value)) {
        for (const child of value)
          if (child?.type) stack.push([child, inPreload]);
      } else if (value?.type) stack.push([value, inPreload]);
    }
  }
  return [...found];
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  try {
    const bin = process.env.PATH.split(path.delimiter).find((item) =>
      item.endsWith("node_modules/.bin"),
    );
    const require = createRequire(
      path.join(path.dirname(bin), "parser-loader.cjs"),
    );
    const { parse } = require("acorn");
    if (require("acorn/package.json").version !== "8.15.0") throw Error();
    const chunks = [];
    let bytes = 0;
    for await (const chunk of process.stdin) {
      bytes += chunk.length;
      if (bytes > 8 * 1024 * 1024) throw Error();
      chunks.push(chunk);
    }
    process.stdout.write(
      JSON.stringify(references(Buffer.concat(chunks).toString("utf8"), parse)),
    );
  } catch {
    process.stdout.write("null");
    process.exitCode = 1;
  }
}
