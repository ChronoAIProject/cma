import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const methods = [
  "get",
  "post",
  "put",
  "patch",
  "delete",
  "head",
  "options",
  "trace",
];

export function missingOperations(document, markdown) {
  if (!document || !document.paths || Array.isArray(document.paths)) {
    throw new Error(
      "The input must be an OpenAPI document with a paths object.",
    );
  }
  const inventory = markdown.match(
    /<!-- openapi-operations:start -->([\s\S]*?)<!-- openapi-operations:end -->/,
  )?.[1];
  if (inventory === undefined)
    throw new Error("The documentation has no operation inventory.");
  const documented = new Set(
    [
      ...inventory.matchAll(
        /\| `(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS|TRACE) ([^`\s]+)` \|/g,
      ),
    ].map((match) => `${match[1]} ${match[2]}`),
  );
  const base = document.servers?.[0]?.url ?? "";
  const prefix = base.startsWith("/") ? base.replace(/\/$/, "") : "";
  const operations = Object.entries(document.paths).flatMap(([path, item]) => {
    if (
      !path.startsWith("/") ||
      !item ||
      typeof item !== "object" ||
      Array.isArray(item)
    ) {
      throw new Error("The OpenAPI paths must contain path-item objects.");
    }
    return methods
      .filter((method) => item[method])
      .map((method) => `${method.toUpperCase()} ${prefix}${path}`);
  });
  if (operations.length === 0)
    throw new Error("The OpenAPI document has no operations.");
  return operations.filter((operation) => !documented.has(operation));
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    const args = process.argv.slice(2);
    if (args.length !== 0 && args.length !== 2)
      throw new Error("usage: check-api-docs.mjs [openapi.json documentation.md]");
    const document = JSON.parse(readFileSync(args[0] ?? 0, "utf8"));
    const markdown = readFileSync(
      args[1] ?? new URL("../../api/crates/layer1-public/tests/fixtures/openapi-operation-inventory.md", import.meta.url),
      "utf8",
    );
    const missing = missingOperations(document, markdown);
    if (missing.length > 0) {
      throw new Error(`Missing documented operations:\n${missing.join("\n")}`);
    }
    console.log(
      `API documentation covers all operations across ${Object.keys(document.paths).length} OpenAPI paths.`,
    );
  } catch (error) {
    console.error(
      error instanceof Error
        ? error.message
        : "API documentation check failed.",
    );
    process.exitCode = 1;
  }
}
