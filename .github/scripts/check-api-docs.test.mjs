import assert from "node:assert/strict";
import { test } from "node:test";
import { missingOperations } from "./check-api-docs.mjs";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const document = { paths: { "/api/v1/agents/{id}": { get: {}, delete: {} } } };
const inventory = (rows) =>
  `<!-- openapi-operations:start -->\n${rows}\n<!-- openapi-operations:end -->`;

test("CMAEG inventory includes the versioned server base and detects one missing operation", () => {
  const spec = readFileSync(new URL("../../api/crates/cmaeg/openapi/v1.json", import.meta.url), "utf8");
  const markdown = readFileSync(new URL("../../docs/CMAEG_Protocol.md", import.meta.url), "utf8");
  assert.deepEqual(missingOperations(JSON.parse(spec), markdown), []);
  assert.deepEqual(missingOperations(JSON.parse(spec), markdown.replaceAll("| `POST /v1/channels` |", "| omitted |")), ["POST /v1/channels"]);
  const result = spawnSync(process.execPath, [
    ".github/scripts/check-api-docs.mjs", "api/crates/cmaeg/openapi/v1.json", "docs/CMAEG_Protocol.md",
  ], { cwd: new URL("../../", import.meta.url), encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
});

test("checks methods individually, not only a shared path", () => {
  assert.deepEqual(
    missingOperations(
      document,
      inventory("| `GET /api/v1/agents/{id}` | body |"),
    ),
    ["DELETE /api/v1/agents/{id}"],
  );
});

test("accepts complete route rows while ignoring path metadata", () => {
  assert.deepEqual(
    missingOperations(
      {
        paths: { ...document.paths, "/other": { summary: "Not an operation" } },
      },
      inventory(
        "| `GET /api/v1/agents/{id}` | body |\n| `DELETE /api/v1/agents/{id}` | empty |",
      ),
    ),
    [],
  );
});

test("does not count examples, another resource or prose outside the inventory", () => {
  const markdown =
    "| `GET /api/v1/agents/{id}` | body |\n" +
    inventory("| `GET /api/v1/agents/{other}` | body |");
  assert.deepEqual(missingOperations(document, markdown), [
    "GET /api/v1/agents/{id}",
    "DELETE /api/v1/agents/{id}",
  ]);
});

test("fails closed on missing inventory or an unusable OpenAPI export", () => {
  assert.throws(
    () => missingOperations(document, "No inventory"),
    /no operation inventory/,
  );
  for (const value of [
    null,
    {},
    { paths: [] },
    { paths: {} },
    { paths: { invalid: {} } },
  ]) {
    assert.throws(() => missingOperations(value, inventory("")));
  }
});
